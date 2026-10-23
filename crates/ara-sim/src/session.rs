use crate::{
    Clock, EditWorld, FixedTick, FrameAdvance, Result, SimContext, SimError, StateHash, TickConfig,
};
use ara_doc::Op;
use ara_input::{ActionMap, Injection, PlayerId};
use bevy_ecs::{
    prelude::*,
    schedule::{ScheduleBuildSettings, SingleThreadedExecutor},
    system::IntoSystem,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

/// Authority topology. Only Local needs no adapter or serialization machinery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    /// One authority world, direct input and command dispatch.
    #[default]
    Local,
    /// Authority world with local and remote owners (optional ara-net adapter).
    Listen,
    /// Headless authority world (optional ara-net adapter).
    Dedicated,
}

/// A typed command envelope. Payloads deserialize through the startup registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundaryCommand {
    /// Registry command name.
    pub kind: String,
    /// Explicit authenticated owner, assigned by the adapter for remote callers.
    pub player: PlayerId,
    /// Monotonic per-owner identity, for replay/idempotent delivery.
    pub sequence: u64,
    /// Stable target address, if the command requires one.
    pub target: Option<String>,
    /// Schema-defined command parameters or asynchronous result data.
    pub payload: Value,
}

/// The data actually applied at one boundary, with its resulting state fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickRecord {
    /// Absolute tick, starting at one.
    pub tick: u64,
    /// Device-independent applied actions in submission order.
    pub inputs: Vec<Injection>,
    /// Encoded commands in player/sequence order. Direct typed Local commands are
    /// included only when hash/history sampling is enabled.
    pub commands: Vec<BoundaryCommand>,
    /// Committed authoring ops applied before gameplay systems.
    pub ops: Vec<Op>,
    /// Host streaming arrivals actually applied before this tick's systems.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streaming: Vec<crate::StreamChange>,
    /// Hash after this tick; absent when history/hash sampling is disabled.
    pub hash: Option<StateHash>,
}

/// M2 integration seam: I/O queues feed boundaries and receive read-only checkpoints.
/// An ara-net implementation owns codecs, transport and permissions outside Local.
pub trait SessionAdapter {
    /// Adapter authority topology.
    fn mode(&self) -> SessionMode;
    /// Drain authenticated arrivals into the session before the next boundary.
    fn before_tick(&mut self, session: &mut Session) -> Result<()>;
    /// Observe a completed tick without allowing presentation to mutate gameplay.
    fn after_tick(&mut self, world: &World, record: &TickRecord) -> Result<()>;
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Order(usize);

#[cfg(feature = "profiling")]
struct ProfileSystem(tracing::Span);

#[cfg(feature = "profiling")]
impl<S: bevy_ecs::system::System<In = (), Out = ()>> bevy_ecs::system::Adapt<S> for ProfileSystem {
    type In = ();
    type Out = ();

