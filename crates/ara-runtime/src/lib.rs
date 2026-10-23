//! Ara's playable runtime: one place that composes the deterministic session
//! (`ara-sim`), input actions, Jolt physics, ara-anim animation, Recast navigation and
//! cooked assets into a world that plays a scene, plus a renderer-neutral
//! presentation extraction.
//!
//! # Fixed-tick system order
//!
//! | # | System | Does |
//! |---|---|---|
//! | 0 | `poses.remember` | last world poses become the interpolation start |
//! | 1 | `input.cameras` | `Look` steers follow-camera rigs |
//! | 2 | `input.players` | `Move`/`Sprint`/`Jump` → character input, facing |
//! | 3 | plugins [`Stage::Gameplay`] | C# script systems |
//! | 4 | `ai.wander` | wanderers choose navmesh goals (session RNG) |
//! | 5 | `navigation` | Detour crowd step (exclusive) |
//! | 6 | `animation` ∥ `navigation.follow` | animator tick ∥ agent poses (parallel group) |
//! | 7 | `animation.root` | root motion moves free animated entities |
//! | 8 | `physics.push` | edited bodies teleport to their `Transform` |
//! | 9 | `physics` | Jolt boundary: extract → step → mirror (exclusive) |
//! | 10 | `physics.pull` | moving bodies/characters → `Transform` |
//! | 11 | `camera.place` | rigs placed behind targets |
//! | 12 | plugins [`Stage::Late`] | replication, audio/VFX emission |
//! | 13 | `poses.resolve` | world poses for presentation (exclusive) |
//!
//! Presentation runs between ticks: [`Runtime::extract`] interpolates world poses with
//! the fixed-tick phase and never mutates the world.
#![warn(missing_docs)]

pub mod animation;
pub mod assets;
pub mod components;
pub mod control;
pub mod controller;
pub mod engine;
pub mod extract;
pub mod live;
pub mod math;
pub mod model;
pub mod navigation;
pub mod physics;
pub mod plugin;
pub mod skin;
pub mod sync;

use crate::{
    animation::{AnimationProvider, AnimatorSlot, Library},
    assets::{AssetSource, asset_id},
    components::{
        Animator, Camera, Environment, FollowCamera, Light, MeshRenderer, PlayerControl, Wanderer,
    },
    extract::RenderFrame,
    math::Pose,
    plugin::{RuntimePlugin, Stage},
};
use ara_doc::{AssetId, Op, Workspace};
use ara_input::Injection;
use ara_physics::{CharacterInput, PhysicsConfig, PhysicsStatus};
use ara_schema::SchemaDb;
use ara_sim::{
    EditWorld, FixedTick, FrameAdvance, PlayControl, PlayState, Sample, Session, SimError,
    SimRegistry, StateHash, StepInput, StepReport, TickConfig,
};
use bevy_ecs::{
    prelude::*,
    system::{BoxedSystem, RunSystemOnce},
};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

pub use ara_sim;

/// A runtime setup, asset or simulation failure with a readable description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError(pub String);

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RuntimeError {}
impl From<SimError> for RuntimeError {
    fn from(e: SimError) -> Self {
        Self(e.0)
    }
}
impl From<String> for RuntimeError {
    fn from(e: String) -> Self {
        Self(e)
    }
}
impl From<serde_json::Error> for RuntimeError {
    fn from(e: serde_json::Error) -> Self {
        Self(e.to_string())
    }
}
impl From<ara_asset::Error> for RuntimeError {
    fn from(e: ara_asset::Error) -> Self {
        Self(e.to_string())
    }
}
impl From<ara_asset_format::FormatError> for RuntimeError {
    fn from(e: ara_asset_format::FormatError) -> Self {
        Self(e.to_string())
    }
}

/// Fallible runtime operation.
pub type Result<T> = std::result::Result<T, RuntimeError>;

