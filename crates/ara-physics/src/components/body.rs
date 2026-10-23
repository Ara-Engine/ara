use ara_schema::AssetRef;
use ara_schema::Schema;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// How a rigid body moves; kinematic targets are runtime intents.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "physics")]
#[serde(rename_all = "camelCase")]
pub enum MotionType {
    /// Infinite mass, fixed scenery.
    #[default]
    Static,
    /// Infinite mass, moves to an explicit target each tick.
    Kinematic,
    /// Finite mass, simulated by the solver.
    Dynamic,
}

/// Rigid-body authoring settings; simulation state is written to pose and velocity.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RigidBody {
    /// Static, kinematic, or dynamic motion.
    pub motion: MotionType,
    /// Mass in kilograms; zero computes mass from the collider's 1000 kg/m3 density.
    #[ara(unit = Kilograms, range(min = 0))]
    pub mass: f32,
    /// Permit inactive bodies to sleep.
    #[ara(default = true)]
    pub allow_sleep: bool,
    /// Use swept collision detection for fast bodies.
    pub continuous: bool,
}
impl Default for RigidBody {
    fn default() -> Self {
        Self {
            motion: MotionType::Static,
            mass: 0.0,
            allow_sleep: true,
            continuous: false,
        }
    }
}

/// Primitive or cooked geometry selection. Cooked geometry is resolved by the asset worker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "physics")]
#[serde(rename_all = "camelCase")]
pub enum ColliderKind {
    /// Box using half extents.
    #[default]
    Box,
    /// Sphere using radius.
    Sphere,
    /// Y-axis capsule using cylinder half height and radius.
    Capsule,
    /// Y-axis cylinder using half height and radius.
    Cylinder,
    /// Immutable cooked hull, mesh, terrain or compound, attached as `CookedCollider`.
    Cooked,
}

/// Collision geometry, material, layer and trigger settings.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Collider {
    /// Primitive type or externally cooked shape.
    pub kind: ColliderKind,
    /// Box half extents; scale must be baked before physics extraction.
    #[ara(default = [0.5,0.5,0.5], unit = Meters, range(min = 0.0001))]
    pub half_extents: [f32; 3],
    /// Sphere, capsule or cylinder radius.
    #[ara(default = 0.5, unit = Meters, range(min = 0.0001))]
    pub radius: f32,
    /// Cylinder half height, excluding the capsule hemispheres.
    #[ara(default = 0.5, unit = Meters, range(min = 0))]
    pub half_height: f32,
    /// Cooked collision asset (`@path#id`); resolve into `CookedCollider` before ticking.
    pub asset: Option<AssetRef>,
    /// Zero selects static/moving defaults from the body; 1..15 select an explicit group.
    #[ara(range(min = 0, max = 15))]
    pub layer: u32,
    /// Report trigger events without a collision response.
    pub sensor: bool,
    /// Coulomb friction coefficient.
    #[ara(default = 0.6, range(min = 0))]
    pub friction: f32,
    /// Elasticity: zero is inelastic, one is perfectly elastic.
    #[ara(range(min = 0, max = 1))]
    pub restitution: f32,
}
impl Default for Collider {
    fn default() -> Self {
        Self {
            kind: ColliderKind::Box,
            half_extents: [0.5; 3],
            radius: 0.5,
            half_height: 0.5,
            asset: None,
            layer: 0,
            sensor: false,
            friction: 0.6,
            restitution: 0.0,
        }
    }
}

/// Authoritative velocity, synchronized at tick boundaries.
#[derive(Debug, Default, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Velocity {
    /// World-space linear velocity.
    #[ara(unit = MetersPerSecond,replicate(audience = Relevant,rollback))]
    pub linear: [f32; 3],
    /// World-space angular velocity.
    #[ara(unit = RadiansPerSecond,replicate(audience = Relevant,rollback))]
    pub angular: [f32; 3],
}
