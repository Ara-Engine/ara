//! Stable-ID extraction -> runtime commands -> fixed step -> ECS writeback.
mod binding;
mod snapshot;
pub use snapshot::{EcsPhysicsSnapshot, restore_ecs, snapshot_ecs};

use crate::*;
use bevy_ecs::change_detection::DetectChangesMut;
use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::prelude::{
    Entity, IntoScheduleConfigs, Mut, QueryState, Resource, Schedule, SystemSet, World,
};
use binding::Binding;
use std::collections::BTreeMap;

/// Ordered runtime body intent addressed by stable PhysicsId, never a document edit.
#[derive(Debug, Clone, Copy)]
pub enum BodyIntent {
    /// Teleport to a new world pose.
    Teleport(PhysicsPose),
    /// Reach a kinematic target at the next tick.
    KinematicTarget(PhysicsPose),
    /// Set linear/angular velocity.
    Velocity(Velocity),
    /// Apply linear impulse at a world point plus angular impulse.
    Impulse {
        /// Linear impulse in kg*m/s.
        impulse: [f32; 3],
        /// World-space application point.
        point: [f32; 3],
        /// Angular impulse in kg*m2/s.
        angular: [f32; 3],
    },
    /// Apply force/torque for one tick.
    Force {
        /// Force in newtons.
        force: [f32; 3],
        /// Torque in newton-meters.
        torque: [f32; 3],
    },
    /// Wake a sleeping body.
    Wake,
}
impl BodyIntent {
    fn resolve(self, body: BodyHandle) -> BodyCommand {
        match self {
            Self::Teleport(pose) => BodyCommand::Teleport { body, pose },
            Self::KinematicTarget(pose) => BodyCommand::KinematicTarget { body, pose },
            Self::Velocity(velocity) => BodyCommand::Velocity { body, velocity },
            Self::Impulse {
                impulse,
                point,
                angular,
            } => BodyCommand::Impulse {
                body,
                impulse,
                point,
                angular,
            },
            Self::Force { force, torque } => BodyCommand::Force {
                body,
                force,
                torque,
            },
            Self::Wake => BodyCommand::Wake(body),
        }
    }
}

/// Queue submitted before `PhysicsSet`; stable sorting preserves intent order per entity.
#[derive(Debug, Default, Clone, Resource)]
pub struct PhysicsCommands {
    /// (nonzero PhysicsId, runtime intent), consumed once by a successful boundary.
    pub bodies: Vec<(u64, BodyIntent)>,
}

/// Sticky fault from the physics schedule. The host pauses that session and reports it.
#[derive(Debug, Default, Resource)]
pub struct PhysicsStatus {
    /// Last failure; clear only after the host has repaired/restored the session.
    pub error: Option<PhysicsError>,
}

/// Last-boundary wall timings, enabled only in benchmark builds. Not gameplay state.
#[cfg(feature = "bench")]
#[derive(Default, Resource)]
pub struct PhysicsTimings {
    /// Lifecycle discovery and native creation/removal.
    pub synchronize_ns: u64,
    /// Input extraction, commands and character movement.
    pub inputs_ns: u64,
    /// Rigid-body update and effect decoding.
    pub step_ns: u64,
    /// Native body-column extraction.
    pub extraction_ns: u64,
    /// Component comparison and changed-value writes, including characters/vehicles.
    pub writeback_ns: u64,
}

/// Ordering seam for ara-sim: gameplay inputs before this set, presentation afterward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SystemSet)]
pub struct PhysicsSet;

/// Infrastructure owned by the physics checkpoint provider. ara-sim can exclude
/// these caches/resources from ordinary Serde state while capturing `snapshot_ecs`.
pub fn infrastructure_type_ids<B: PhysicsBackend>() -> [std::any::TypeId; 4] {
    [
        std::any::TypeId::of::<PhysicsWorld<B>>(),
        std::any::TypeId::of::<Runtime>(),
        std::any::TypeId::of::<PhysicsCommands>(),
        std::any::TypeId::of::<PhysicsStatus>(),
    ]
}

