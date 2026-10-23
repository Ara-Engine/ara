//! Navigation (ara-nav v2): navmeshes baked at world load, ORCA crowds in the fixed
//! tick, and the bridge between agent positions and transforms/physics.
//!
//! - Static worlds bake one mesh per profile (dimensions from the profile's
//!   `NavAgentType`, else its first agent) over static box colliders and terrain, and
//!   register it with `NavWorld::set_bake_source` so `NavObstacle` carve/area changes
//!   rebake in the background (published a fixed number of ticks later).
//! - Streamed worlds ([`NavStreaming`]) bake every terrain cell up front and keep the
//!   resident mesh following the resident `TerrainChunk`s (which stream with world
//!   cells, so replays reproduce it): `NavStreamer` assembles at that tick boundary and
//!   each swap's tick is recorded.
//!
//! Agent gameplay state (`NavPosition`, `NavAgentState`), obstacle positions, pending
//! obstacle rebakes and the streamed resident sets are captured by
//! [`NavigationProvider`]; meshes are derived data, rebuilt from authoring.
use crate::{
    Result, RuntimeError,
    components::Wanderer,
    math::{Pose, stable_id},
    sync::WorldPose,
    terrain::{TerrainChunk, TerrainState},
};
use ara_components::Transform;
use ara_nav::{
    AgentProfile, BakeInput, BakeSettings, Baker, Bounds, CellCoord, CellGrid, CellSource,
    Geometry, NavAgent, NavAgentState, NavAgentType, NavConfig, NavMesh, NavMeshSurface,
    NavObstacle, NavPosition, NavStatus, NavStreamer, NavWorld, RebakeCheckpoint,
};
use ara_physics::{Collider, ColliderKind, MotionType, PhysicsPose, RigidBody};
use ara_sim::{Identity, SimContext, SimError, StateProvider};
use bevy_ecs::prelude::*;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

/// Swaps kept in [`NavStreaming::swaps`] (oldest dropped first).
const SWAP_HISTORY: usize = 256;

/// One published resident set of a streamed navmesh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavSwap {
    /// Tick whose navigation step published it (0: world load or restore).
    pub tick: u64,
    /// Agent profile.
    pub profile: u32,
    /// Resident nav cells.
    pub cells: Vec<CellCoord>,
}

/// Streamed navigation: one `NavStreamer` per profile over the terrain's cells.
#[derive(Resource)]
pub struct NavStreaming {
    streamers: BTreeMap<u32, NavStreamer>,
    /// The latest wanted cells per profile (assembly runs only when they change).
    wanted: BTreeMap<u32, BTreeSet<CellCoord>>,
    /// Recent publications, oldest first.
    pub swaps: Vec<NavSwap>,
    /// The first assembly failure; the runtime reports it as a fault.
    pub error: Option<String>,
}

impl NavStreaming {
    /// Resident cells per profile (as last published).
    pub fn resident(&self) -> BTreeMap<u32, Vec<CellCoord>> {
        self.streamers
            .iter()
            .map(|(p, s)| (*p, s.resident().iter().copied().collect()))
            .collect()
    }

