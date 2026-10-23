//! Navigation: navmeshes baked from static collision at world load, Detour crowds in
//! the fixed tick, and the bridge between agent positions and transforms/physics.
//! Agent gameplay state (`NavPosition`, `NavAgentState`) is captured by
//! [`NavigationProvider`]; meshes are derived data, rebuilt from authoring.
use crate::{
    Result, RuntimeError,
    components::Wanderer,
    math::{Pose, stable_id},
};
use ara_components::Transform;
use ara_nav::{
    AgentProfile, BakeInput, BakeSettings, Baker, Bounds, Geometry, NavAgent, NavAgentState,
    NavConfig, NavMesh, NavMeshSurface, NavPosition, NavStatus, NavWorld,
};
use ara_physics::{Collider, ColliderKind, MotionType, PhysicsPose, RigidBody};
use ara_sim::{Identity, SimContext, SimError, StateProvider};
use bevy_ecs::prelude::*;
use glam::Vec3;
use serde_json::Value;
use std::{any::TypeId, collections::BTreeMap, path::PathBuf};

/// Registers the navigation world, gives agents their runtime state and bakes every
/// `NavMeshSurface` from static box colliders inside its bounds. `cache` keeps
/// content-addressed tiles so repeated loads skip Recast.
pub fn install(
    world: &mut World,
    fixed_dt: f32,
    cache: Option<PathBuf>,
    poses: &BTreeMap<Entity, Pose>,
) -> Result<()> {
    ara_nav::register_sim(
        world,
        &mut Schedule::default(),
        NavConfig {
            fixed_dt,
            ..Default::default()
        },
    )?;
    let mut agents = Vec::new();
    let mut query = world.query::<(Entity, &Identity, &NavAgent)>();
    for (entity, id, agent) in query.iter(world) {
        agents.push((entity, id.0.clone(), *agent));
    }
    for (entity, id, _) in &agents {
        let position = poses.get(entity).map_or(Vec3::ZERO, |p| p.translation);
        world.entity_mut(*entity).insert((
            NavPosition(position.to_array()),
            NavAgentState::new(stable_id(id)),
        ));
    }
    let mut surfaces = Vec::new();
    let mut query = world.query::<(&Identity, &NavMeshSurface)>();
    for (id, surface) in query.iter(world) {
        surfaces.push((id.0.clone(), surface.clone()));
    }
    surfaces.sort_by(|a, b| a.0.cmp(&b.0));
    if surfaces.is_empty() {
        return Ok(());
    }
    let geometry = static_geometry(world);
    let baker = Baker::new(cache);
    for (_, surface) in surfaces {
        if surface.geometry.is_some() {
            return Err(RuntimeError(
                "NavMeshSurface.geometry assets are not baked by the runtime yet; use static colliders".into(),
            ));
        }
        for &profile in &surface.profiles {
            // Agents of this profile define its walkability dimensions.
            let agent = agents
                .iter()
                .map(|(_, _, a)| a)
                .find(|a| a.profile == profile)
                .copied()
                .unwrap_or_default();
            let settings = BakeSettings {
                origin: surface.min,
                max_y: surface.max[1],
                cell_size: surface.cell_size,
                tile_size: surface.tile_size,
                agent: AgentProfile {
                    id: profile,
                    radius: agent.radius,
                    height: agent.height,
                    ..Default::default()
                },
                ..Default::default()
            };
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
            let mesh = NavMesh::from_artifact(&artifact)?;
            world.resource_mut::<NavWorld>().stage(mesh);
        }
    }
    Ok(())
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

/// Snapshot provider for agent runtime state.
pub struct NavigationProvider;

impl StateProvider for NavigationProvider {
    fn name(&self) -> &'static str {
        "navigation"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![
            TypeId::of::<NavWorld>(),
            TypeId::of::<NavPosition>(),
            TypeId::of::<NavAgentState>(),
        ]
    }
    fn capture(&self, world: &World) -> std::result::Result<Value, SimError> {
        let mut rows = Vec::new();
        for entity in world.iter_entities() {
            if let (Some(id), Some(position), Some(state)) = (
                entity.get::<Identity>(),
                entity.get::<NavPosition>(),
                entity.get::<NavAgentState>(),
            ) {
                rows.push((id.0.clone(), *position, state.clone()));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(serde_json::to_value(rows)?)
    }
    fn restore(&self, world: &mut World, state: &Value) -> std::result::Result<(), SimError> {
        let rows: BTreeMap<String, (NavPosition, NavAgentState)> =
            serde_json::from_value::<Vec<(String, NavPosition, NavAgentState)>>(state.clone())?
                .into_iter()
                .map(|(id, p, s)| (id, (p, s)))
                .collect();
        let mut agents = Vec::new();
        let mut query = world.query_filtered::<(Entity, &Identity), With<NavAgent>>();
        for (entity, id) in query.iter(world) {
            agents.push((entity, id.0.clone()));
        }
        for (entity, id) in agents {
            let (position, state) = rows
                .get(&id)
                .cloned()
                .ok_or_else(|| SimError(format!("snapshot has no navigation state for {id}")))?;
            world.entity_mut(entity).insert((position, state));
        }
        Ok(())
    }
}

impl From<ara_nav::NavError> for RuntimeError {
    fn from(e: ara_nav::NavError) -> Self {
        Self(e.to_string())
    }
}