/// How a runtime world is built.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Fixed rate and catch-up bound. The rate must divide 48 000 (animation timebase).
    pub tick: TickConfig,
    /// Session seed for every deterministic random stream.
    pub seed: u64,
    /// Digest of the binary and gameplay modules; snapshots and replays must match it.
    pub build: StateHash,
    /// Physics allocation and solver settings; `fixed_dt` is derived from `tick`.
    pub physics: PhysicsConfig,
    /// Content-addressed navmesh tile cache (for example `.ara/cache/nav`).
    pub nav_cache: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            tick: TickConfig::default(),
            seed: 0,
            build: StateHash::of_bytes(
                concat!("ara-runtime ", env!("CARGO_PKG_VERSION")).as_bytes(),
            ),
            physics: PhysicsConfig {
                max_bodies: 4096,
                max_pairs: 16384,
                max_contacts: 16384,
                event_capacity: 4096,
                scratch_mb: 16,
                ..Default::default()
            },
            nav_cache: None,
        }
    }
}

/// Builds the schema DB a runtime projects with: the engine schema world
/// ([`engine::register_schemas`]) and project-defined `*.types.json` documents.
pub fn schema(project_types: &[serde_json::Value]) -> Result<SchemaDb> {
    let mut db = SchemaDb::new();
    if let Some(problem) = engine::register_schemas(&mut db).into_iter().next() {
        return Err(RuntimeError(problem.message));
    }
    for types in project_types {
        if let Some(d) = db.add_json_types(types).into_iter().find(|d| d.is_error()) {
            return Err(RuntimeError(d.message));
        }
    }
    Ok(db)
}

fn register_components(registry: &mut SimRegistry) -> std::result::Result<(), SimError> {
    use ara_physics as p;
    registry.register_component::<p::PhysicsId>()?;
    registry.register_component::<p::PhysicsPose>()?;
    registry.register_component::<p::RigidBody>()?;
    registry.register_component::<p::Collider>()?;
    registry.register_component::<p::Velocity>()?;
    registry.register_component::<p::CharacterController>()?;
    registry.register_component::<p::CharacterInput>()?;
    registry.register_component::<p::CharacterMotion>()?;
    registry.register_component::<p::Vehicle>()?;
    registry.register_component::<p::VehicleInput>()?;
    registry.register_component::<p::VehicleTelemetry>()?;
    registry.register_component::<p::Joint>()?;
    registry.register_component::<ara_nav::NavAgent>()?;
    registry.register_component::<ara_nav::NavMeshSurface>()?;
    registry.register_component::<ara_nav::NavLink>()?;
    registry.register_component::<ara_nav::NavArea>()?;
    registry.register_component::<MeshRenderer>()?;
    registry.register_component::<Light>()?;
    registry.register_component::<Environment>()?;
    registry.register_component::<Camera>()?;
    registry.register_component::<FollowCamera>()?;
    registry.register_component::<PlayerControl>()?;
    registry.register_component::<Wanderer>()?;
    registry.register_component::<Animator>()?;
    registry.register_component::<components::VisualEffect>()?;
    // Presentation sound intent; playback cursors never enter the simulation.
    registry.register_component::<ara_audio::AudioSource>()?;
    registry.register_component::<ara_audio::AudioListener>()?;
    registry.register_component::<ara_acoustics::AcousticSource>()?;
    registry.register_component::<ara_acoustics::AcousticSurface>()?;
    registry.register_component::<ara_acoustics::ReverbZone>()?;
    registry.register_component::<ara_acoustics::AcousticPortal>()?;
    // 2D scenes: sprites, tilemaps, lights, cameras and Rapier2D bodies.
    ara_2d::register_sim(registry)?;
    ara_physics2d::register_sim(registry)?;
    Ok(())
}

/// Whether the scene has 2D bodies (2D physics is installed only then).
fn has_2d_physics(world: &mut World) -> bool {
    use ara_physics2d::{CharacterController2d, Collider2d, RigidBody2d};
    world
        .query_filtered::<(), Or<(
            With<RigidBody2d>,
            With<Collider2d>,
            With<CharacterController2d>,
        )>>()
        .iter(world)
        .next()
        .is_some()
}

fn boxed<M>(system: impl IntoSystem<(), (), M>) -> BoxedSystem {
    Box::new(IntoSystem::into_system(system))
}

/// A playable world: the fixed-tick session with every subsystem installed, a
/// host-time accumulator, plugins and the asset source.
pub struct Runtime {
    play: PlayControl,
    clock: FixedTick,
    plugins: Vec<Box<dyn RuntimePlugin>>,
    assets: Box<dyn AssetSource>,
    hz: u32,
    /// Authoring schema used to apply live edits and diff values.
    schema: SchemaDb,
    /// Documents as projected at tick 0, the "keep changes" baseline.
    baseline: Workspace,
    /// Mirror including patches queued for the next tick.
    pending: Option<Workspace>,
}

