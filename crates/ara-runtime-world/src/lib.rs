//! World streaming in play: the play snapshot's `.world` (ara-world) streams its cells
//! into the running simulation around the players, at the session's streaming
//! boundary (`RuntimePlugin::stream` → `Session::step_with_streaming`), so arrivals are
//! recorded and replay exactly.
//!
//! - [`SnapshotSource`]: cells cooked from the snapshot's in-memory documents on the
//!   world workers (no project files in a player process).
//! - [`WorldStreaming`]: the plugin. `before_tick` plans interest from every
//!   `PlayerControl` entity's pose; `stream` activates within an entity budget;
//!   `mirror` streams a networked client's never-stepped world around its own player.
//!   [`WorldStreaming::settled`] makes arrivals reproducible (waits for cooks).
//!   Cell readiness and removal go out as [`CellEvent`]s for presentation consumers
//!   (baked `AcousticCell` probes, terrain tiles), in order.
//!
//! Install it after every plugin that registers components: the world workers prepare
//! rows against the registry as it is at registration.
use ara_doc::Workspace;
use ara_runtime::{RuntimeError, components::PlayerControl, plugin::RuntimePlugin};
use ara_sim::{Session, SimRegistry, StreamingBoundary};
use ara_world::{
    ActivationBudget, CellBlob, CellKey, CellSource, HlodProxy, Interest, Manifest, StreamConfig,
    Streaming, StreamingSource, WorkerPool, cook_cell,
};
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::Duration,
};

/// Cell reservation (bytes) and the total streaming budget.
const CELL_BYTES: usize = 8 * 1024 * 1024;
const MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Cells cooked from the play snapshot's documents (cell scenes and their prefabs).
pub struct SnapshotSource {
    manifest: Manifest,
    workspace: Workspace,
    schema: ara_schema::SchemaDb,
}

impl CellSource for SnapshotSource {
    fn load(&self, cell: &str, max_bytes: usize) -> ara_world::Result<CellBlob> {
        let blob = cook_cell(&self.manifest, cell, &self.workspace, &self.schema, None)?;
        if blob.encode()?.len() > max_bytes {
            return Err(ara_world::Error::Invalid(format!(
                "cell {cell} exceeds its {max_bytes}-byte reservation; split it"
            )));
        }
        Ok(blob)
    }
}

/// A cell finished activating (`Ready`) or was fully removed (`Removed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellEvent {
    /// Every entity of the cell is resident.
    Ready {
        /// Cell record ID.
        cell: String,
        /// Grid key (acoustic and terrain data share it).
        key: CellKey,
    },
    /// The cell's entities are gone.
    Removed {
        /// Cell record ID.
        cell: String,
        /// Grid key.
        key: CellKey,
    },
}

/// Forwards ara-world interest callbacks as events.
struct Events(Sender<CellEvent>);

impl Interest for Events {
    fn ready(&mut self, cell: &str, key: CellKey, _: bool, _: &[HlodProxy]) {
        let _ = self.0.send(CellEvent::Ready {
            cell: cell.to_owned(),
            key,
        });
    }
    fn removed(&mut self, cell: &str, key: CellKey) {
        let _ = self.0.send(CellEvent::Removed {
            cell: cell.to_owned(),
            key,
        });
    }
    fn visibility(&mut self, _: &str, _: bool) {}
}

/// The streaming plugin for one world.
pub struct WorldStreaming {
    manifest: Manifest,
    /// The snapshot documents the cells cook from.
    workspace: Workspace,
    /// Only ever reached through `&mut self` (`Mutex::get_mut`, never locked): the
    /// worker pool's channels are `Send` but not `Sync`, and plugins must be `Sync`.
    streaming: Option<Mutex<Streaming>>,
    events: Events,
    budget: ActivationBudget,
    workers: usize,
    /// Wait for in-flight cooks before each boundary (reproducible arrivals).
    settle: Option<Duration>,
    /// The registry the world workers prepare rows against (kept for restores).
    registry: Option<SimRegistry>,
    /// After a restore: cells the restored world holds, kept wanted (and adopted)
    /// until resident again.
    restoring: Vec<String>,
    /// Cells whose cook failure was already reported.
    failed: std::collections::BTreeSet<String>,
}

/// The streaming state through `&mut` (the mutex is never locked, so never poisoned).
fn exclusive(streaming: &mut Mutex<Streaming>) -> ara_runtime::Result<&mut Streaming> {
    streaming
        .get_mut()
        .map_err(|_| RuntimeError("world streaming state poisoned".into()))
}

