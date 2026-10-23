#![warn(missing_docs)]
//! Owned 2D physics boundary. Gameplay state is in reflected ECS components.
pub mod commands;
mod components;
mod shape;
mod solver;
pub use components::*;
pub use solver::{Physics2d, RayHit2d, step_system};

/// Registers reflected authoritative physics components.
pub fn register(db: &mut ara_schema::SchemaDb) {
    db.register::<Body2d>()
        .register::<Collider2d>()
        .register::<Character2d>();
}
/// Registers component codecs and the complete rollback-safe solver resource.
pub fn register_sim(registry: &mut ara_sim::SimRegistry) -> Result<(), ara_sim::SimError> {
    registry.register_component::<Body2d>()?;
    registry.register_component::<Collider2d>()?;
    registry.register_component::<Character2d>()?;
    registry.register_component::<commands::TileColliderSource2d>()?;
    registry.register_resource::<Physics2d>()
}