    fn adapt(
        &mut self,
        input: (),
        run: impl FnOnce(
            bevy_ecs::system::SystemIn<'_, S>,
        ) -> std::result::Result<(), bevy_ecs::system::RunSystemError>,
    ) -> std::result::Result<(), bevy_ecs::system::RunSystemError> {
        let _span = self.0.enter();
        run(input)
    }
}

/// One independent authority world and an explicitly ordered fixed schedule.
/// No transport, replica world, global current player, or automatic state serialization.
pub struct Session {
    pub(crate) edit: EditWorld,
    schedule: Schedule,
    pub(crate) systems: Vec<String>,
    pub(crate) fixed: FixedTick,
    pub(crate) actions: ActionMap,
    pub(crate) pending_inputs: BTreeMap<u64, Vec<Injection>>,
    pub(crate) pending_commands: BTreeMap<u64, Vec<BoundaryCommand>>,
    pub(crate) pending_typed: BTreeMap<u64, Vec<Box<dyn crate::command::QueuedTyped>>>,
    pub(crate) pending_ops: BTreeMap<u64, Vec<Op>>,
    pub(crate) sequences: BTreeMap<PlayerId, u64>,
    pub(crate) mode: SessionMode,
    pub(crate) record_hashes: bool,
    pub(crate) fault: Option<SimError>,
}

impl Session {
    /// Starts a Local simulation from a projected scene, without taking a snapshot.
    pub fn local(
        mut edit: EditWorld,
        config: TickConfig,
        seed: u64,
        actions: ActionMap,
    ) -> Result<Self> {
        let fixed = FixedTick::new(config)?;
        crate::initialize_compute_pool(
            std::thread::available_parallelism().unwrap_or(std::num::NonZeroUsize::MIN),
        );
        edit.world.insert_resource(SimContext::new(seed, config.hz));
        let mut schedule = Schedule::default();
        schedule.set_executor(SingleThreadedExecutor::new());
        schedule.set_build_settings(ScheduleBuildSettings {
            auto_insert_apply_deferred: false,
            ..Default::default()
        });
        Ok(Self {
            edit,
            schedule,
            systems: Vec::new(),
            fixed,
            actions,
            pending_inputs: BTreeMap::new(),
            pending_commands: BTreeMap::new(),
            pending_typed: BTreeMap::new(),
            pending_ops: BTreeMap::new(),
            sequences: BTreeMap::new(),
            mode: SessionMode::Local,
            record_hashes: false,
            fault: None,
        })
    }

    /// Adds one system in call order. Deferred structural writes flush at tick end,
    /// so later systems observe spawns/despawns at the next tick, not halfway through.
    /// Future-affecting state must live in registered components/context, never Local
    /// or captured variables. Added/Changed filters are presentation optimizations;
    /// authoritative decisions use explicit state/tick components so restore is complete.
    pub fn add_system<M>(&mut self, name: &str, system: impl IntoSystem<(), (), M>) -> Result<()> {
        if name.is_empty() || self.systems.iter().any(|s| s == name) {
            return Err(SimError("empty or duplicate system identity".into()));
        }
        let index = self.systems.len();
        if index > 0 {
            self.schedule
                .configure_sets(Order(index).after(Order(index - 1)));
        }
        let system = IntoSystem::into_system(system);
        #[cfg(feature = "profiling")]
        let system = bevy_ecs::system::AdapterSystem::new(
            ProfileSystem(tracing::info_span!(target: "ara_profile", "system", name)),
            system,
            name.to_owned().into(),
        );
        self.schedule.add_systems(system.in_set(Order(index)));
        self.systems.push(name.into());
        Ok(())
    }

    /// Authority topology (Local until an optional adapter drives it).
    pub fn mode(&self) -> SessionMode {
        self.mode
    }
    /// Read-only world for presentation and inspection.
    pub fn world(&self) -> &World {
        &self.edit.world
    }
    /// Setup/tick-system access. External arrivals use queues instead of borrowing it.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.edit.world
    }
    /// Explicit simulation time, inputs and seeded random streams.
    pub fn context(&self) -> &SimContext {
        self.edit.world.resource::<SimContext>()
    }
    /// Completed ticks.
    pub fn tick(&self) -> u64 {
        self.context().tick
    }
    /// Immutable projection/mapping metadata.
    pub fn projection(&self) -> &EditWorld {
        &self.edit
    }
    /// Complete startup schema shared by runtime codecs and optional adapters.
    pub fn schema(&self) -> &ara_schema::SchemaDb {
        self.edit.registry.schema()
    }
    /// Reads one registered component through a tick-stable entity handle.
    /// Adapters resolve identities once per boundary instead of rescanning per field.
    pub fn component_at(&self, entity: Entity, name: &str) -> Result<Option<Value>> {
        self.edit
            .registry
            .read(self.world(), &self.edit.ids, entity, name)
    }
    /// Resolves a stable authored or runtime identity, including spawned test entities.
    pub fn entity(&self, identity: &str) -> Option<Entity> {
        self.world()
            .iter_entities()
            .find(|entity| {
                entity
                    .get::<crate::Identity>()
                    .is_some_and(|id| id.0 == identity)
            })
            .map(|entity| entity.id())
    }