#[derive(Resource)]
struct Runtime {
    query: QueryState<(Entity, &'static PhysicsId)>,
    rows: Vec<(u64, Entity)>,
    removed: Vec<u64>,
    bindings: BTreeMap<u64, Binding>,
    commands: Vec<BodyCommand>,
    bodies: Vec<BodyHandle>,
    columns: BodyColumns,
    pending: Vec<BodyDesc>,
    spawned: Vec<BodyHandle>,
    despawned: Vec<BodyHandle>,
}

/// Installs one physics backend into a Bevy 0.20 world and fixed schedule.
/// The host resolves hierarchy/scale into `PhysicsPose` before `PhysicsSet`.
/// Register all reflected physics components with ara-sim's typed registry as well.
pub fn register<B: PhysicsBackend>(
    world: &mut World,
    schedule: &mut Schedule,
    config: PhysicsConfig,
) -> Result<()> {
    if world.contains_resource::<Runtime>() {
        return Err(PhysicsError(
            "physics is already registered in this ECS world".into(),
        ));
    }
    let physics = PhysicsWorld::<B>::new(config)?;
    let query = world.query::<(Entity, &PhysicsId)>();
    world.insert_resource(physics);
    world.insert_resource(Runtime {
        query,
        rows: Vec::new(),
        removed: Vec::new(),
        bindings: BTreeMap::new(),
        commands: Vec::new(),
        bodies: Vec::new(),
        columns: BodyColumns::default(),
        pending: Vec::new(),
        spawned: Vec::new(),
        despawned: Vec::new(),
    });
    world.init_resource::<PhysicsCommands>();
    world.init_resource::<PhysicsStatus>();
    #[cfg(feature = "bench")]
    world.init_resource::<PhysicsTimings>();
    schedule.add_systems(scheduled_tick::<B>.in_set(PhysicsSet));
    Ok(())
}

fn scheduled_tick<B: PhysicsBackend>(world: &mut World) {
    if world.resource::<PhysicsStatus>().error.is_some() {
        return;
    }
    if let Err(error) = tick_ecs::<B>(world) {
        world.resource_mut::<PhysicsStatus>().error = Some(error);
    }
}

/// Runs the boundary directly, returning errors instead of storing a schedule fault.
pub fn tick_ecs<B: PhysicsBackend>(world: &mut World) -> Result<()> {
    world.resource_scope(|world, mut physics: Mut<PhysicsWorld<B>>| {
        world.resource_scope(|world, mut runtime: Mut<Runtime>| {
            run(world, &mut physics, &mut runtime)
        })
    })
}

/// Resolves a current runtime body from stable identity. Do not persist the returned handle.
pub fn body_for_id(world: &World, id: u64) -> Option<BodyHandle> {
    world.get_resource::<Runtime>()?.bindings.get(&id)?.body()
}

/// Resolves event/query body identity back to its stable simulation entity identity.
pub fn id_for_body(world: &World, body: BodyHandle) -> Option<u64> {
    world
        .get_resource::<Runtime>()?
        .bindings
        .iter()
        .find_map(|(&id, b)| (b.body() == Some(body)).then_some(id))
}

/// Extracts lifecycle/configuration changes without advancing a tick or consuming input.
/// Use this after scene projection and before the initial tick-zero simulation snapshot.
pub fn prepare_ecs<B: PhysicsBackend>(world: &mut World) -> Result<()> {
    world.resource_scope(|world, mut physics: Mut<PhysicsWorld<B>>| {
        world.resource_scope(|world, mut runtime: Mut<Runtime>| {
            synchronize(world, &mut physics, &mut runtime)
        })
    })
}

fn run<B: PhysicsBackend>(
    world: &mut World,
    physics: &mut PhysicsWorld<B>,
    r: &mut Runtime,
) -> Result<()> {
    #[cfg(feature = "bench")]
    let start = std::time::Instant::now();
    synchronize(world, physics, r)?;
    #[cfg(feature = "bench")]
    {
        world.resource_mut::<PhysicsTimings>().synchronize_ns = start.elapsed().as_nanos() as u64;
    }
    #[cfg(feature = "bench")]
    let start = std::time::Instant::now();
    r.commands.clear();
    r.bodies.clear();
    // synchronize establishes one binding per sorted row. Walk both ordered
    // collections together instead of doing a tree lookup for every column.
    for (&(_, e), b) in r.rows.iter().zip(r.bindings.values()) {
        if let Some(body) = b.body() {
            let pose = world.get::<PhysicsPose>(e).copied().unwrap_or_default();
            let velocity = world.get::<Velocity>(e).copied().unwrap_or_default();
            if pose != b.last_pose {
                r.commands.push(if b.motion() == MotionType::Kinematic {
                    BodyCommand::KinematicTarget { body, pose }
                } else {
                    BodyCommand::Teleport { body, pose }
                });
            }
            if velocity != b.last_velocity {
                r.commands.push(BodyCommand::Velocity { body, velocity });
            }
            r.bodies.push(body);
        }
    }
    world.resource_scope(|_, mut queue: Mut<PhysicsCommands>| -> Result<()> {
        queue.bodies.sort_by_key(|c| c.0);
        for &(id, intent) in &queue.bodies {
            let body =
                r.bindings.get(&id).and_then(Binding::body).ok_or_else(|| {
                    PhysicsError("body intent targets an unknown PhysicsId".into())
                })?;
            r.commands.push(intent.resolve(body));
        }
        physics.apply(&r.commands)?;
        queue.bodies.clear();
        Ok(())
    })?;
    for (&(_, e), b) in r.rows.iter().zip(r.bindings.values()) {
        if let Some(c) = b.character {
            physics.step_character(
                c,
                world.get::<CharacterInput>(e).copied().unwrap_or_default(),
            )?;
        }
        if let Some(v) = b.vehicle {
            physics
                .backend_mut()
                .drive(v, world.get::<VehicleInput>(e).copied().unwrap_or_default())?;
        }
    }
    #[cfg(feature = "bench")]
    {
        world.resource_mut::<PhysicsTimings>().inputs_ns = start.elapsed().as_nanos() as u64;
    }
    #[cfg(feature = "bench")]
    let start = std::time::Instant::now();
    physics.step()?;
    #[cfg(feature = "bench")]
    {
        world.resource_mut::<PhysicsTimings>().step_ns = start.elapsed().as_nanos() as u64;
    }
    writeback(world, physics, r)
}

fn writeback<B: PhysicsBackend>(
    world: &mut World,
    physics: &mut PhysicsWorld<B>,
    r: &mut Runtime,
) -> Result<()> {
    #[cfg(feature = "bench")]
    let start = std::time::Instant::now();
    physics
        .backend_mut()
        .read_body_columns(&r.bodies, &mut r.columns)?;
    #[cfg(feature = "bench")]
    {
        world.resource_mut::<PhysicsTimings>().extraction_ns = start.elapsed().as_nanos() as u64;
    }
    #[cfg(feature = "bench")]
    let start = std::time::Instant::now();
    let mut index = 0;
    for (&(_, e), b) in r.rows.iter().zip(r.bindings.values_mut()) {
        if b.body().is_some() {
            let (pose, velocity) = r
                .columns
                .state(index)
                .ok_or_else(|| PhysicsError("incomplete extraction columns".into()))?;
            index += 1;
            b.last_pose = pose;
            b.last_velocity = velocity;
            write_changed(world, e, pose);
            write_changed(world, e, velocity);
        }
        if let Some(c) = b.character {
            let s = physics.backend().character_state(c)?;
            let pose = PhysicsPose {
                position: s.position,
                ..Default::default()
            };
            let velocity = Velocity {
                linear: s.velocity,
                angular: [0.0; 3],
            };
            b.last_pose = pose;
            b.last_velocity = velocity;
            write_changed(world, e, pose);
            write_changed(world, e, velocity);
            write_changed(
                world,
                e,
                CharacterMotion {
                    grounded: s.grounded,
                    crouched: s.crouched,
                    ground_velocity: s.ground_velocity,
                    ground_normal: s.ground_normal,
                },
            );
        }
        if let Some(v) = b.vehicle {
            let s = physics.backend().vehicle_state(v)?;
            write_changed(
                world,
                e,
                VehicleTelemetry {
                    wheels_in_contact: s.wheels_in_contact,
                    engine_rpm: s.engine_rpm,
                    gear: s.gear,
                },
            );
        }
    }
    #[cfg(feature = "bench")]
    {
        world.resource_mut::<PhysicsTimings>().writeback_ns = start.elapsed().as_nanos() as u64;
    }
    Ok(())
}

// Check the actual ECS value: gameplay may remove a mirrored component between
// boundaries, in which case it must be reinstalled even if native state is quiet.
fn write_changed<T: Component<Mutability = Mutable> + PartialEq>(
    world: &mut World,
    e: Entity,
    value: T,
) {
    if let Some(mut current) = world.get_mut::<T>(e) {
        current.set_if_neq(value);
    } else {
        world.entity_mut(e).insert(value);
    }
}

fn synchronize<B: PhysicsBackend>(
    world: &mut World,
    physics: &mut PhysicsWorld<B>,
    r: &mut Runtime,
) -> Result<()> {
    let result = synchronize_inner(world, physics, r);
    if result.is_err() {
        r.bindings.retain(|_, binding| !binding.incomplete());
        r.pending.clear();
        r.spawned.clear();
    }
    result
}

fn synchronize_inner<B: PhysicsBackend>(world: &mut World, physics: &mut PhysicsWorld<B>, r: &mut Runtime) -> Result<()> {
    r.rows.clear();
    r.rows
        .extend(r.query.iter(world).map(|(entity, id)| (id.value, entity)));
    r.rows.sort_unstable_by_key(|r| r.0);
    if r.rows.first().is_some_and(|r| r.0 == 0) || r.rows.windows(2).any(|p| p[0].0 == p[1].0) {
        return Err(PhysicsError("PhysicsId must be nonzero and unique".into()));
    }
    // Validate the entire extraction before processing lifecycle changes.
    for &(_, e) in &r.rows {
        binding::validate(world, e)?;
    }
    r.removed.clear();
    for (&id, b) in &r.bindings {
        if let Ok(i) = r.rows.binary_search_by_key(&id, |r| r.0) {
            if !b.matches(world, r.rows[i].1) {
                r.removed.push(id);
            }
        } else {
            r.removed.push(id);
        }
    }
    let rebuild_joints = !r.removed.is_empty()
        || r.rows.iter().any(|(id, e)| {
            r.bindings
                .get(id)
                .is_none_or(|b| b.joint_settings != world.get::<Joint>(*e).copied())
        });
    if rebuild_joints {
        for b in r.bindings.values_mut() {
            if let Some(j) = b.joint.take() {
                physics.backend_mut().destroy_joint(j)?;
            }
            b.joint_settings = None;
        }
    }
    r.despawned.clear();
    for id in &r.removed {
        if let Some(b) = r.bindings.remove(id) {
            if let Ok(i) = r.rows.binary_search_by_key(id, |r| r.0) {
                let e = r.rows[i].1;
                if b.character.is_some() && world.get::<CharacterController>(e).is_none() {
                    world.entity_mut(e).remove::<CharacterMotion>();
                }
                if b.vehicle.is_some() && world.get::<Vehicle>(e).is_none() {
                    world.entity_mut(e).remove::<VehicleTelemetry>();
                }
            }
            if b.vehicle.is_some() || b.character.is_some() {
                physics.backend_mut().destroy_bodies(&r.despawned)?;
                r.despawned.clear();
                b.destroy(physics.backend_mut())?;
            } else if let Some(body) = b.body {
                r.despawned.push(body);
            }
        }
    }
    physics.backend_mut().destroy_bodies(&r.despawned)?;
    r.pending.clear();
    r.spawned.clear();
    let optimize = !r.removed.is_empty()
        || r.rows.iter().any(|(id, _)| !r.bindings.contains_key(id));
    // Flush before a different actor kind so native construction still follows
    // PhysicsId across rigid bodies, vehicles and characters, not just within a batch.
    for index in 0..r.rows.len() {
        let (id, e) = r.rows[index];
        if !r.bindings.contains_key(&id) {
            if world.get::<CharacterController>(e).is_some() || world.get::<Vehicle>(e).is_some() {
                flush_bodies(physics.backend_mut(), r)?;
            }
            let binding = Binding::create(
                world,
                e,
                id,
                physics.backend_mut(),
                &mut r.pending,
            )?;
            r.bindings.insert(id, binding);
        }
    }
    flush_bodies(physics.backend_mut(), r)?;
    if optimize {
        physics.backend_mut().optimize()?;
    }
    if rebuild_joints {
        // Resolve connected bodies only after the complete sorted create batch exists.
        for &(id, e) in &r.rows {
            if let Some(joint) = world.get::<Joint>(e).copied() {
                let body_b =
                    r.bindings.get(&id).and_then(Binding::body).ok_or_else(|| {
                        PhysicsError("joint requires a rigid body or vehicle".into())
                    })?;
                let body_a = if joint.connected_body == 0 {
                    None
                } else {
                    Some(
                        r.bindings
                            .get(&joint.connected_body)
                            .and_then(Binding::body)
                            .ok_or_else(|| {
                                PhysicsError("connected PhysicsId has no rigid body".into())
                            })?,
                    )
                };
                let handle = physics.backend_mut().create_joint(JointDesc {
                    body_a,
                    body_b,
                    joint,
                })?;
                if let Some(b) = r.bindings.get_mut(&id) {
                    b.joint = Some(handle);
                    b.joint_settings = Some(joint);
                }
            }
        }
    }
    Ok(())
}

fn flush_bodies<B: PhysicsBackend>(backend: &mut B, r: &mut Runtime) -> Result<()> {
    if r.pending.is_empty() { return Ok(()); }
    r.spawned.clear();
    backend.create_bodies(&r.pending, &mut r.spawned)?;
    if r.pending.len() != r.spawned.len() {
        return Err(PhysicsError("backend returned an incomplete create batch".into()));
    }
    for (desc, &body) in r.pending.iter().zip(&r.spawned) {
        let binding = r.bindings.get_mut(&desc.user_data)
            .ok_or_else(|| PhysicsError("missing create binding".into()))?;
        binding.body = Some(body);
    }
    r.pending.clear();
    Ok(())
}