impl Runtime {
    /// Projects `scene` from `workspace`, loads every referenced model generation and
    /// builds the complete world at tick 0, paused. Fails with a structured message
    /// on schema, projection, asset, bake or physics errors.
    pub fn new(
        schema: SchemaDb,
        workspace: Workspace,
        scene: AssetId,
        assets: Box<dyn AssetSource>,
        mut config: RuntimeConfig,
        mut plugins: Vec<Box<dyn RuntimePlugin>>,
    ) -> Result<Self> {
        let hz = config.tick.hz;
        config.physics.fixed_dt = 1.0 / hz as f32;
        let baseline = workspace.clone();
        let mut schema = schema;
        let mut world = World::new();
        for plugin in &mut plugins {
            plugin.prepare(&mut world, &mut schema)?;
        }
        let mut registry = SimRegistry::new(schema.clone())?;
        register_components(&mut registry)?;
        for plugin in &mut plugins {
            plugin.register(&mut registry)?;
        }
        let edit = EditWorld::from_workspace_in(world, registry, workspace, scene)?;
        let mut session =
            Session::local(edit, config.tick, config.seed, control::default_actions())?;
        let world = session.world_mut();
        let mut library = Library::default();
        let mut models = BTreeSet::new();
        let mut query = world.query::<&MeshRenderer>();
        for renderer in query.iter(world) {
            if let Some(model) = &renderer.model {
                models.insert(asset_id(model)?.to_owned());
            }
        }
        for model in &models {
            library.models.load(assets.as_ref(), model)?;
        }
        library.documents = controller::Documents::load(&baseline)?;
        world.insert_resource(library);
        let mut players = Vec::new();
        let mut query = world
            .query_filtered::<(Entity, Has<ara_physics2d::CharacterController2d>), (
                With<PlayerControl>,
                Without<CharacterInput>,
                Without<ara_physics2d::CharacterInput2d>,
            )>();
        players.extend(query.iter(world));
        for (entity, two_d) in players {
            if two_d {
                world
                    .entity_mut(entity)
                    .insert(ara_physics2d::CharacterInput2d::default());
            } else {
                world.entity_mut(entity).insert(CharacterInput::default());
            }
        }
        let matrices = sync::world_matrices(world);
        let poses: Vec<(Entity, Pose)> = matrices
            .iter()
            .map(|(&e, &m)| (e, Pose::from_matrix(m)))
            .collect();
        physics::install(world, &config.physics, &poses)?;
        navigation::install(
            world,
            config.physics.fixed_dt,
            config.nav_cache.clone(),
            &poses.iter().copied().collect(),
        )?;
        animation::ensure_slots(world, hz)?;
        // Cameras start behind their targets rather than at their authored pose.
        world
            .run_system_once(control::place_cameras)
            .map_err(|e| RuntimeError(e.to_string()))?;
        sync::resolve_poses(world);

        session.add_provider(sync::PoseProvider)?;
        session.add_provider(physics::PhysicsProvider {
            config: config.physics.clone(),
        })?;
        session.add_provider(navigation::NavigationProvider)?;
        session.add_provider(AnimationProvider { hz })?;

        session.add_system("poses.remember", sync::remember_poses)?;
        session.add_system("input.cameras", control::steer_cameras)?;
        session.add_system("input.players", control::drive_players)?;
        session.add_system("input.players2d", control::drive_players_2d)?;
        for plugin in &mut plugins {
            plugin.install(Stage::Gameplay, &mut session)?;
        }
        session.add_system("ai.wander", navigation::wander)?;
        session.add_system("navigation", ara_nav::tick)?;
        session.add_parallel_systems(vec![
            ("animation", boxed(animation::animate)),
            ("navigation.follow", boxed(navigation::follow)),
        ])?;
        session.add_system("animation.root", sync::apply_root_motion)?;
        session.add_system("physics.push", sync::push_poses)?;
        session.add_system("physics", physics::step)?;
        session.add_system("physics.pull", sync::pull_poses)?;
        if has_2d_physics(session.world_mut()) {
            ara_physics2d::install(
                &mut session,
                ara_physics2d::Physics2dConfig {
                    tick_hz: hz,
                    ..Default::default()
                },
            )
            .map_err(|e| RuntimeError(format!("2D physics: {e:#}")))?;
        }
        session.add_system("camera.place", control::place_cameras)?;
        for plugin in &mut plugins {
            plugin.install(Stage::Late, &mut session)?;
        }
        session.add_system("poses.resolve", sync::resolve_poses)?;

        let mut play = PlayControl::new(session, config.build);
        play.start();
        play.pause()?;
        Ok(Self {
            play,
            clock: FixedTick::new(config.tick)?,
            plugins,
            assets,
            hz,
            schema,
            baseline,
            pending: None,
        })
    }