    /// Requests `cells` for `profile` and waits for the assembly (deterministic: the
    /// mesh belongs to this boundary). Returns the mesh when the resident set changed.
    fn assemble(
        &mut self,
        profile: u32,
        cells: BTreeSet<CellCoord>,
    ) -> std::result::Result<Option<NavMesh>, String> {
        let Some(streamer) = self.streamers.get_mut(&profile) else {
            return Ok(None);
        };
        self.wanted.insert(profile, cells.clone());
        streamer.request(cells);
        match streamer.wait() {
            Some(Ok(mesh)) => Ok(Some(mesh)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    fn record(&mut self, tick: u64, profile: u32) {
        let cells = self
            .streamers
            .get(&profile)
            .map(|s| s.resident().iter().copied().collect())
            .unwrap_or_default();
        self.swaps.push(NavSwap {
            tick,
            profile,
            cells,
        });
        if self.swaps.len() > SWAP_HISTORY {
            self.swaps.remove(0);
        }
    }
}

/// How navigation is built at world load.
pub struct NavSetup<'a> {
    /// Fixed tick in seconds.
    pub fixed_dt: f32,
    /// Content-addressed tile cache.
    pub cache: Option<PathBuf>,
    /// World poses of every entity at load.
    pub poses: &'a BTreeMap<Entity, Pose>,
    /// Whether the scene streams a world: the mesh then follows resident terrain tiles.
    pub streamed: bool,
}

/// Registers the navigation world, gives agents and obstacles their runtime state and
/// bakes every `NavMeshSurface` (static boxes and terrain heightfields inside its
/// bounds), statically or per streamed cell.
pub fn install(world: &mut World, setup: NavSetup<'_>) -> Result<()> {
    ara_nav::register_sim(
        world,
        &mut Schedule::default(),
        NavConfig {
            fixed_dt: setup.fixed_dt,
            ..Default::default()
        },
    )?;
    adopt_with(world, setup.poses);
    let mut surfaces = Vec::new();
    let mut query = world.query::<(&Identity, &NavMeshSurface)>();
    for (id, surface) in query.iter(world) {
        surfaces.push((id.0.clone(), surface.clone()));
    }
    surfaces.sort_by(|a, b| a.0.cmp(&b.0));
    if surfaces.is_empty() {
        return Ok(());
    }
    let mut geometry = static_geometry(world);
    let terrain = world.get_resource::<TerrainState>().cloned();
    if let Some(state) = &terrain {
        crate::terrain::navigation_geometry(state, &mut geometry)?;
    }
    let baker = Baker::new(setup.cache.clone());
    let mut streaming = NavStreaming {
        streamers: BTreeMap::new(),
        wanted: BTreeMap::new(),
        swaps: Vec::new(),
        error: None,
    };
    for (_, surface) in surfaces {
        if surface.geometry.is_some() {
            return Err(RuntimeError(
                "NavMeshSurface.geometry assets are not baked by the runtime yet; use static colliders or terrain".into(),
            ));
        }
        for &profile in &surface.profiles {
            let settings = BakeSettings {
                origin: surface.min,
                max_y: surface.max[1],
                cell_size: surface.cell_size,
                tile_size: surface.tile_size,
                agent: agent_profile(world, profile),
                ..Default::default()
            };
            match (&terrain, setup.streamed) {
                (Some(state), true) => {
                    let streamer = bake_cells(&baker, settings, state, &geometry)?;
                    world
                        .resource_mut::<NavWorld>()
                        .set_cluster_graph(profile, streamer.graph());
                    streaming.streamers.insert(profile, streamer);
                }
                _ => {
                    let bounds = Bounds {
                        min: surface.min,
                        max: surface.max,
                    };
                    let input = BakeInput {
                        tiles: BakeInput::tiles_in(&settings, bounds)?,
                        settings,
                        geometry: geometry.clone(),
                        links: Vec::new(),
                        obstacles: Vec::new(),
                    };
                    let artifact = baker.bake(&input, None)?;
                    world.resource_mut::<NavWorld>().set_bake_source(
                        input,
                        artifact,
                        setup.cache.clone(),
                    )?;
                }
            }
        }
    }
    if !streaming.streamers.is_empty() {
        // The tiles resident at load (authored in the level) define the first mesh.
        let wanted = wanted_cells(world, &streaming);
        let profiles: Vec<u32> = streaming.streamers.keys().copied().collect();
        for profile in profiles {
            let cells = wanted.get(&profile).cloned().unwrap_or_default();
            if let Some(mesh) = streaming.assemble(profile, cells).map_err(RuntimeError)? {
                world.resource_mut::<NavWorld>().publish(mesh);
                streaming.record(0, profile);
            }
        }
        world.insert_resource(streaming);
    }
    Ok(())
}

/// Bake dimensions of `profile`: its `NavAgentType` (lowest ID first), else its first
/// agent's radius/height.
fn agent_profile(world: &mut World, profile: u32) -> AgentProfile {
    let mut types: Vec<(String, NavAgentType)> = world
        .query::<(&Identity, &NavAgentType)>()
        .iter(world)
        .filter(|(_, t)| t.profile == profile)
        .map(|(id, t)| (id.0.clone(), t.clone()))
        .collect();
    types.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some((_, agent_type)) = types.first() {
        return agent_type.profile();
    }
    let mut agents: Vec<(String, NavAgent)> = world
        .query::<(&Identity, &NavAgent)>()
        .iter(world)
        .filter(|(_, a)| a.profile == profile)
        .map(|(id, a)| (id.0.clone(), *a))
        .collect();
    agents.sort_by(|a, b| a.0.cmp(&b.0));
    let agent = agents.first().map(|(_, a)| *a).unwrap_or_default();
    AgentProfile {
        id: profile,
        radius: agent.radius,
        height: agent.height,
        ..Default::default()
    }
}

/// Bakes every terrain cell (one nav cell per terrain tile; worker threads) and
/// indexes them for streaming.
fn bake_cells(
    baker: &Baker,
    settings: BakeSettings,
    terrain: &TerrainState,
    geometry: &Geometry,
) -> Result<NavStreamer> {
    let header = &terrain.0.header;
    let width = header.tile_width();
    let ratio = width / settings.tile_width();
    let tiles_per_cell = ratio.round() as u16;
    let aligned = |o: f32, g: f32| ((o - g) / settings.tile_width()).fract().abs() < 1e-3;
    if (ratio - f32::from(tiles_per_cell)).abs() > 1e-3
        || !aligned(header.origin[0], settings.origin[0])
        || !aligned(header.origin[1], settings.origin[2])
    {
        return Err(RuntimeError(format!(
            "streamed navigation needs terrain tiles ({width} m) to be whole nav tiles \
             ({} m) on the same grid",
            settings.tile_width()
        )));
    }
    let grid = CellGrid::new(settings, tiles_per_cell)?;
    let cells: Vec<CellCoord> = terrain
        .0
        .tiles
        .keys()
        .map(|c| grid.cell_of(tile_center(terrain, c.x, c.z)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4));
    let spacing = grid.cell_width() / 8.0;
    let bakes = std::thread::scope(|scope| {
        let handles: Vec<_> = cells
            .chunks(cells.len().div_ceil(workers).max(1))
            .map(|chunk| {
                let grid = &grid;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|&cell| {
                            let source = CellSource {
                                geometry: geometry.clone(),
                                links: Vec::new(),
                                obstacles: Vec::new(),
                            };
                            ara_nav::bake_cell(baker, grid, cell, source, None, spacing)
                                .map_err(|e| e.to_string())
                        })
                        .collect::<std::result::Result<Vec<_>, String>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("navigation bake worker panicked".into()))
            })
            .collect::<std::result::Result<Vec<_>, String>>()
    })
    .map_err(|e| RuntimeError(format!("streamed navigation: {e}")))?;
    Ok(NavStreamer::new(
        grid,
        bakes.into_iter().flatten().collect(),
    )?)
}

