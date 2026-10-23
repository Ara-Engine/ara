//! Jolt physics in the fixed tick. Bodies are synchronized at the tick boundary
//! (`ara_physics::tick_ecs`); ECS mirrors (pose, velocity, character motion) are
//! registered components, and the native world is a checkpoint provider: exact native
//! checkpoints in-process, rebuilt from the mirrors when a portable snapshot is restored.
use crate::{Result, RuntimeError, math::Pose};
use ara_physics::{
    CharacterController, CookedCollider, EcsPhysicsSnapshot, PhysicsCommands, PhysicsConfig,
    PhysicsPose, PhysicsStatus, RigidBody, Vehicle,
};
use ara_physics_jolt::{JoltBackend, JoltSnapshot};
use ara_sim::{SimError, StateProvider, TrustedState};
use bevy_ecs::prelude::*;
use serde_json::Value;
use std::{any::TypeId, sync::Arc};

/// Installs the Jolt world and gives every body without a `PhysicsPose` one from its
/// world transform, then builds native bodies without advancing time.
pub fn install(world: &mut World, config: &PhysicsConfig, poses: &[(Entity, Pose)]) -> Result<()> {
    for &(entity, pose) in poses {
        let simulated = world.get::<RigidBody>(entity).is_some()
            || world.get::<CharacterController>(entity).is_some()
            || world.get::<Vehicle>(entity).is_some();
        if simulated && world.get::<PhysicsPose>(entity).is_none() {
            world.entity_mut(entity).insert(pose.to_physics());
        }
    }
    build(world, config)
}

/// Poses simulated bodies spawned after build (streamed world cells) at their
/// transform before the next physics push, as `install` does at build.
pub fn adopt(world: &mut World) {
    let mut new = Vec::new();
    let mut query = world.query_filtered::<
        (Entity, &ara_components::Transform),
        (
            Without<PhysicsPose>,
            Or<(With<RigidBody>, With<CharacterController>, With<Vehicle>)>,
        ),
    >();
    for (entity, transform) in query.iter(world) {
        new.push((entity, Pose::from_transform(transform)));
    }
    for (entity, pose) in new {
        let pose = world
            .get::<crate::sync::WorldPose>(entity)
            .map_or(pose, |w| w.0);
        world.entity_mut(entity).insert(pose.to_physics());
    }
}

fn build(world: &mut World, config: &PhysicsConfig) -> Result<()> {
    ara_physics::register::<JoltBackend>(world, &mut Schedule::default(), config.clone())?;
    // Cooked shapes are handles into the native world just (re)created.
    let mut stale = Vec::new();
    let mut query = world
        .query_filtered::<Entity, (With<crate::terrain::TerrainTile>, With<CookedCollider>)>();
    stale.extend(query.iter(world));
    for entity in stale {
        world.entity_mut(entity).remove::<CookedCollider>();
    }
    crate::terrain::cook_tiles(world)?;
    ara_physics::prepare_ecs::<JoltBackend>(world)?;
    Ok(())
}

/// Drops the native world and every physics infrastructure resource.
fn teardown(world: &mut World) {
    for ty in ara_physics::infrastructure_type_ids::<JoltBackend>() {
        if let Some(id) = world.components().get_id(ty) {
            world.remove_resource_by_id(id);
        }
    }
}

/// The fixed-tick physics boundary. A failure is sticky in `PhysicsStatus`; the
/// runtime reports it and stops advancing that session.
pub fn step(world: &mut World) {
    if world.resource::<PhysicsStatus>().error.is_some() {
        return;
    }
    if let Err(error) = ara_physics::tick_ecs::<JoltBackend>(world) {
        world.resource_mut::<PhysicsStatus>().error = Some(error);
    }
}

/// Snapshot provider for the native physics world.
pub struct PhysicsProvider {
    /// Configuration used to rebuild the world on portable restore.
    pub config: PhysicsConfig,
}

impl StateProvider for PhysicsProvider {
    fn name(&self) -> &'static str {
        "physics"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        let mut types = ara_physics::infrastructure_type_ids::<JoltBackend>().to_vec();
        types.push(TypeId::of::<CookedCollider>());
        types
    }
    fn capture(&self, world: &World) -> std::result::Result<Value, SimError> {
        if world
            .get_resource::<PhysicsCommands>()
            .is_some_and(|c| !c.bodies.is_empty())
        {
            return Err(SimError(
                "physics intents are queued between ticks; submit them inside a tick".into(),
            ));
        }
        if let Some(error) = world
            .get_resource::<PhysicsStatus>()
            .and_then(|s| s.error.clone())
        {
            return Err(SimError(format!("physics is faulted: {error}")));
        }
        // Poses, velocities and character state are registered ECS mirrors.
        Ok(Value::Null)
    }
    fn restore(&self, world: &mut World, _: &Value) -> std::result::Result<(), SimError> {
        teardown(world);
        build(world, &self.config).map_err(|e| SimError(e.0))
    }
    fn checkpoint(&self, world: &mut World) -> std::result::Result<Option<TrustedState>, SimError> {
        let snapshot =
            ara_physics::snapshot_ecs::<JoltBackend>(world).map_err(|e| SimError(e.to_string()))?;
        Ok(Some(Arc::new(snapshot)))
    }
    fn restore_checkpoint(
        &self,
        world: &mut World,
        state: &Value,
        trusted: &TrustedState,
    ) -> std::result::Result<(), SimError> {
        let snapshot = trusted
            .downcast_ref::<EcsPhysicsSnapshot<JoltSnapshot>>()
            .ok_or_else(|| SimError("foreign physics checkpoint".into()))?;
        // A changed body topology cannot take the native state; rebuild instead.
        if ara_physics::restore_ecs::<JoltBackend>(world, snapshot).is_err() {
            return self.restore(world, state);
        }
        Ok(())
    }
}

impl From<ara_physics::PhysicsError> for RuntimeError {
    fn from(e: ara_physics::PhysicsError) -> Self {
        Self(e.0)
    }
}
