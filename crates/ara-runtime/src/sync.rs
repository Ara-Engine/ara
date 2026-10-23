//! Transform ↔ physics synchronization and world poses for interpolated presentation.
//!
//! - Before physics, a body whose `Transform` differs from the value physics last wrote
//!   (a live patch, a script teleport) is teleported to it.
//! - After physics, moving bodies and characters write their poses back to `Transform`.
//! - At tick end every entity's world pose is resolved; the previous tick's is kept so
//!   presentation interpolates between them. These are derived, provider-owned values.
use crate::{
    animation::AnimatorSlot,
    components::{Animator, PlayerControl},
    math::Pose,
};
use ara_components::Transform;
use ara_nav::NavAgent;
use ara_physics::{CharacterController, MotionType, PhysicsPose, RigidBody};
use ara_sim::{SimError, StateProvider};
use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use std::{any::TypeId, collections::BTreeMap};

/// World pose resolved at the end of the latest tick.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WorldPose(pub Pose);

/// World pose at the end of the previous tick.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PreviousPose(pub Pose);

/// The `Transform` physics last wrote; a different value means an external edit.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct SyncedTransform(pub Transform);

/// Resolves world matrices parent-first for every entity with a `Transform`.
pub fn world_matrices(world: &mut World) -> BTreeMap<Entity, Mat4> {
    let mut query = world.query::<(Entity, &Transform, Option<&ChildOf>)>();
    let rows: BTreeMap<Entity, (Mat4, Option<Entity>)> = query
        .iter(world)
        .map(|(e, t, p)| {
            (
                e,
                (Pose::from_transform(t).matrix(), p.map(ChildOf::parent)),
            )
        })
        .collect();
    let mut out = BTreeMap::new();
    for &entity in rows.keys() {
        resolve(entity, &rows, &mut out);
    }
    out
}

fn resolve(
    entity: Entity,
    rows: &BTreeMap<Entity, (Mat4, Option<Entity>)>,
    out: &mut BTreeMap<Entity, Mat4>,
) -> Mat4 {
    if let Some(m) = out.get(&entity) {
        return *m;
    }
    let Some(&(local, parent)) = rows.get(&entity) else {
        return Mat4::IDENTITY;
    };
    let world = match parent {
        Some(p) => resolve(p, rows, out) * local,
        None => local,
    };
    out.insert(entity, world);
    world
}

/// Start of tick: the last resolved pose becomes the previous one.
pub fn remember_poses(mut query: Query<(&WorldPose, &mut PreviousPose)>) {
    for (current, mut previous) in &mut query {
        previous.0 = current.0;
    }
}

/// End of tick: resolves world poses (inserting pose components for new entities).
pub fn resolve_poses(world: &mut World) {
    for (entity, matrix) in world_matrices(world) {
        let pose = WorldPose(Pose::from_matrix(matrix));
        let mut entity = world.entity_mut(entity);
        if entity.contains::<WorldPose>() {
            entity.insert(pose);
        } else {
            entity.insert((pose, PreviousPose(pose.0)));
        }
    }
}

type Bodies<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static mut PhysicsPose,
        Option<&'static SyncedTransform>,
        Option<&'static ChildOf>,
    ),
    (Without<NavAgent>, With<PhysicsPose>),
>;

/// Before physics: teleports bodies whose `Transform` was edited since physics last
/// wrote it (live patches, scripts).
pub fn push_poses(mut bodies: Bodies, parents: Query<&WorldPose>) {
    for (transform, mut pose, synced, parent) in &mut bodies {
        if synced.is_some_and(|s| s.0 == *transform) {
            continue;
        }
        let local = Pose::from_transform(transform).matrix();
        let world = parent
            .and_then(|p| parents.get(p.parent()).ok())
            .map_or(local, |p| p.0.matrix() * local);
        let target = Pose::from_matrix(world).to_physics();
        if synced.is_some() || *pose != target {
            *pose = target;
        }
    }
}

type RootMotion<'w, 's> = Query<
    'w,
    's,
    (
        &'static Animator,
        &'static AnimatorSlot,
        &'static mut Transform,
    ),
    (
        Without<CharacterController>,
        Without<NavAgent>,
        Without<PhysicsPose>,
    ),
>;

/// Moves `rootMotion` animators by their extracted root displacement (entities whose
/// movement no controller, agent or body owns).
pub fn apply_root_motion(mut animated: RootMotion) {
    for (animator, slot, mut transform) in &mut animated {
        if !animator.root_motion {
            continue;
        }
        let mut pose = Pose::from_transform(&transform);
        let delta = Vec3::from_array(slot.root.translation.to_array()) * pose.scale;
        pose.translation += pose.rotation * delta;
        pose.rotation =
            (pose.rotation * Quat::from_array(slot.root.rotation.to_array())).normalize();
        *transform = pose.to_transform();
    }
}

type Moving<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static PhysicsPose,
        &'static mut Transform,
        Option<&'static mut SyncedTransform>,
        Option<&'static RigidBody>,
        Option<&'static PlayerControl>,
        Option<&'static ChildOf>,
    ),
    (
        Without<NavAgent>,
        Or<(With<RigidBody>, With<CharacterController>)>,
    ),
>;

/// After physics: writes moving bodies' and characters' poses to `Transform` and
/// records what was written. Characters face their `PlayerControl.yaw` (the
/// controller capsule stays upright). Static bodies keep their authored transform.
pub fn pull_poses(mut commands: Commands, mut bodies: Moving, parents: Query<&WorldPose>) {
    for (entity, pose, mut transform, synced, body, control, parent) in &mut bodies {
        if !body.is_some_and(|b| b.motion == MotionType::Static) {
            let scale = Vec3::from_array(transform.scale);
            let mut world = Pose::from_physics(pose, scale);
            if let Some(control) = control {
                world.rotation = crate::math::yaw_rotation(control.yaw);
            }
            let local = match parent.and_then(|p| parents.get(p.parent()).ok()) {
                Some(p) => Pose::from_matrix(p.0.matrix().inverse() * world.matrix()),
                None => world,
            };
            *transform = Pose { scale, ..local }.to_transform();
        }
        match synced {
            Some(mut synced) => synced.0 = *transform,
            // Deferred only for bodies seen for the first time.
            None => {
                commands.entity(entity).insert(SyncedTransform(*transform));
            }
        }
    }
}

/// Owns the derived pose components; restore rebuilds them from `Transform`.
pub struct PoseProvider;

impl StateProvider for PoseProvider {
    fn name(&self) -> &'static str {
        "poses"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![
            TypeId::of::<WorldPose>(),
            TypeId::of::<PreviousPose>(),
            TypeId::of::<SyncedTransform>(),
        ]
    }
    fn capture(&self, _: &World) -> Result<Value, SimError> {
        Ok(Value::Null)
    }
    fn restore(&self, world: &mut World, _: &Value) -> Result<(), SimError> {
        for (entity, matrix) in world_matrices(world) {
            let pose = Pose::from_matrix(matrix);
            world
                .entity_mut(entity)
                .insert((WorldPose(pose), PreviousPose(pose)));
        }
        let mut synced = Vec::new();
        let mut query = world.query_filtered::<(Entity, &Transform), With<SyncedTransform>>();
        for (entity, transform) in query.iter(world) {
            synced.push((entity, *transform));
        }
        for (entity, transform) in synced {
            world.entity_mut(entity).insert(SyncedTransform(transform));
        }
        Ok(())
    }
}