fn runtime_error(e: ara_world::Error) -> RuntimeError {
    RuntimeError(format!("world streaming: {e}"))
}

impl WorldStreaming {
    /// The plugin for the world `scene` streams (its `StreamedWorld` component), if it
    /// streams one. Returns the plugin and its cell event receiver.
    pub fn from_workspace(
        workspace: &Workspace,
        scene: &str,
    ) -> ara_runtime::Result<Option<(Self, Receiver<CellEvent>)>> {
        let Some(reference) = workspace.doc(scene).and_then(|doc| {
            doc.records().values().find_map(|record| {
                let world = record.get("StreamedWorld")?.get("world")?.as_str()?;
                Some(ara_schema::AssetRef(world.to_owned()))
            })
        }) else {
            return Ok(None);
        };
        let id = ara_runtime::assets::asset_id(&reference)?;
        let world = workspace
            .doc(id)
            .filter(|d| d.kind == "world")
            .ok_or_else(|| RuntimeError(format!("{} is not a loaded world", reference.0)))?;
        let manifest = Manifest::read(world).map_err(runtime_error)?;
        let (tx, rx) = channel();
        Ok(Some((
            Self {
                manifest,
                workspace: workspace.clone(),
                streaming: None,
                events: Events(tx),
                // A deterministic entity ceiling bounds the work at each boundary; the
                // time budget is generous so the ceiling decides (replay-stable).
                budget: ActivationBudget {
                    time: Duration::from_millis(50),
                    max_entities: 64,
                },
                workers: 2,
                settle: None,
                registry: None,
                restoring: Vec::new(),
                failed: Default::default(),
            },
            rx,
        )))
    }

    /// Settled streaming: before each boundary, waits (at most `timeout`) until every
    /// wanted cell is cooked, so cells arrive at the first boundary after they become
    /// wanted, whatever the worker speed. Same seed and inputs then give the same state
    /// hash across runs and machines (agent playtests, headless players). Live windowed
    /// play leaves it off: a slow cook never stalls a frame.
    pub fn settled(mut self, timeout: Duration) -> Self {
        self.settle = Some(timeout);
        self
    }

    /// The world's cell documents: the projected scene must not also contain them.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Plans interest around the players of `session`'s world at `tick`.
    fn update(&mut self, session: &Session, tick: u64) -> ara_runtime::Result<()> {
        let Some(streaming) = self.streaming.as_mut().map(exclusive).transpose()? else {
            return Ok(());
        };
        let world = session.world();
        let mut sources: Vec<(u64, StreamingSource)> = world
            .iter_entities()
            .filter_map(|e| {
                let player = e.get::<PlayerControl>()?.player;
                let pose = e.get::<ara_runtime::sync::WorldPose>()?.0;
                Some((
                    player,
                    StreamingSource {
                        position: pose.translation.to_array(),
                        velocity: e
                            .get::<ara_physics::Velocity>()
                            .map_or([0.0; 3], |v| v.linear),
                        range_scale: 1.0,
                    },
                ))
            })
            .collect();
        sources.sort_by_key(|(player, _)| *player);
        let mut sources: Vec<StreamingSource> = sources.into_iter().map(|(_, s)| s).collect();
        // Restored cells stay wanted (a zero-range source at their centre) until
        // they are resident again.
        let size = self.manifest.settings.cell_size;
        sources.extend(self.restoring.iter().filter_map(|cell| {
            let key = self.manifest.cells.get(cell)?.key();
            Some(StreamingSource {
                position: [
                    (key.x as f32 + 0.5) * size,
                    0.0,
                    (key.z as f32 + 0.5) * size,
                ],
                velocity: [0.0; 3],
                range_scale: 0.0,
            })
        }));
        match self.settle {
            Some(timeout) => {
                let settled = streaming
                    .settle(&self.manifest, &sources, tick, &mut self.events, timeout)
                    .map_err(runtime_error)?;
                if !settled {
                    return Err(RuntimeError(format!(
                        "world streaming: cells were not cooked within {timeout:?}"
                    )));
                }
            }
            None => streaming
                .update(&self.manifest, &sources, tick, &mut self.events)
                .map_err(runtime_error)?,
        }
        // A cell that cannot cook is a content error: report it once (it stays out
        // until retried; everything else keeps streaming).
        for cell in self.manifest.cells.keys() {
            if let Some(error) = streaming.failure(cell)
                && self.failed.insert(cell.clone())
            {
                return Err(RuntimeError(format!(
                    "world streaming: cell {cell} failed to load: {error}"
                )));
            }
        }
        Ok(())
    }

