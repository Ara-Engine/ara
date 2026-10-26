//! Ara's playable runtime: one place that composes the deterministic session
//! (`ara-sim`), input actions, Jolt physics, ara-anim animation, Recast navigation and
//! cooked assets into a world that plays a scene, plus a renderer-neutral
//! presentation extraction.
//!
//! # Fixed-tick system order
//!
//! Before the systems, the boundary applies inputs, live patches, commands and
//! streamed cell arrivals (plugins' `stream`).
//!
//! | # | System | Does |
//! |---|---|---|
//! | 0 | `poses.remember` | last world poses become the interpolation start |
//! | 1 | `input.cameras` | `Look` steers follow-camera rigs |
//! | 2 | `input.players`, `input.players2d` | `Move`/`Sprint`/`Jump` → character input, facing |
//! | 3 | plugins [`Stage::Gameplay`] | C# script systems |
//! | 4 | `ai.wander` | wanderers choose navmesh goals (session RNG) |
//! | 5 | `animation.slots`, `navigation.adopt` | streamed animators/agents/obstacles get runtime state |
//! | 6 | `navigation.stream` | resident terrain chunks changed → streamed navmesh assembled, staged |
//! | 7 | `navigation.obstacles` | obstacles follow their world pose |
//! | 8 | `ai.adopt`, `ai.poses` | streamed brains; perception poses of players/props |
//! | 9 | `ai.sync` → `ai.perceive` → `ai.think` → `ai.bridge` | brains perceive, think, steer navigation |
//! | 10 | `navigation` | publish meshes, obstacle rebakes, ORCA crowd step (exclusive) |
//! | 11 | `ai.locomotion` → `ai.report` | locomotion intents for animation; `ara why` reports |
//! | 12 | `animation` ∥ `navigation.follow` | animator tick ∥ agent poses (parallel group) |
//! | 13 | `animation.root` | root motion moves free animated entities |
//! | 14 | `terrain.chunks`, `physics.adopt` | streamed terrain chunks / bodies get native bodies |
//! | 15 | `physics.push` | edited bodies teleport to their `Transform` |
//! | 16 | `physics` | Jolt boundary: extract → step → mirror (exclusive) |
//! | 17 | `physics.pull` | moving bodies/characters → `Transform` |
//! | 18 | 2D physics (only with 2D bodies) | Rapier2D step |
//! | 19 | `camera.place` | rigs placed behind targets |
//! | 20 | plugins [`Stage::Late`] | replication, audio/VFX emission |
//! | 21 | `poses.resolve` | world poses for presentation (exclusive) |
//!
//! Presentation runs between ticks: [`Runtime::extract`] interpolates world poses with
//! the fixed-tick phase and never mutates the world.
#![warn(missing_docs)]

extern crate glam34 as glam;
/// `ARA7310`: a model or native material is unavailable in Play, so placeholder geometry
/// (or the source material) is shown. Players report asset updates with this code too.
pub const ASSET_PLACEHOLDER: ara_diag::Code = ara_diag::Code(7310);

pub mod ai;
pub mod animation;
pub mod assets;
pub mod components;
pub mod control;
pub mod controller;
pub mod engine;
pub mod extract;
pub mod follow;
pub mod live;
pub mod material;
pub mod math;
pub mod model;
pub mod navigation;
pub mod physics;
pub mod plugin;
mod preload;
pub mod skin;
pub mod sync;
pub mod terrain;

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
    registry.register_component::<ara_nav::NavObstacle>()?;
    registry.register_component::<ara_nav::NavAgentType>()?;
    registry.register_component::<ara_ai::Brain>()?;
    registry.register_component::<ara_perception::Perceiver>()?;
    registry.register_component::<ara_perception::Stimulus>()?;
    registry.register_component::<components::StreamedWorld>()?;
    registry.register_component::<MeshRenderer>()?;
    registry.register_component::<Light>()?;
    registry.register_component::<Environment>()?;
    registry.register_component::<ara_components::render::SkyAtmosphere>()?;
    registry.register_component::<ara_components::render::HeightFog>()?;
    registry.register_component::<ara_components::post::PostProcessVolume>()?;
    registry.register_component::<Camera>()?;
    registry.register_component::<FollowCamera>()?;
    registry.register_component::<PlayerControl>()?;
    registry.register_component::<Wanderer>()?;
    registry.register_component::<Animator>()?;
    registry.register_component::<components::VisualEffect>()?;
    registry.register_component::<components::MusicSource>()?;
    registry.register_component::<components::AudioGraphSource>()?;
    registry.register_component::<terrain::Terrain>()?;
    registry.register_component::<terrain::TerrainChunk>()?;
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