fn tile_center(terrain: &TerrainState, x: i32, z: i32) -> [f32; 3] {
    let header = &terrain.0.header;
    let width = header.tile_width();
    [
        header.origin[0] + (x as f32 + 0.5) * width,
        0.0,
        header.origin[1] + (z as f32 + 0.5) * width,
    ]
}

/// Nav cells under every resident `TerrainChunk`, per streamed profile.
fn wanted_cells(world: &mut World, streaming: &NavStreaming) -> BTreeMap<u32, BTreeSet<CellCoord>> {
    let Some(terrain) = world.get_resource::<TerrainState>().cloned() else {
        return BTreeMap::new();
    };
    let tiles: Vec<TerrainChunk> = world
        .query::<&TerrainChunk>()
        .iter(world)
        .copied()
        .collect();
    streaming
        .streamers
        .iter()
        .map(|(profile, streamer)| {
            let cells = tiles
                .iter()
                .map(|t| streamer.grid().cell_of(tile_center(&terrain, t.x, t.z)))
                .collect();
            (*profile, cells)
        })
        .collect()
}

/// The fixed-tick streaming hook (before `navigation`): when the resident terrain
/// tiles changed, assembles the new resident mesh at this boundary and stages it (the
/// navigation step publishes it this tick). Agents keep their world-space corridors.
pub fn stream(world: &mut World) {
    let Some(mut streaming) = world.remove_resource::<NavStreaming>() else {
        return;
    };
    if streaming.error.is_none() {
        let tick = world.resource::<SimContext>().tick();
        for (profile, cells) in wanted_cells(world, &streaming) {
            if streaming.wanted.get(&profile) == Some(&cells) {
                continue;
            }
            match streaming.assemble(profile, cells) {
                Ok(Some(mesh)) => {
                    world.resource_mut::<NavWorld>().stage(mesh);
                    streaming.record(tick, profile);
                }
                Ok(None) => {}
                Err(e) => streaming.error = Some(format!("navigation streaming: {e}")),
            }
        }
    }
    world.insert_resource(streaming);
}

