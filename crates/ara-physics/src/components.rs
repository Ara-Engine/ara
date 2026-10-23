//! Reflected authoring data and replayable inputs. Runtime handles live separately.
use ara_schema::{Quat, Schema, SchemaDb};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Stable simulation identity; nonzero and unique within a physics world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PhysicsId {
    /// Stable scene/network identity, used to sort creation and inputs across peers.
    #[ara(default = 1, range(min = 1))]
    pub value: u64,
}
impl Default for PhysicsId {
    fn default() -> Self {
        Self { value: 1 }
    }
}

/// Resolved world-space pose in meters. Hierarchy/scale resolution runs before physics.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PhysicsPose {
    /// Body origin or character feet, in world space.
    #[ara(unit = Meters, replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub position: [f32; 3],
    /// Unit quaternion in XYZW order; use baked rotations for cross-platform replay.
    #[ara(replicate(audience = Relevant, interpolate = Slerp, rollback))]
    pub rotation: Quat,
}
impl Default for PhysicsPose {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: Quat([0.0, 0.0, 0.0, 1.0]),
        }
    }
}

mod body;
mod character;
mod joint;
mod vehicle;
pub use body::*;
pub use character::*;
pub use joint::*;
pub use vehicle::*;

/// Registers physics authoring components and all nested types in the schema DB.
pub fn register_schemas(db: &mut SchemaDb) {
    db.register::<PhysicsId>()
        .register::<PhysicsPose>()
        .register::<RigidBody>()
        .register::<Collider>()
        .register::<Velocity>()
        .register::<CharacterController>()
        .register::<CharacterInput>()
        .register::<Vehicle>()
        .register::<VehicleInput>()
        .register::<Joint>()
        .register::<crate::CharacterMotion>()
        .register::<crate::VehicleTelemetry>();
}
