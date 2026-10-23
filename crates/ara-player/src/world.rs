//! One running world inside the player: the runtime plus the `play.*` method
//! implementations. Patches acknowledge after the tick that applied them.
use crate::crash::LAST_TICK;
use anyhow::{Context, Result, anyhow, bail};
use ara_asset::{Digest, FileCache};
use ara_input::{ActionValue, Injection, PlayerId};
use ara_play::{
    Captured, Input, Mark, Patch, Patched, PlayScene, Sample, Set, Started, Stepped, unhex,
};
use ara_render::capture::ViewMode;
use ara_runtime::{
    Runtime, RuntimeConfig, assets::CachedAssets, extract::RenderFrame, live::set_edit,
};
use ara_runtime_render::Presenter;
use ara_sim::{StepInput, TickConfig, Watch};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::Ordering, time::Instant};

/// A patch waiting for its tick boundary.
pub struct PendingPatch {
    /// Request ID to answer.
    pub id: Value,
    /// Tick whose boundary applies it.
    pub tick: u64,
    /// The edits, for read-back.
    pub sets: Vec<Set>,
}

/// The assigned world.
pub struct World {
    /// The simulation and presentation source.
    pub runtime: Runtime,
    scene: ara_doc::AssetId,
    seed: u64,
    cache: PathBuf,
    pins: BTreeMap<String, Digest>,
    /// Patches acknowledged after their tick.
    pub pending: Vec<PendingPatch>,
}

fn assets(cache: &PathBuf, pins: &BTreeMap<String, Digest>) -> Result<CachedAssets> {
    Ok(CachedAssets::new(FileCache::at(cache)?, pins.clone()))
}

impl World {
    /// Builds the world from a decoded snapshot (tick 0, paused).
    pub fn build(scene: &PlayScene) -> Result<(Self, f64)> {
        let started = Instant::now();
        let schema = ara_runtime::schema(&scene.types)?;
        let (workspace, id) = scene.workspace().map_err(|e| anyhow!(e))?;
        let pins = scene
            .assets
            .iter()
            .map(|(k, v)| Ok((k.clone(), Digest(unhex(v).map_err(|e| anyhow!(e))?))))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let cache = PathBuf::from(&scene.cache);
        let runtime = Runtime::new(
            schema,
            workspace,
            id.clone(),
            Box::new(assets(&cache, &pins)?),
            RuntimeConfig {
                tick: TickConfig {
                    hz: scene.tick_hz,
                    ..Default::default()
                },
                seed: scene.seed,
                nav_cache: scene.nav_cache.as_ref().map(PathBuf::from),
                ..Default::default()
            },
            Vec::new(),
        )?;
        Ok((
            Self {
                runtime,
                scene: id,
                seed: scene.seed,
                cache,
                pins,
                pending: Vec::new(),
            },
            started.elapsed().as_secs_f64() * 1000.0,
        ))
    }

    /// Starts host-time ticking and runs the first tick now (S5 "first tick").
    pub fn start(&mut self, paused: bool, build_ms: f64) -> Result<Started> {
        let mut first_tick_ms = 0.0;
        if !paused {
            let t = Instant::now();
            self.runtime.step(1, &[])?;
            first_tick_ms = t.elapsed().as_secs_f64() * 1000.0;
            self.runtime.resume()?;
        }
        self.mark();
        Ok(Started {
            tick: self.runtime.tick(),
            entities: self.entities(),
            build_ms,
            first_tick_ms,
            net_handshake_ms: None,
        })
    }

    fn entities(&self) -> usize {
        self.runtime
            .world()
            .iter_entities()
            .filter(|e| e.contains::<ara_sim::Identity>())
            .count()
    }

    /// Publishes the latest tick for crash reports.
    pub fn mark(&self) {
        LAST_TICK.store(self.runtime.tick(), Ordering::Relaxed);
    }

    fn hash(&self) -> Result<String> {
        Ok(self.runtime.play().session().state_hash()?.to_string())
    }

    /// `play.step`.
    pub fn step(&mut self, ticks: u32, inputs: &[Input]) -> Result<Stepped> {
        let inputs = step_inputs(inputs)?;
        let report = self.runtime.step(ticks, &inputs)?;
        self.mark();
        Ok(Stepped {
            ticks: report.ticks,
            tick: report.tick,
            reached: report.reached,
            hash: self.hash()?,
            samples: samples(&report.samples),
        })
    }

