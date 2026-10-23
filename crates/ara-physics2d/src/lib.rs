#![warn(missing_docs)]
//! Owned 2D physics boundary over Rapier2D, mirroring ara-physics' component split.
//! Gameplay state is in reflected ECS components; the solver is checkpointed infrastructure.
pub mod commands;
mod components;
mod ecs;
mod shape;
mod world;
pub use components::*;
pub use ecs::{Physics2dStatus, install, prepare_ecs, step_system, tick_ecs};
pub use world::{Physics2d, Physics2dConfig, RayHit2d};

/// Registers reflected authoring, input and mirrored-state components.
pub fn register(db: &mut ara_schema::SchemaDb) {
    db.register::<PhysicsPose2d>()
        .register::<Velocity2d>()
        .register::<RigidBody2d>()
        .register::<Collider2d>()
        .register::<CharacterController2d>()
        .register::<CharacterInput2d>()
        .register::<CharacterMotion2d>();
}
/// Registers component codecs and the complete rollback-safe solver resource.
pub fn register_sim(registry: &mut ara_sim::SimRegistry) -> Result<(), ara_sim::SimError> {
    registry.register_component::<PhysicsPose2d>()?;
    registry.register_component::<Velocity2d>()?;
    registry.register_component::<RigidBody2d>()?;
    registry.register_component::<Collider2d>()?;
    registry.register_component::<CharacterController2d>()?;
    registry.register_component::<CharacterInput2d>()?;
    registry.register_component::<CharacterMotion2d>()?;
    registry.register_component::<commands::TileColliderSource2d>()?;
    registry.register_resource::<Physics2d>()?;
    registry.register_resource::<Physics2dStatus>()
}