    /// Reads reflected runtime state without serializing an entire world.
    pub fn component(&self, identity: &str, name: &str) -> Result<Option<Value>> {
        let entity = self
            .entity(identity)
            .ok_or_else(|| SimError(format!("unknown entity {identity}")))?;
        self.component_at(entity, name)
    }

    /// Writes registered gameplay data during setup or a controlled simulation boundary.
    /// This never edits an authoring document; live authoring changes use patch instead.
    pub fn write_runtime_component(
        &mut self,
        identity: &str,
        name: &str,
        value: &Value,
    ) -> Result<()> {
        let entity = self
            .entity(identity)
            .ok_or_else(|| SimError(format!("unknown entity {identity}")))?;
        self.edit
            .registry
            .write(&mut self.edit.world, &self.edit.ids, entity, name, value)
    }
    /// Enables optional per-tick state hashing for recording/tests/desync checks.
    pub fn set_hash_sampling(&mut self, enabled: bool) {
        self.record_hashes = enabled;
    }
    /// Adjusts host-time speed; explicit step calls always execute exact fixed ticks.
    pub fn set_timescale(&mut self, value: f64) -> Result<()> {
        self.fixed.set_timescale(value)
    }
    /// Current render interpolation phase.
    pub fn alpha(&self) -> f64 {
        self.fixed.alpha()
    }

    /// Validates and queues actions for a future absolute tick.
    pub fn inject(&mut self, tick: u64, inputs: Vec<Injection>) -> Result<()> {
        self.future(tick)?;
        for input in &inputs {
            self.actions.validate(&input.action, input.value)?;
            if input.hold_ticks == 0 || tick.checked_add(u64::from(input.hold_ticks)).is_none() {
                return Err(SimError("invalid input duration".into()));
            }
        }
        self.pending_inputs.entry(tick).or_default().extend(inputs);
        Ok(())
    }

    /// Queues a typed intent/async result. It cannot execute before its tick boundary.
    pub fn submit(&mut self, tick: u64, mut command: BoundaryCommand) -> Result<()> {
        self.future(tick)?;
        self.edit.registry.prepare_command(&mut command)?;
        self.pending_commands.entry(tick).or_default().push(command);
        Ok(())
    }

    /// Queues registered typed data directly in Local mode. No serialization occurs
    /// unless recording or a snapshot explicitly asks for its portable representation.
    pub fn submit_typed<C: crate::SimCommand>(
        &mut self,
        tick: u64,
        context: crate::CommandContext,
        value: C,
    ) -> Result<()> {
        self.future(tick)?;
        let registered = self
            .edit
            .registry
            .handlers
            .get(C::NAME)
            .and_then(|codec| codec.direct);
        if registered != Some(std::any::TypeId::of::<C>()) {
            return Err(SimError(format!(
                "{} is not registered for direct typed dispatch",
                C::NAME
            )));
        }
        value.validate(&context)?;
        self.pending_typed
            .entry(tick)
            .or_default()
            .push(Box::new(crate::command::Typed { context, value }));
        Ok(())
    }

    /// Queues a committed authoring transaction for the edit/play bridge.
    /// This administrative path must never be exposed as a public gameplay RPC.
    pub fn patch(&mut self, tick: u64, transaction: &ara_ops::Transaction) -> Result<()> {
        self.future(tick)?;
        self.pending_ops
            .entry(tick)
            .or_default()
            .extend(transaction.ops.iter().cloned());
        Ok(())
    }

    fn future(&self, tick: u64) -> Result<()> {
        if tick <= self.tick() {
            return Err(SimError("input/command tick must be in the future".into()));
        }
        Ok(())
    }

    /// Executes exactly one fixed boundary, independently of a host clock.
    /// A handler/patch failure faults the session; restore a checkpoint before continuing.
    pub fn step(&mut self) -> Result<TickRecord> {
        self.step_with_streaming(|_| Ok(()))
    }