/// Every plugin's streaming at a boundary (runtime errors become a session fault).
fn stream(
    plugins: &mut [Box<dyn RuntimePlugin>],
    boundary: &mut ara_sim::StreamingBoundary<'_>,
) -> std::result::Result<(), SimError> {
    for plugin in plugins {
        plugin.stream(boundary).map_err(|e| SimError(e.0))?;
    }
    Ok(())
}

/// A network adapter with the runtime's plugins streaming at its boundary.
struct WithPlugins<'a, A> {
    adapter: &'a mut A,
    plugins: &'a mut [Box<dyn RuntimePlugin>],
}

impl<A: ara_sim::SessionAdapter> ara_sim::SessionAdapter for WithPlugins<'_, A> {
    fn mode(&self) -> ara_sim::SessionMode {
        self.adapter.mode()
    }
    fn before_tick(&mut self, session: &mut Session) -> std::result::Result<(), SimError> {
        self.adapter.before_tick(session)
    }
    fn stream(
        &mut self,
        boundary: &mut ara_sim::StreamingBoundary<'_>,
    ) -> std::result::Result<(), SimError> {
        stream(self.plugins, boundary)?;
        self.adapter.stream(boundary)
    }
    fn after_tick(
        &mut self,
        world: &World,
        record: &ara_sim::TickRecord,
    ) -> std::result::Result<(), SimError> {
        self.adapter.after_tick(world, record)
    }
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
    /// Presentation systems of a networked client's mirror world (built on first use).
    mirror: Option<Schedule>,
    /// The authoritative tick a mirror world presents (its own session never steps).
    presented: Option<u64>,
    asset_errors: Vec<String>,
}