/// Triangles of every static box collider, in stable physics-ID order.
fn static_geometry(world: &mut World) -> Geometry {
    let mut boxes = Vec::new();
    let mut query = world.query::<(&RigidBody, &Collider, &PhysicsPose, &ara_physics::PhysicsId)>();
    for (body, collider, pose, id) in query.iter(world) {
        if body.motion == MotionType::Static
            && collider.kind == ColliderKind::Box
            && !collider.sensor
        {
            boxes.push((id.value, *pose, collider.half_extents));
        }
    }
    boxes.sort_by_key(|b| b.0);
    let mut geometry = Geometry::default();
    for (_, pose, half) in boxes {
        let pose = Pose::from_physics(&pose, Vec3::ONE);
        let h = Vec3::from_array(half);
        let corner = |i: usize| {
            let sign = |bit: usize| if i >> bit & 1 == 1 { 1.0 } else { -1.0 };
            pose.translation + pose.rotation * (h * Vec3::new(sign(0), sign(1), sign(2)))
        };
        let base = geometry.vertices.len() as u32;
        geometry
            .vertices
            .extend((0..8).map(|i| corner(i).to_array()));
        // Six faces as corner quads with their outward local normals.
        let faces = [
            ([0, 2, 6, 4], Vec3::NEG_X),
            ([1, 5, 7, 3], Vec3::X),
            ([0, 4, 5, 1], Vec3::NEG_Y),
            ([2, 3, 7, 6], Vec3::Y),
            ([0, 1, 3, 2], Vec3::NEG_Z),
            ([4, 6, 7, 5], Vec3::Z),
        ];
        for (quad, normal) in faces {
            let outward = pose.rotation * normal;
            for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                let [a, b, c] = tri.map(corner);
                // Recast walks the side where (b - a) × (c - a) points.
                let tri = if (b - a).cross(c - a).dot(outward) >= 0.0 {
                    tri
                } else {
                    [tri[0], tri[2], tri[1]]
                };
                geometry.triangles.push(tri.map(|i| base + i as u32));
                geometry.areas.push(0);
            }
        }
    }
    geometry
}