    /// The authoring schema this world was projected with.
    pub fn schema(&self) -> &SchemaDb {
        &self.schema
    }

    /// The documents as projected at tick 0 (scene, prefabs, animation, mixer, music).
    pub fn documents(&self) -> &Workspace {
        &self.baseline
    }

    /// Play control: watches, snapshots, record/replay and the session.
    pub fn play(&self) -> &PlayControl {
        &self.play
    }
    /// Mutable play control (watches, record, replay, restore).
    pub fn play_mut(&mut self) -> &mut PlayControl {
        &mut self.play
    }
    /// The simulation world, read-only.
    pub fn world(&self) -> &World {
        self.play.session().world()
    }
    /// Completed ticks.
    pub fn tick(&self) -> u64 {
        self.play.session().tick()
    }
    /// Fixed simulation rate.
    pub fn hz(&self) -> u32 {
        self.hz
    }
    /// Whether host time advances the simulation.
    pub fn running(&self) -> bool {
        self.play.state() == PlayState::Running
    }
    /// Starts advancing with host time.
    pub fn resume(&mut self) -> Result<()> {
        self.clock.rebase_clock();
        Ok(self.play.resume()?)
    }
    /// Freezes host-time advancement; explicit steps still work.
    pub fn pause(&mut self) -> Result<()> {
        Ok(self.play.pause()?)
    }
    /// Host-time speed multiplier in [0, 1000]; the fixed dt never changes.
    pub fn set_timescale(&mut self, value: f64) -> Result<()> {
        Ok(self.clock.set_timescale(value)?)
    }
    /// Interpolation phase toward the next tick.
    pub fn alpha(&self) -> f32 {
        if self.running() {
            self.clock.alpha() as f32
        } else {
            1.0
        }
    }

    /// Advances by host time (bounded catch-up) while running.
    pub fn advance(&mut self, elapsed: Duration) -> Result<FrameAdvance> {
        if !self.running() {
            return Ok(FrameAdvance {
                ticks: 0,
                alpha: 1.0,
                dropped_ticks: 0,
            });
        }
        let frame = self.clock.advance(elapsed);
        for _ in 0..frame.ticks {
            self.tick_once(&mut Vec::new())?;
        }
        Ok(frame)
    }

    /// Executes exactly `ticks` fixed ticks with action injections at relative offsets,
    /// sampling active watches. Works paused or running.
    pub fn step(&mut self, ticks: u32, inputs: &[StepInput]) -> Result<StepReport> {
        self.step_until(ticks, inputs, |_| false)
    }

    /// Steps until `predicate` holds (checked before the first tick), at most `max` ticks.
    pub fn step_until(
        &mut self,
        max: u32,
        inputs: &[StepInput],
        mut predicate: impl FnMut(&Session) -> bool,
    ) -> Result<StepReport> {
        let start = self.tick();
        for input in inputs {
            if input.after_ticks >= max.max(1) {
                return Err(RuntimeError(
                    "input offset lies outside the step request".into(),
                ));
            }
            self.play.session_mut().inject(
                start + 1 + u64::from(input.after_ticks),
                vec![input.input.clone()],
            )?;
        }
        let mut report = StepReport {
            ticks: 0,
            tick: start,
            reached: predicate(self.play.session()),
            samples: Vec::new(),
        };
        while report.ticks < max && !report.reached {
            self.tick_once(&mut report.samples)?;
            report.ticks += 1;
            report.tick = self.tick();
            report.reached = predicate(self.play.session());
        }
        Ok(report)
    }

