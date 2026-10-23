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
//!   Cell readiness and removal go out as [`CellEvent`]s for presentation consumers
//!   (baked `AcousticCell` probes, terrain tiles), in order.
//!
//! Install it after every plugin that registers components: the world workers prepare
//! rows against the registry as it is at registration.
use ara_doc::Workspace;
use ara_runtime::{RuntimeError, components::PlayerControl, plugin::RuntimePlugin};
use ara_schema::SchemaDb;
use ara_sim::{Session, SimRegistry, StreamingBoundary};
use ara_world::{
    ActivationBudget, CellBlob, CellKey, CellSource, HlodProxy, Interest, Manifest,
    StreamConfig, Streaming, StreamingSource, WorkerPool, cook_cell,
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
    schema: SchemaDb,
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
    source: Arc<SnapshotSource>,
    /// Only ever reached through `&mut self` (`Mutex::get_mut`, never locked): the
    /// worker pool's channels are `Send` but not `Sync`, and plugins must be `Sync`.
    streaming: Option<Mutex<Streaming>>,
    events: Events,
    budget: ActivationBudget,
    workers: usize,
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
    /// The plugin for the snapshot's world document, if it has one (several: the
    /// first by ID). Returns the plugin and its cell event receiver.
    pub fn from_workspace(
        workspace: &Workspace,
        schema: &SchemaDb,
    ) -> ara_runtime::Result<Option<(Self, Receiver<CellEvent>)>> {
        let Some((_, world)) = workspace.docs().find(|(_, d)| d.kind == "world") else {
            return Ok(None);
        };
        let manifest = Manifest::read(world).map_err(runtime_error)?;
        let (tx, rx) = channel();
        Ok(Some((
            Self {
                manifest: manifest.clone(),
                source: Arc::new(SnapshotSource {
                    manifest,
                    workspace: workspace.clone(),
                    schema: schema.clone(),
                }),
                streaming: None,
                events: Events(tx),
                // A deterministic entity ceiling bounds the work at each boundary; the
                // time budget is generous so the ceiling decides (replay-stable).
                budget: ActivationBudget {
                    time: Duration::from_millis(50),
                    max_entities: 64,
                },
                workers: 2,
            },
            rx,
        )))
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
        let sources: Vec<StreamingSource> = sources.into_iter().map(|(_, s)| s).collect();
        streaming
            .update(&self.manifest, &sources, tick, &mut self.events)
            .map_err(runtime_error)
    }

    fn activate(&mut self, boundary: &mut StreamingBoundary<'_>) -> ara_runtime::Result<()> {
        if let Some(streaming) = self.streaming.as_mut().map(exclusive).transpose()? {
            streaming
                .activate(boundary, self.budget, &mut self.events)
                .map_err(runtime_error)?;
        }
        Ok(())
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
        let pool = WorkerPool::new(
            self.source.clone(),
            registry.clone(),
            self.workers,
            CELL_BYTES,
        )
        .map_err(runtime_error)?;
        self.streaming = Some(Mutex::new(
            Streaming::new(
                &self.manifest,
                pool,
                StreamConfig {
                    memory_bytes: MEMORY_BYTES,
                    cell_reservation: CELL_BYTES,
                    lookahead_seconds: 1.0,
                },
            )
            .map_err(runtime_error)?,
        ));
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
        let key = CellKey { x: 1, z: 0, slot: 0 };
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