/// Agents and obstacles without runtime state get it, positioned at `poses` (load) or
/// their world pose/transform (streamed in).
fn adopt_with(world: &mut World, poses: &BTreeMap<Entity, Pose>) {
    let mut agents = Vec::new();
    let mut query = world.query_filtered::<
        (Entity, &Identity, Option<&WorldPose>, Option<&Transform>),
        (With<NavAgent>, Without<NavAgentState>),
    >();
    for (entity, id, world_pose, transform) in query.iter(world) {
        let position = poses
            .get(&entity)
            .map(|p| p.translation.to_array())
            .or_else(|| world_pose.map(|p| p.0.translation.to_array()))
            .or_else(|| transform.map(|t| t.position))
            .unwrap_or_default();
        agents.push((entity, stable_id(&id.0), position));
    }
    for (entity, id, position) in agents {
        world
            .entity_mut(entity)
            .insert((NavPosition(position), NavAgentState::new(id)));
    }
    let mut obstacles = Vec::new();
    let mut query = world.query_filtered::<
        (Entity, Option<&WorldPose>, Option<&Transform>),
        (With<NavObstacle>, Without<NavPosition>),
    >();
    for (entity, world_pose, transform) in query.iter(world) {
        let position = poses
            .get(&entity)
            .map(|p| p.translation.to_array())
            .or_else(|| world_pose.map(|p| p.0.translation.to_array()))
            .or_else(|| transform.map(|t| t.position))
            .unwrap_or_default();
        obstacles.push((entity, position));
    }
    for (entity, position) in obstacles {
        world.entity_mut(entity).insert(NavPosition(position));
    }
}

/// The fixed-tick adoption hook: streamed-in agents and obstacles get their runtime
/// state at the boundary that spawned them.
pub fn adopt(world: &mut World) {
    adopt_with(world, &BTreeMap::new());
}

/// Obstacles follow their entity's world pose (moved props carve where they are).
pub fn obstacles(
    mut query: Query<(&mut NavPosition, &WorldPose), (With<NavObstacle>, Without<NavAgent>)>,
) {
    for (mut position, pose) in &mut query {
        let p = pose.0.translation.to_array();
        if position.0 != p {
            position.0 = p;
        }
    }
}

/// Chooses new goals for idle wanderers from the session RNG, after a pause.
pub fn wander(
    mut ctx: ResMut<SimContext>,
    mut query: Query<(&Identity, &mut Wanderer, &mut NavAgentState)>,
) {
    let tick = ctx.tick();
    for (id, mut wanderer, mut state) in &mut query {
        if state.status == NavStatus::Moving {
            continue;
        }
        if wanderer.resume_tick == 0 {
            wanderer.resume_tick = tick + u64::from(wanderer.pause_ticks.max(1));
            continue;
        }
        if tick < wanderer.resume_tick {
            continue;
        }
        let mut rng = ctx.rng("wander", &id.0);
        let mut goal = [0.0; 3];
        for (axis, slot) in goal.iter_mut().enumerate() {
            *slot = wanderer.min[axis] + (wanderer.max[axis] - wanderer.min[axis]) * rng.unit_f32();
        }
        state.target = Some(goal);
        state.path = None;
        state.corner = 0;
        state.traversal = None;
        state.status = NavStatus::Moving;
        wanderer.resume_tick = 0;
        wanderer.goals += 1;
    }
}

type Agents<'w, 's> = Query<
    'w,
    's,
    (
        &'static NavPosition,
        &'static NavAgentState,
        &'static mut Transform,
        Option<&'static mut PhysicsPose>,
        Option<&'static Collider>,
    ),
    With<NavAgent>,
>;

/// Moves navigation agents' transforms to their crowd positions (feet), facing their
/// velocity. A kinematic body follows, lifted so its collider rests on the ground.
pub fn follow(mut query: Agents) {
    for (position, state, mut transform, pose, collider) in &mut query {
        let [vx, _, vz] = state.velocity;
        transform.position = position.0;
        if vx * vx + vz * vz > 1e-4 {
            transform.rotation = [0.0, vx.atan2(vz).to_degrees(), 0.0];
        }
        if let Some(mut pose) = pose {
            let lift = collider.map_or(0.0, |c| match c.kind {
                ColliderKind::Box => c.half_extents[1],
                ColliderKind::Sphere => c.radius,
                ColliderKind::Capsule => c.half_height + c.radius,
                ColliderKind::Cylinder => c.half_height,
                ColliderKind::Cooked => 0.0,
            });
            let mut body = Pose::from_transform(&transform);
            body.translation.y += lift;
            *pose = body.to_physics();
        }
    }
}