    /// Queues action injections for the next tick boundary.
    pub fn inject(&mut self, inputs: Vec<Injection>) -> Result<()> {
        let next = self.tick() + 1;
        Ok(self.play.session_mut().inject(next, inputs)?)
    }

    /// Queues committed authoring ops for the next tick boundary (live patch). The
    /// ops must match the projection baseline (`prev` values), like any transaction.
    pub fn patch(&mut self, ops: Vec<Op>) -> Result<()> {
        let next = self.tick() + 1;
        let transaction = ara_ops::Transaction {
            id: ara_ops::TxnId("tx_livepatch".into()),
            author: ara_ops::Author::Human,
            why: None,
            label: None,
            base: 0,
            result: 0,
            idempotency_key: None,
            reverts: None,
            ops,
        };
        Ok(self.play.session_mut().patch(next, &transaction)?)
    }

    fn tick_once(&mut self, samples: &mut Vec<Sample>) -> Result<()> {
        for plugin in &mut self.plugins {
            plugin.before_tick(self.play.session_mut())?;
        }
        let paused = self.play.state() == PlayState::Paused;
        let report = self.play.step(1, &[])?;
        self.pending = None;
        samples.extend(report.samples);
        if let Some(error) = self.fault() {
            if !paused {
                self.play.pause()?;
            }
            return Err(RuntimeError(error));
        }
        for plugin in &mut self.plugins {
            plugin.after_tick(self.play.session().world())?;
        }
        Ok(())
    }

    /// The first fatal subsystem fault (physics, animation), if any. The session
    /// pauses on a fault; restore a checkpoint to continue.
    pub fn fault(&self) -> Option<String> {
        let world = self.world();
        if let Some(error) = world
            .get_resource::<PhysicsStatus>()
            .and_then(|s| s.error.as_ref())
        {
            return Some(format!("physics: {error}"));
        }
        world
            .iter_entities()
            .find_map(|e| e.get::<AnimatorSlot>()?.fault.clone())
            .map(|e| format!("animation: {e}"))
    }

    /// Non-fatal navigation problems from the latest tick (agent ID, error); such
    /// agents report `Unreachable`.
    pub fn navigation_errors(&self) -> Vec<(u64, String)> {
        self.world()
            .get_resource::<ara_nav::NavWorld>()
            .map(|n| {
                n.errors()
                    .iter()
                    .map(|(id, e)| (*id, e.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Fills `frame` with the presentation of the current world at the clock's phase.
    pub fn extract(&mut self, frame: &mut RenderFrame) {
        let alpha = self.alpha();
        let tick = self.tick();
        frame.extract(self.play.session().world(), tick, alpha);
        for plugin in &mut self.plugins {
            plugin.frame(self.play.session().world(), frame);
        }
    }

    /// Replaces the asset source (newly pinned generations) and reloads changed
    /// models at this tick boundary. Animators of a reloaded model restart from its
    /// initial state (new programs reject old animation snapshots). A failing reload
    /// keeps the last good generation and is reported per asset.
    pub fn reload_assets(
        &mut self,
        assets: Box<dyn AssetSource>,
    ) -> Result<Vec<(String, std::result::Result<bool, String>)>> {
        self.assets = assets;
        let world = self.play.session_mut().world_mut();
        let ids: Vec<String> = world
            .resource::<Library>()
            .models
            .ids()
            .map(str::to_owned)
            .collect();
        let mut results = Vec::new();
        for id in ids {
            let result = world
                .resource_mut::<Library>()
                .models
                .load(self.assets.as_ref(), &id)
                .map_err(|e| e.0);
            results.push((id, result));
        }
        if results.iter().any(|(_, r)| matches!(r, Ok(true))) {
            world.resource_mut::<Library>().retain_live();
            let mut stale = Vec::new();
            let mut query = world.query::<(Entity, &AnimatorSlot)>();
            for (entity, slot) in query.iter(world) {
                let live = world
                    .resource::<Library>()
                    .models
                    .get(&slot.program.model.asset)
                    .is_some_and(|m| m.digest == slot.program.model.digest);
                if !live {
                    stale.push(entity);
                }
            }
            for entity in stale {
                world.entity_mut(entity).remove::<AnimatorSlot>();
            }
            animation::ensure_slots(world, self.hz)?;
        }
        Ok(results)
    }
}