impl Runtime {
    /// Projects `scene` from `workspace`, preloads model generations reachable through
    /// its prefabs, streamed cells and foliage, and builds the world at tick 0, paused.
    /// Missing models use diagnosed
    /// placeholders; schema, projection, bake and physics errors fail the build.
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
        let preload_models = preload::models(&baseline, &scene)?;
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
        let mut models = preload_models;
        let mut query = world.query::<&MeshRenderer>();
        for renderer in query.iter(world) {
            if let Some(model) = &renderer.model {
                models.insert(asset_id(model)?.to_owned());
            }
        }
        let mut query = world.query::<&ara_physics::Collider>();
        for collider in query.iter(world) {
            if matches!(
                collider.kind,
                ara_physics::ColliderKind::ModelConvex | ara_physics::ColliderKind::ModelMesh
            ) && let Some(asset) = &collider.asset
            {
                models.insert(asset_id(asset)?.to_owned());
            }
        }
        let (prepared, errors) = assets::Models::preload(assets.as_ref(), &models)?;
        library.models = prepared;
        let mut asset_errors: Vec<_> = errors
            .into_iter()
            .map(|error| {
                format!("ARA7310: {error}; showing placeholder geometry; re-import the asset")
            })
            .collect();
        let mut query = world.query::<(&ara_sim::Identity, &ara_physics::Collider)>();
        for (identity, collider) in query.iter(world) {
            if matches!(
                collider.kind,
                ara_physics::ColliderKind::ModelConvex | ara_physics::ColliderKind::ModelMesh
            ) && let Some(reference) = &collider.asset
                && library.models.get(asset_id(reference)?).is_none()
            {
                asset_errors.push(format!(
                    "{ASSET_PLACEHOLDER}: {}: model collider {} is unavailable; using Collider.halfExtents box bounds until its generation arrives",
                    identity.0, reference.0
                ));
            }
        }
        asset_errors.extend(library.models.validate_renderers(&baseline));
        for material in material::references(&baseline)? {
            if let Err(error) = library
                .materials
                .load(&baseline, assets.as_ref(), &material)
            {
                asset_errors.push(format!("ARA7310: {error}; retaining the source material"));
            }
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
        // Terrain evaluates (worker threads) before physics and navigation use it.
        if let Some(state) = terrain::build(world, &baseline)? {
            world.insert_resource(state);
        }
        let matrices = sync::world_matrices(world);
        let poses: Vec<(Entity, Pose)> = matrices
            .iter()
            .map(|(&e, &m)| (e, Pose::from_matrix(m)))
            .collect();
        physics::install(world, &config.physics, &poses)?;
        let streamed = world
            .query::<&components::StreamedWorld>()
            .iter(world)
            .any(|s| s.world.is_some());
        navigation::install(
            world,
            navigation::NavSetup {
                fixed_dt: config.physics.fixed_dt,
                cache: config.nav_cache.clone(),
                poses: &poses.iter().copied().collect(),
                streamed,
            },
        )?;
        ai::install(world, &baseline, config.physics.fixed_dt)?;
        animation::ensure_slots(world, hz)?;
        // Cameras start behind their targets rather than at their authored pose.
        world
            .run_system_once(control::place_cameras)
            .map_err(|e| RuntimeError(e.to_string()))?;
        sync::resolve_poses(world);

        session.add_provider(sync::PoseProvider)?;
        session.add_provider(terrain::TerrainProvider)?;
        session.add_provider(physics::PhysicsProvider {
            config: config.physics.clone(),
        })?;
        session.add_provider(navigation::NavigationProvider)?;
        session.add_provider(ai::AiProvider)?;
        session.add_provider(AnimationProvider { hz })?;

        session.add_system("poses.remember", sync::remember_poses)?;
        session.add_system("input.cameras", control::steer_cameras)?;
        session.add_system("input.players", control::drive_players)?;
        session.add_system("input.players2d", control::drive_players_2d)?;
        for plugin in &mut plugins {
            plugin.install(Stage::Gameplay, &mut session)?;
        }
        session.add_system("ai.wander", navigation::wander)?;
        session.add_system("animation.slots", animation::adopt_slots)?;
        session.add_system("navigation.adopt", navigation::adopt)?;
        session.add_system("navigation.stream", navigation::stream)?;
        session.add_system("navigation.obstacles", navigation::obstacles)?;
        session.add_system("ai.adopt", ai::adopt)?;
        session.add_system("ai.poses", ai::poses)?;
        session.add_system("ai.sync", ara_ai::sync)?;
        session.add_system("ai.perceive", ara_ai::perceive)?;
        session.add_system("ai.think", ara_ai::think)?;
        session.add_system("ai.bridge", ara_ai::bridge)?;
        session.add_system("navigation", ara_nav::tick)?;
        session.add_system("ai.locomotion", ara_ai::locomotion)?;
        session.add_system("ai.report", ara_ai::why::report)?;
        session.add_parallel_systems(vec![
            ("animation", boxed(animation::animate)),
            ("navigation.follow", boxed(navigation::follow)),
        ])?;
        session.add_system("animation.root", sync::apply_root_motion)?;
        session.add_system("terrain.chunks", terrain::adopt_chunks)?;
        session.add_system("physics.adopt", physics::adopt)?;
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
            mirror: None,
            presented: None,
            asset_errors,
        })
    }

    /// The authoring schema this world was projected with.
    pub fn schema(&self) -> &SchemaDb {
        &self.schema
    }

    /// Asset-load diagnostics; missing models render as placeholders and can recover
    /// on a later asset reload without restarting the simulation.
    pub fn asset_errors(&self) -> &[String] {
        &self.asset_errors
    }

    /// Asset-load warnings for the player's owner and diagnostics console.
    pub fn asset_diagnostics(&self) -> impl Iterator<Item = ara_diag::Diagnostic> + '_ {
        self.asset_errors
            .iter()
            .map(|message| ara_diag::Diagnostic::warning(ASSET_PLACEHOLDER, message))
    }

    /// The documents as projected at tick 0 (scene, prefabs, animation, mixer, music).
    pub fn documents(&self) -> &Workspace {
        &self.baseline
    }

    /// Play control: watches, snapshots, record/replay and the session.
    pub fn play(&self) -> &PlayControl {
        &self.play
    }
    /// Mutable play control (watches, record). Restore and replay through
    /// [`Runtime::restore`] and [`Runtime::replay`] so plugins resynchronize.
    pub fn play_mut(&mut self) -> &mut PlayControl {
        &mut self.play
    }
    /// Restores a checkpoint or portable snapshot, then lets plugins resynchronize
    /// (streamed cells resident at the checkpoint are adopted, not re-spawned).
    pub fn restore(&mut self, snapshot: &ara_sim::Snapshot) -> Result<()> {
        self.play.restore(snapshot)?;
        self.pending = None;
        for plugin in &mut self.plugins {
            plugin.restored(self.play.session_mut())?;
        }
        Ok(())
    }
    /// Verifies a replay tick by tick from its initial state (ending at its last
    /// tick), then lets plugins resynchronize. Returns the final state hash.
    pub fn replay(&mut self, replay: &ara_sim::Replay) -> Result<StateHash> {
        let hash = self.play.replay(replay)?;
        self.pending = None;
        for plugin in &mut self.plugins {
            plugin.restored(self.play.session_mut())?;
        }
        Ok(hash)
    }
    /// The simulation world, read-only.
    pub fn world(&self) -> &World {
        self.play.session().world()
    }
    /// Completed ticks (a networked mirror: the authoritative tick it presents).
    pub fn tick(&self) -> u64 {
        self.presented.unwrap_or_else(|| self.play.session().tick())
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
        let mut models = BTreeSet::new();
        for op in &ops {
            match op {
                Op::Set {
                    path,
                    value: Some(value),
                    ..
                } => {
                    assets::model_references(&path.to_string(), value, &mut models)?;
                    preload::extend(&self.baseline, value, &mut models)?;
                }
                Op::Create { record, .. } => {
                    for (key, value) in record {
                        assets::model_references(key, value, &mut models)?;
                        preload::extend(&self.baseline, value, &mut models)?;
                    }
                }
                _ => {}
            }
        }
        for asset in models {
            if let Err(error) = self
                .play
                .session_mut()
                .world_mut()
                .resource_mut::<Library>()
                .models
                .load(self.assets.as_ref(), &asset)
            {
                self.asset_errors.push(format!("ARA7310: {asset}: {error}"));
            }
        }
        // Resolve newly referenced material assets on this control boundary, never
        // from frame extraction. Existing entries survive a failed preparation.
        for asset in material::patch_references(&ops) {
            let result = self
                .play
                .session_mut()
                .world_mut()
                .resource_mut::<Library>()
                .materials
                .load(&self.baseline, self.assets.as_ref(), &asset);
            if let Err(error) = result {
                self.asset_errors.push(format!("ARA7310: {asset}: {error}"));
            }
        }
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

    /// Runs exactly one boundary with `adapter` around the session step (a networked
    /// authority: network input in before it, replication out after it), plugins
    /// around that. Host time does not advance here; the caller is the clock.
    pub fn step_with_adapter(
        &mut self,
        adapter: &mut impl ara_sim::SessionAdapter,
    ) -> Result<ara_sim::TickRecord> {
        for plugin in &mut self.plugins {
            plugin.before_tick(self.play.session_mut())?;
        }
        let mut adapter = WithPlugins {
            adapter,
            plugins: &mut self.plugins,
        };
        let record = self.play.session_mut().step_with_adapter(&mut adapter)?;
        self.pending = None;
        if let Some(error) = self.fault() {
            return Err(RuntimeError(error));
        }
        for plugin in &mut self.plugins {
            plugin.after_tick(self.play.session().world())?;
        }
        Ok(record)
    }

    /// A networked client's presentation advance: the caller wrote replicated state
    /// (transforms, velocities, player yaw) for authoritative `tick` into the world;
    /// this runs animation, camera placement and world poses once (one tick of
    /// animation time) without stepping the simulation. From then on [`Runtime::tick`]
    /// reports `tick` (effects replay to it, status shows it).
    pub fn present_mirror(&mut self, tick: u64) -> Result<()> {
        self.presented = Some(tick);
        for plugin in &mut self.plugins {
            plugin.mirror(self.play.session_mut(), tick)?;
        }
        let mirror = self.mirror.get_or_insert_with(|| {
            let mut schedule = Schedule::default();
            schedule.set_executor(bevy_ecs::schedule::SingleThreadedExecutor::new());
            schedule.add_systems(
                (
                    animation::animate,
                    control::place_cameras,
                    sync::resolve_poses,
                )
                    .chain(),
            );
            schedule
        });
        mirror.run(self.play.session_mut().world_mut());
        if let Some(error) = self.fault() {
            return Err(RuntimeError(error));
        }
        Ok(())
    }

    fn tick_once(&mut self, samples: &mut Vec<Sample>) -> Result<()> {
        for plugin in &mut self.plugins {
            plugin.before_tick(self.play.session_mut())?;
        }
        let paused = self.play.state() == PlayState::Paused;
        let plugins = &mut self.plugins;
        let report = self
            .play
            .step_streaming(|boundary| stream(plugins, boundary))?;
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
        if let Some(error) = world
            .get_resource::<navigation::NavStreaming>()
            .and_then(|s| s.error.clone())
        {
            return Some(error);
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
        let material_ids: Vec<_> = world
            .resource::<Library>()
            .materials
            .ids()
            .map(str::to_owned)
            .collect();
        for id in material_ids {
            let result = world
                .resource_mut::<Library>()
                .materials
                .load(&self.baseline, self.assets.as_ref(), &id)
                .map_err(|error| error.0);
            results.push((id, result));
        }
        Ok(results)
    }
}