/// Snapshot form of the navigation runtime state.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Captured {
    /// Agents and obstacles by identity: position and (agents) state.
    rows: Vec<(String, NavPosition, Option<NavAgentState>)>,
    /// Obstacles baked into each live mesh and pending rebakes.
    rebakes: RebakeCheckpoint,
    /// Streamed resident cells per profile.
    #[serde(default)]
    resident: BTreeMap<u32, Vec<CellCoord>>,
}

/// Snapshot provider for agent/obstacle runtime state, obstacle rebakes and streamed
/// resident sets. Restores rebuild the matching meshes (blocking, outside a tick).
pub struct NavigationProvider;

impl StateProvider for NavigationProvider {
    fn name(&self) -> &'static str {
        "navigation"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![
            TypeId::of::<NavWorld>(),
            TypeId::of::<NavStreaming>(),
            TypeId::of::<NavPosition>(),
            TypeId::of::<NavAgentState>(),
        ]
    }
    fn capture(&self, world: &World) -> std::result::Result<Value, SimError> {
        let mut rows = Vec::new();
        for entity in world.iter_entities() {
            if let (Some(id), Some(position)) =
                (entity.get::<Identity>(), entity.get::<NavPosition>())
            {
                rows.push((
                    id.0.clone(),
                    *position,
                    entity.get::<NavAgentState>().cloned(),
                ));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(serde_json::to_value(Captured {
            rows,
            rebakes: world
                .get_resource::<NavWorld>()
                .map(NavWorld::rebake_checkpoint)
                .unwrap_or_default(),
            resident: world
                .get_resource::<NavStreaming>()
                .map(NavStreaming::resident)
                .unwrap_or_default(),
        })?)
    }
    fn restore(&self, world: &mut World, state: &Value) -> std::result::Result<(), SimError> {
        let captured: Captured = serde_json::from_value(state.clone())?;
        let mut rows: BTreeMap<String, (NavPosition, Option<NavAgentState>)> = captured
            .rows
            .into_iter()
            .map(|(id, p, s)| (id, (p, s)))
            .collect();
        let mut targets = Vec::new();
        let mut query = world
            .query_filtered::<(Entity, &Identity, Has<NavAgent>), Or<(With<NavAgent>, With<NavObstacle>)>>();
        for (entity, id, agent) in query.iter(world) {
            targets.push((entity, id.0.clone(), agent));
        }
        for (entity, id, agent) in targets {
            let (position, state) = rows
                .remove(&id)
                .ok_or_else(|| SimError(format!("snapshot has no navigation state for {id}")))?;
            let mut entity = world.entity_mut(entity);
            entity.insert(position);
            match state {
                Some(state) => {
                    entity.insert(state);
                }
                None if agent => {
                    return Err(SimError(format!("snapshot has no agent state for {id}")));
                }
                None => {}
            }
        }
        if let Some(mut nav) = world.get_resource_mut::<NavWorld>() {
            nav.restore_rebakes(&captured.rebakes)
                .map_err(|e| SimError(e.to_string()))?;
        }
        if let Some(mut streaming) = world.remove_resource::<NavStreaming>() {
            let mut result = Ok(());
            for (profile, cells) in captured.resident {
                let same = streaming
                    .streamers
                    .get(&profile)
                    .is_some_and(|s| s.resident().iter().eq(cells.iter()));
                let cells: BTreeSet<CellCoord> = cells.into_iter().collect();
                if same {
                    streaming.wanted.insert(profile, cells);
                    continue;
                }
                match streaming.assemble(profile, cells) {
                    Ok(Some(mesh)) => {
                        world.resource_mut::<NavWorld>().publish(mesh);
                        streaming.record(0, profile);
                    }
                    Ok(None) => {}
                    Err(e) => result = Err(SimError(format!("navigation streaming: {e}"))),
                }
            }
            world.insert_resource(streaming);
            result?;
        }
        Ok(())
    }
}

impl From<ara_nav::NavError> for RuntimeError {
    fn from(e: ara_nav::NavError) -> Self {
        Self(e.to_string())
    }
}