    /// `play.until`.
    pub fn until(&mut self, until: &ara_play::Until) -> Result<Stepped> {
        let inputs = step_inputs(&until.inputs)?;
        let (path, compare, value) = (until.path.clone(), until.compare, until.value.clone());
        // Evaluated through the read-only world after each tick.
        let mut error = None;
        let report =
            self.runtime
                .step_until(until.max_ticks, &inputs, |session| {
                    match read_path(session, &path) {
                        Ok(Some(actual)) => compare.test(&actual, &value),
                        Ok(None) => false,
                        Err(e) => {
                            error = Some(e);
                            true
                        }
                    }
                })?;
        if let Some(e) = error {
            return Err(e);
        }
        self.mark();
        Ok(Stepped {
            ticks: report.ticks,
            tick: report.tick,
            reached: report.reached,
            hash: self.hash()?,
            samples: samples(&report.samples),
        })
    }

    /// `play.input`: injections for the next tick (offsets relative to it).
    pub fn input(&mut self, inputs: &[Input]) -> Result<u64> {
        let next = self.runtime.tick() + 1;
        for input in inputs {
            let injection = injection(input)?;
            self.runtime
                .play_mut()
                .session_mut()
                .inject(next + u64::from(input.after_ticks), vec![injection])?;
        }
        Ok(next)
    }

    /// `play.inspect`.
    pub fn inspect(&self, entity: &str, components: &[String]) -> Result<Value> {
        let session = self.runtime.play().session();
        session
            .entity(entity)
            .with_context(|| format!("unknown entity {entity}"))?;
        let names: Vec<String> = if components.is_empty() {
            self.runtime
                .schema()
                .components()
                .map(|c| c.name.clone())
                .collect()
        } else {
            components.to_vec()
        };
        let mut out = serde_json::Map::new();
        for name in names {
            if let Some(value) = session.component(entity, &name)? {
                out.insert(name, value);
            }
        }
        Ok(json!({"tick": self.runtime.tick(), "entity": entity, "components": out}))
    }

    /// `play.patch`: queues the edits; returns them if they must be acked later.
    pub fn patch(&mut self, patch: &Patch) -> Result<u64> {
        let edits = patch
            .sets
            .iter()
            .map(|s| {
                set_edit(&self.scene, &s.entity, &s.path, s.value.clone()).map_err(|e| anyhow!(e.0))
            })
            .collect::<Result<Vec<_>>>()?;
        self.runtime.patch_edits(edits)?;
        Ok(self.runtime.tick() + 1)
    }

    /// Read-back values of patched paths.
    pub fn patched(&self, tick: u64, sets: &[Set]) -> Patched {
        Patched {
            applied_at_tick: tick,
            read_back: sets
                .iter()
                .map(|s| {
                    read_path(
                        self.runtime.play().session(),
                        &format!("{}.{}", s.entity, s.path),
                    )
                    .ok()
                    .flatten()
                })
                .collect(),
        }
    }

