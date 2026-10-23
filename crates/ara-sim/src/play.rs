use crate::{
    Clock, FrameAdvance, Replay, Result, Session, SimError, Snapshot, StateHash, TickRecord,
};
use ara_input::Injection;
use ara_schema::{FieldPath, path};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Duration;

/// Host control state; pausing never changes fixed simulation dt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    /// Initialized or explicitly stopped; step requests are rejected.
    Stopped,
    /// Advances from host time or explicit steps.
    Running,
    /// Host time is frozen; explicit steps remain available.
    Paused,
}

/// One action injection relative to the start of a step request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepInput {
    /// Offset zero applies at the very next boundary.
    pub after_ticks: u32,
    /// Action-level input with a hold duration.
    pub input: Injection,
}

/// A stable entity address and reflected field/component path to sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    /// Document or runtime entity identity.
    pub entity: String,
    /// Component/field path, such as Transform.position.y.
    pub path: FieldPath,
}

impl Watch {
    /// Parses e_4f9a.Transform.position.y (prefab addresses are accepted).
    pub fn parse(text: &str) -> Result<Self> {
        let (entity, path) = text
            .split_once('.')
            .ok_or_else(|| SimError("watch must contain an entity and component path".into()))?;
        Ok(Self {
            entity: entity.into(),
            path: path.parse().map_err(SimError)?,
        })
    }
}

/// One post-tick watch value; absent values represent a missing entity/component.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Tick after which this value was sampled.
    pub tick: u64,
    /// Watched address/path.
    pub watch: Watch,
    /// Effective value, including schema defaults.
    pub value: Option<Value>,
}

/// Bounded play/test result suitable for an Engine Protocol wrapper.
#[derive(Debug, Clone, PartialEq)]
pub struct StepReport {
    /// Number of ticks actually executed by this request.
    pub ticks: u32,
    /// Final absolute tick.
    pub tick: u64,
    /// Whether an until predicate was satisfied (including before the first tick).
    pub reached: bool,
    /// Samples in tick order, then watch declaration order.
    pub samples: Vec<Sample>,
}

/// Rust engine-side play/test API; independent of ara-proto and process transport.
pub struct PlayControl {
    session: Session,
    state: PlayState,
    build: StateHash,
    watches: Vec<Watch>,
    sample_every: u32,
    recording: Option<Replay>,
}

impl PlayControl {
    /// Owns an initialized Local session. Construction does not serialize the world.
    pub fn new(session: Session, build: StateHash) -> Self {
        Self {
            session,
            state: PlayState::Stopped,
            build,
            watches: Vec::new(),
            sample_every: 1,
            recording: None,
        }
    }
    /// Current host play state.
    pub fn state(&self) -> PlayState {
        self.state
    }
    /// Read-only session/world access.
    pub fn session(&self) -> &Session {
        &self.session
    }
    /// Configuration and future-boundary submission access.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Starts the initialized world. Use a fresh session/restore for a clean scene.
    pub fn start(&mut self) {
        self.state = PlayState::Running;
        self.session.fixed.rebase_clock();
    }
    /// Stops automatic/explicit stepping; the initialized world remains inspectable.
    pub fn stop(&mut self) {
        self.state = PlayState::Stopped;
        self.session.fixed.rebase_clock();
    }
    /// Freezes host-time stepping while permitting explicit test steps.
    pub fn pause(&mut self) -> Result<()> {
        self.active()?;
        self.state = PlayState::Paused;
        self.session.fixed.rebase_clock();
        Ok(())
    }
    /// Resumes host-time stepping without catching up paused wall time.
    pub fn resume(&mut self) -> Result<()> {
        self.active()?;
        self.state = PlayState::Running;
        self.session.fixed.rebase_clock();
        Ok(())
    }
    /// Sets host-time speed while preserving the fixed dt.
    pub fn timescale(&mut self, value: f64) -> Result<()> {
        self.session.set_timescale(value)
    }

    /// Replaces watched paths and sampling cadence. Paths validate before mutation.
    pub fn watch(&mut self, watches: Vec<Watch>, sample_every: u32) -> Result<()> {
        if sample_every == 0 {
            return Err(SimError("watch sampling interval must be positive".into()));
        }
        let record = self
            .session
            .edit
            .registry
            .schema
            .get("Entity")
            .ok_or_else(|| SimError("Entity schema is not registered".into()))?;
        for watch in &watches {
            self.session
                .edit
                .registry
                .schema
                .resolve_path(record, &watch.path)
                .map_err(|d| SimError(d.message))?;
        }
        self.watches = watches;
        self.sample_every = sample_every;
        Ok(())
    }