    fn activate(&mut self, boundary: &mut StreamingBoundary<'_>) -> ara_runtime::Result<()> {
        if let Some(streaming) = self.streaming.as_mut().map(exclusive).transpose()? {
            streaming
                .activate(boundary, self.budget, &mut self.events)
                .map_err(runtime_error)?;
            if !self.restoring.is_empty()
                && self
                    .restoring
                    .iter()
                    .all(|cell| streaming.is_resident(cell))
            {
                // Every restored cell is adopted: duplicates are errors again.
                streaming.adopt_resident(false);
                self.restoring.clear();
            }
        }
        Ok(())
    }

    /// A fresh streaming state over a new worker pool. Cells cook against the session
    /// registry's schema (runtime and plugin types included), as the workers check.
    fn fresh(&self, registry: &SimRegistry) -> ara_runtime::Result<Streaming> {
        let source = Arc::new(SnapshotSource {
            manifest: self.manifest.clone(),
            workspace: self.workspace.clone(),
            schema: registry.schema().clone(),
        });
        let pool = WorkerPool::new(source, registry.clone(), self.workers, CELL_BYTES)
            .map_err(runtime_error)?;
        Streaming::new(
            &self.manifest,
            pool,
            StreamConfig {
                memory_bytes: MEMORY_BYTES,
                cell_reservation: CELL_BYTES,
                lookahead_seconds: 1.0,
            },
        )
        .map_err(runtime_error)
    }

    /// Whether `cell`'s entities are all resident.
    pub fn is_resident(&self, cell: &str) -> bool {
        self.streaming
            .as_ref()
            .and_then(|s| s.lock().ok())
            .is_some_and(|s| s.is_resident(cell))
    }
}

impl RuntimePlugin for WorldStreaming {
    fn name(&self) -> &'static str {
        "world"
    }

    fn register(&mut self, registry: &mut SimRegistry) -> ara_runtime::Result<()> {
        self.streaming = Some(Mutex::new(self.fresh(registry)?));
        self.registry = Some(registry.clone());
        Ok(())
    }

    /// The restored world holds exactly the rows resident at the checkpoint: start a
    /// fresh streaming state (ticks may have gone backwards), report the cells that
    /// were resident as removed, and keep every cell with rows in the world wanted
    /// in adoption mode until resident again (its rows count as spawned).
    fn restored(&mut self, session: &mut Session) -> ara_runtime::Result<()> {
        let Some(registry) = self.registry.clone() else {
            return Ok(());
        };
        if let Some(old) = self.streaming.as_mut().map(exclusive).transpose()? {
            for (id, cell) in &self.manifest.cells {
                if old.is_resident(id) {
                    self.events.removed(id, cell.key());
                }
            }
        }
        let mut streaming = self.fresh(&registry)?;
        streaming.adopt_resident(true);
        self.streaming = Some(Mutex::new(streaming));
        self.restoring = self
            .manifest
            .cells
            .keys()
            .filter(|cell| {
                self.manifest
                    .documents(&self.workspace, cell)
                    .is_ok_and(|docs| {
                        docs.iter().any(|(_, doc)| {
                            doc.records()
                                .keys()
                                .any(|id| session.entity(id.as_str()).is_some())
                        })
                    })
            })
            .cloned()
            .collect();
        Ok(())
    }

    fn before_tick(&mut self, session: &mut Session) -> ara_runtime::Result<()> {
        let tick = session.tick() + 1;
        self.update(session, tick)
    }

    fn stream(&mut self, boundary: &mut StreamingBoundary<'_>) -> ara_runtime::Result<()> {
        self.activate(boundary)
    }

    fn mirror(&mut self, session: &mut Session, tick: u64) -> ara_runtime::Result<()> {
        self.update(session, tick)?;
        let mut result = Ok(());
        session.mirror_streaming(|boundary| {
            result = self.activate(boundary);
            Ok(())
        })?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_forward_interest_in_order() {
        let (tx, rx) = channel();
        let mut events = Events(tx);
        let key = CellKey {
            x: 1,
            z: 0,
            slot: 0,
        };
        events.ready("c_a", key, true, &[]);
        events.removed("c_a", key);
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            got,
            [
                CellEvent::Ready {
                    cell: "c_a".into(),
                    key
                },
                CellEvent::Removed {
                    cell: "c_a".into(),
                    key
                }
            ]
        );
    }
}