    /// Runs bounded host activation at a fixed boundary, before gameplay systems.
    /// The host owns timing decisions; recorded changes make replay independent of
    /// worker completion and frame timing. Errors fault the session like any command.
    pub fn step_with_streaming(
        &mut self,
        activate: impl FnOnce(&mut crate::StreamingBoundary<'_>) -> Result<()>,
    ) -> Result<TickRecord> {
        #[cfg(feature = "profiling")]
        ara_profile::scope!("sim.tick");
        if let Some(error) = &self.fault {
            return Err(SimError(format!(
                "session is faulted; restore a checkpoint: {error}"
            )));
        }
        let result = self.step_inner(activate);
        if let Err(error) = &result {
            self.fault = Some(error.clone());
        }
        result
    }

    fn step_inner(
        &mut self,
        activate: impl FnOnce(&mut crate::StreamingBoundary<'_>) -> Result<()>,
    ) -> Result<TickRecord> {
        let tick = self
            .tick()
            .checked_add(1)
            .ok_or_else(|| SimError("simulation tick overflow".into()))?;
        let inputs = self.pending_inputs.remove(&tick).unwrap_or_default();
        let mut pending: Vec<_> = self
            .pending_commands
            .remove(&tick)
            .unwrap_or_default()
            .into_iter()
            .map(crate::command::Pending::Data)
            .collect();
        pending.extend(
            self.pending_typed
                .remove(&tick)
                .unwrap_or_default()
                .into_iter()
                .map(crate::command::Pending::Typed),
        );
        let ops = self.pending_ops.remove(&tick).unwrap_or_default();
        pending.sort_by_key(crate::command::Pending::key);
        // Duplicate/old intents are ignored before calling gameplay, including retransmits.
        pending.retain(|c| {
            let (player, sequence) = c.key();
            if self
                .sequences
                .get(&player)
                .is_some_and(|last| *last >= sequence)
            {
                return false;
            }
            self.sequences.insert(player, sequence);
            true
        });
        self.edit.apply_ops(&ops)?;
        let mut streaming = Vec::new();
        activate(&mut crate::StreamingBoundary {
            edit: &mut self.edit,
            changes: &mut streaming,
            recording: self.record_hashes,
        })?;
        {
            let mut context = self.edit.world.resource_mut::<SimContext>();
            context.inputs.apply(&self.actions, tick, &inputs)?;
            context.tick = tick;
        }
        let mut commands = Vec::new();
        for command in pending {
            match command {
                crate::command::Pending::Data(command) => {
                    let handler =
                        self.edit
                            .registry
                            .handlers
                            .get(&command.kind)
                            .ok_or_else(|| {
                                SimError("boundary command registration disappeared".into())
                            })?;
                    (handler.run)(&mut self.edit.world, &command)?;
                    commands.push(command);
                }
                crate::command::Pending::Typed(command) => {
                    if self.record_hashes {
                        let mut record = command.record()?;
                        self.edit.registry.prepare_command(&mut record)?;
                        commands.push(record);
                    }
                    command.execute(&mut self.edit.world)?;
                }
            }
        }
        self.schedule.run(&mut self.edit.world);
        let hash = self.record_hashes.then(|| self.state_hash()).transpose()?;
        Ok(TickRecord {
            tick,
            inputs,
            commands,
            ops,
            streaming,
            hash,
        })
    }

    /// Drives the same step through an optional external networking adapter.
    pub fn step_with_adapter(&mut self, adapter: &mut impl SessionAdapter) -> Result<TickRecord> {
        self.mode = adapter.mode();
        adapter.before_tick(self)?;
        let record = self.step()?;
        adapter.after_tick(self.world(), &record)?;
        Ok(record)
    }

    /// Runs bounded catch-up from an explicitly supplied host duration.
    pub fn advance(&mut self, elapsed: Duration) -> Result<FrameAdvance> {
        let frame = self.fixed.advance(elapsed);
        for _ in 0..frame.ticks {
            self.step()?;
        }
        Ok(frame)
    }

    /// Runs bounded catch-up from an injected host clock.
    pub fn poll(&mut self, clock: &impl Clock) -> Result<FrameAdvance> {
        let frame = self.fixed.poll(clock)?;
        for _ in 0..frame.ticks {
            self.step()?;
        }
        Ok(frame)
    }
}