    /// Executes exact fixed ticks with action-level injection at relative offsets.
    pub fn step(&mut self, ticks: u32, inputs: &[StepInput]) -> Result<StepReport> {
        self.step_until(ticks, inputs, |_| false)
    }

    /// Runs until a read-only predicate succeeds, bounded by max_ticks.
    pub fn until(
        &mut self,
        max_ticks: u32,
        predicate: impl FnMut(&Session) -> bool,
    ) -> Result<StepReport> {
        self.step_until(max_ticks, &[], predicate)
    }

    /// Combines relative input scheduling, watch sampling and a bounded predicate.
    pub fn step_until(
        &mut self,
        max_ticks: u32,
        inputs: &[StepInput],
        mut predicate: impl FnMut(&Session) -> bool,
    ) -> Result<StepReport> {
        self.active()?;
        let start = self.session.tick();
        if start.checked_add(u64::from(max_ticks)).is_none() {
            return Err(SimError("step tick overflow".into()));
        }
        for input in inputs {
            if input.after_ticks >= max_ticks {
                return Err(SimError(
                    "input offset lies outside the step request".into(),
                ));
            }
            self.session
                .actions
                .validate(&input.input.action, input.input.value)?;
            if input.input.hold_ticks == 0
                || (start + 1 + u64::from(input.after_ticks))
                    .checked_add(u64::from(input.input.hold_ticks))
                    .is_none()
            {
                return Err(SimError("invalid input duration".into()));
            }
        }
        for input in inputs {
            self.session.inject(
                start + 1 + u64::from(input.after_ticks),
                vec![input.input.clone()],
            )?;
        }
        let mut report = StepReport {
            ticks: 0,
            tick: start,
            reached: predicate(&self.session),
            samples: Vec::new(),
        };
        while report.ticks < max_ticks && !report.reached {
            self.tick()?;
            report.ticks += 1;
            report.tick = self.session.tick();
            if report.tick.is_multiple_of(u64::from(self.sample_every)) {
                self.sample(&mut report.samples)?;
            }
            report.reached = predicate(&self.session);
        }
        Ok(report)
    }

    /// Bounded automatic advancement, recording exactly the same boundaries as step.
    pub fn advance(&mut self, elapsed: Duration) -> Result<FrameAdvance> {
        if self.state != PlayState::Running {
            return Ok(self.frozen());
        }
        let frame = self.session.fixed.advance(elapsed);
        for _ in 0..frame.ticks {
            self.tick()?;
        }
        Ok(frame)
    }

    /// Uses an injected monotonic host clock; pauses rebase it automatically.
    pub fn poll(&mut self, clock: &impl Clock) -> Result<FrameAdvance> {
        if self.state != PlayState::Running {
            return Ok(self.frozen());
        }
        let frame = self.session.fixed.poll(clock)?;
        for _ in 0..frame.ticks {
            self.tick()?;
        }
        Ok(frame)
    }