    /// `play.watch`.
    pub fn watch(&mut self, paths: &[String], every: u32) -> Result<()> {
        let watches = paths
            .iter()
            .map(|p| Watch::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        self.runtime.play_mut().watch(watches, every)?;
        Ok(())
    }

    /// `play.capture`: state hash plus an optional labelled image.
    pub fn capture(
        &mut self,
        params: &ara_play::CaptureParams,
        presenter: Option<&mut Presenter>,
        frame: &mut RenderFrame,
    ) -> Result<Captured> {
        let mut marks = Vec::new();
        if let (Some(path), Some(presenter)) = (&params.path, presenter) {
            self.runtime.extract(frame);
            let capture = presenter.capture(
                frame,
                [params.width, params.height],
                ViewMode::Lit,
                (params.marks > 0).then_some(params.marks),
                self.seed,
            )?;
            capture.write(std::path::Path::new(path))?;
            marks = capture
                .legend
                .iter()
                .map(|m| Mark {
                    mark: m.mark,
                    label: m.name.clone(),
                    bbox: m.bbox,
                })
                .collect();
        }
        Ok(Captured {
            tick: self.runtime.tick(),
            hash: self.hash()?,
            entities: self.entities(),
            image: params.path.clone(),
            marks,
        })
    }

    /// `play.recording`: finishes the active recording into a file.
    pub fn save_replay(&mut self, path: &str) -> Result<Value> {
        let replay = self.runtime.play_mut().finish_recording()?;
        let bytes = replay.to_bytes()?;
        std::fs::write(path, &bytes)?;
        Ok(json!({"path": path, "ticks": replay.ticks.len(), "bytes": bytes.len()}))
    }

    /// `play.replay`: verifies a replay file tick by tick from its initial state.
    pub fn replay(&mut self, path: &str) -> Result<Value> {
        let replay = ara_sim::Replay::from_bytes(&std::fs::read(path)?)?;
        let hash = self.runtime.play_mut().replay(&replay)?;
        self.mark();
        Ok(
            json!({"tick": self.runtime.tick(), "ticks": replay.ticks.len(), "hash": hash.to_string(), "verified": true}),
        )
    }

    /// `play.assets`: pins new generations and reloads them at this boundary.
    pub fn assets(&mut self, pins: &BTreeMap<String, String>) -> Result<Value> {
        for (id, digest) in pins {
            self.pins
                .insert(id.clone(), Digest(unhex(digest).map_err(|e| anyhow!(e))?));
        }
        let results = self
            .runtime
            .reload_assets(Box::new(assets(&self.cache, &self.pins)?))?;
        Ok(json!(
            results
                .into_iter()
                .map(|(id, r)| match r {
                    Ok(reloaded) => json!({"asset": id, "reloaded": reloaded}),
                    Err(e) => json!({"asset": id, "error": e, "kept": "lastGood"}),
                })
                .collect::<Vec<_>>()
        ))
    }
}

fn samples(samples: &[ara_sim::Sample]) -> Vec<Sample> {
    samples
        .iter()
        .map(|s| Sample {
            tick: s.tick,
            path: format!("{}.{}", s.watch.entity, s.watch.path),
            value: s.value.clone(),
        })
        .collect()
}

fn action(value: &Value) -> Result<ActionValue> {
    Ok(match value {
        Value::Bool(b) => ActionValue::Button(*b),
        Value::Number(n) => ActionValue::Axis(n.as_f64().context("axis value")? as f32),
        Value::Array(v) if v.len() == 2 => ActionValue::Axis2([
            v[0].as_f64().context("axis x")? as f32,
            v[1].as_f64().context("axis y")? as f32,
        ]),
        other => bail!("unsupported action value {other}"),
    })
}

fn injection(input: &Input) -> Result<Injection> {
    Ok(Injection {
        player: PlayerId(input.player),
        action: input.action.clone(),
        value: action(&input.value)?,
        hold_ticks: input.hold_ticks,
    })
}

fn step_inputs(inputs: &[Input]) -> Result<Vec<StepInput>> {
    inputs
        .iter()
        .map(|i| {
            Ok(StepInput {
                after_ticks: i.after_ticks,
                input: injection(i)?,
            })
        })
        .collect()
}

/// Reads `entity.Component.field…` (any registered component); vector fields accept
/// `x/y/z/w`, `r/g/b/a` or numeric indices. Missing entities/components read as `None`.
pub fn read_path(session: &ara_sim::Session, path: &str) -> Result<Option<Value>> {
    let mut parts = path.split('.');
    let entity = parts.next().context("empty path")?;
    let component = parts
        .next()
        .with_context(|| format!("`{path}` needs entity.Component"))?;
    if session.entity(entity).is_none() {
        return Ok(None);
    }
    let Some(mut current) = session.component(entity, component)? else {
        return Ok(None);
    };
    for part in parts {
        let next = match &current {
            Value::Object(map) => map.get(part).cloned(),
            Value::Array(items) => {
                let index = match part {
                    "x" | "r" => Some(0),
                    "y" | "g" => Some(1),
                    "z" | "b" => Some(2),
                    "w" | "a" => Some(3),
                    other => other.parse().ok(),
                };
                index.and_then(|i: usize| items.get(i).cloned())
            }
            _ => None,
        };
        match next {
            Some(v) => current = v,
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}
