//! Persistent query plans, matching normal scheduled ECS systems.
use super::baseline::{Position, Rotation, StableId, Velocity};
use bevy_ecs::prelude::*;

/// Matching metadata is setup work; row counts and archetypes still refresh at use.
pub(super) struct CachedQueries {
    /// W1 translation, velocity and quaternion access.
    pub transforms: QueryState<(
        &'static mut Position,
        &'static Velocity,
        &'static mut Rotation,
    )>,
    /// Read-only Jacobi input gathered before W2 updates.
    pub snapshot: QueryState<(&'static StableId, &'static Position, &'static Velocity)>,
    /// W2 identity and writable output columns.
    pub boids: QueryState<(
        &'static StableId,
        &'static mut Position,
        &'static mut Velocity,
    )>,
    /// W3 writable particle columns.
    pub particles: QueryState<(&'static mut Position, &'static mut Velocity)>,
}

impl CachedQueries {
    /// Resolves all kernel access once after the initial world is populated.
    pub fn new(world: &mut World) -> Self {
        Self {
            transforms: world.query(),
            snapshot: world.query(),
            boids: world.query(),
            particles: world.query(),
        }
    }
}