    /// Complete registered-state checkpoint, including queued inputs/commands.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.session.snapshot(self.build)
    }
    /// Snapshot plus exact in-process provider checkpoints (bit-exact native rollback).
    pub fn checkpoint(&mut self) -> Result<Snapshot> {
        self.session.checkpoint(self.build)
    }
    /// Build/module digest that snapshots and replays must match.
    pub fn build(&self) -> StateHash {
        self.build
    }
    /// Restores a matching checkpoint; an active recording must first be finished.
    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<()> {
        if self.recording.is_some() {
            return Err(SimError(
                "finish recording before restoring a checkpoint".into(),
            ));
        }
        self.session.restore(snapshot, self.build)
    }

    /// Begins optional history with an initial snapshot and per-tick state hashes.
    pub fn record(&mut self) -> Result<()> {
        if self.recording.is_some() {
            return Err(SimError("already recording".into()));
        }
        let initial = self.checkpoint()?;
        self.recording = Some(Replay {
            format: 1,
            scene_hash: initial.scene_hash,
            seed: initial.context.seed,
            build_hash: self.build,
            schema_hash: initial.schema_hash,
            initial,
            ticks: Vec::new(),
        });
        self.session.record_hashes = true;
        Ok(())
    }

    /// Finishes a record/replay artifact, leaving hashing disabled for normal Local play.
    pub fn finish_recording(&mut self) -> Result<Replay> {
        let replay = self
            .recording
            .take()
            .ok_or_else(|| SimError("not recording".into()))?;
        self.session.record_hashes = false;
        Ok(replay)
    }

    /// Restores a replay's initial state and verifies every tick, failing at the first
    /// divergence. Exogenous future queues are rebuilt from the applied log once.
    pub fn replay(&mut self, replay: &Replay) -> Result<StateHash> {
        if self.recording.is_some() {
            return Err(SimError("finish recording before replay".into()));
        }
        if replay.format != 1
            || replay.build_hash != self.build
            || replay.schema_hash != self.session.schema_hash()
            || replay.scene_hash != replay.initial.scene_hash
            || replay.seed != replay.initial.context.seed
        {
            return Err(SimError(
                "replay format/scene/seed/build/schema mismatch".into(),
            ));
        }
        self.session.restore(&replay.initial, self.build)?;
        self.session.pending_inputs.clear();
        self.session.pending_commands.clear();
        self.session.pending_typed.clear();
        self.session.pending_ops.clear();
        let was_hashing = self.session.record_hashes;
        self.session.record_hashes = true;
        let result = (|| {
            for expected in &replay.ticks {
                if expected.tick != self.session.tick() + 1 || expected.hash.is_none() {
                    return Err(SimError("replay has a gap or missing tick hash".into()));
                }
                self.session
                    .inject(expected.tick, expected.inputs.clone())?;
                for command in &expected.commands {
                    self.session.submit(expected.tick, command.clone())?;
                }
                self.session
                    .pending_ops
                    .insert(expected.tick, expected.ops.clone());
                let actual = self.session.step_with_streaming(|boundary| {
                    for change in &expected.streaming {
                        boundary.replay(change.clone())?;
                    }
                    Ok(())
                })?;
                if actual.hash != expected.hash {
                    return Err(SimError(format!(
                        "replay state mismatch at tick {}: expected {:?}, got {:?}",
                        expected.tick, expected.hash, actual.hash
                    )));
                }
            }
            self.session.state_hash()
        })();
        self.session.record_hashes = was_hashing;
        result
    }

    fn tick(&mut self) -> Result<TickRecord> {
        let record = self.session.step()?;
        if let Some(replay) = &mut self.recording {
            replay.ticks.push(record.clone());
        }
        Ok(record)
    }

    fn active(&self) -> Result<()> {
        if self.state == PlayState::Stopped {
            return Err(SimError("play session is stopped".into()));
        }
        Ok(())
    }

    fn frozen(&self) -> FrameAdvance {
        FrameAdvance {
            ticks: 0,
            alpha: self.session.alpha(),
            dropped_ticks: 0,
        }
    }

    fn sample(&self, samples: &mut Vec<Sample>) -> Result<()> {
        let schema = &self.session.edit.registry.schema;
        let rt = schema
            .get("Entity")
            .ok_or_else(|| SimError("Entity schema missing".into()))?;
        for watch in &self.watches {
            let entity = self.session.world().iter_entities().find(|e| {
                e.get::<crate::Identity>()
                    .is_some_and(|id| id.0 == watch.entity)
            });
            let value = if let Some(entity) = entity {
                let resolved = schema
                    .resolve_path(rt, &watch.path)
                    .map_err(|d| SimError(d.message))?;
                let key = match resolved.steps.first() {
                    Some(ara_schema::Step::Key { key, .. }) => key.as_str(),
                    _ => watch.path.head(),
                };
                let metadata = entity.get::<crate::SceneMetadata>();
                let component = match key {
                    "name" => Some(Value::from(metadata.map_or("", |meta| meta.name.as_str()))),
                    "order" => Some(Value::from(metadata.map_or("", |meta| meta.order.as_str()))),
                    "parent" => Some(
                        entity
                            .get::<bevy_ecs::prelude::ChildOf>()
                            .and_then(|parent| {
                                self.session.world().get::<crate::Identity>(parent.parent())
                            })
                            .map_or(Value::Null, |id| Value::from(id.0.as_str())),
                    ),
                    name if ara_doc::is_component_key(name) => self.session.edit.registry.read(
                        self.session.world(),
                        &self.session.edit.ids,
                        entity.id(),
                        name,
                    )?,
                    _ => None,
                };
                if let Some(component) = component {
                    let record: Map<String, Value> =
                        [(key.into(), component)].into_iter().collect();
                    path::get_effective(&record, &resolved.steps)
                } else {
                    None
                }
            } else {
                None
            };
            samples.push(Sample {
                tick: self.session.tick(),
                watch: watch.clone(),
                value,
            });
        }
        Ok(())
    }
}
