#![expect(
    clippy::approx_constant,
    reason = "Schema defaults require JSON literals; the controller's default slope is pi/4."
)]
use ara_schema::Schema;
use ara_sim::bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Rigid body integration policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BodyKind2d {
    /// Infinite-mass authored geometry.
    #[default]
    Fixed,
    /// Solver-driven body affected by gravity and contacts.
    Dynamic,
    /// Position-driven mover, including character controllers and platforms.
    Kinematic,
}
/// Authoritative 2D pose and velocity, read/written at fixed boundaries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Body2d {
    /// Integration policy.
    pub kind: BodyKind2d,
    /// World position in metres, independent of presentation hierarchy.
    pub position: [f32; 2],
    /// Rotation in radians counter-clockwise.
    pub rotation: f32,
    /// Linear velocity in metres per second.
    pub velocity: [f32; 2],
    /// Angular velocity in radians per second.
    pub angular_velocity: f32,
    /// Prevent physical rotation, useful for characters.
    pub lock_rotation: bool,
    /// Enable swept continuous collision detection for fast movers.
    pub ccd: bool,
}
/// Collider geometry selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Shape2d {
    /// Axis-aligned local box, rotated by its body.
    #[default]
    Box,
    /// Circle using radius.
    Circle,
    /// Vertical capsule using radius and half-height of the centre segment.
    Capsule,
    /// Convex hull of polygon points.
    Polygon,
}
/// Convex collider attached to the same entity's Body2d.
#[derive(Debug, Clone, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Collider2d {
    /// Geometry selection.
    pub shape: Shape2d,
    /// Local half extents for box geometry, in metres.
    #[ara(default = [0.5, 0.5])]
    pub half_extents: [f32; 2],
    /// Circle/capsule radius in metres.
    #[ara(default = 0.5)]
    pub radius: f32,
    /// Capsule centre-segment half-height in metres.
    #[ara(default = 0.5)]
    pub half_height: f32,
    /// Local polygon hull input, in metres; 3..64 finite points.
    pub points: Vec<[f32; 2]>,
    /// Area density in kilograms per square metre.
    #[ara(default = 1)]
    pub density: f32,
    /// Coulomb friction coefficient.
    #[ara(default = 0.5)]
    pub friction: f32,
    /// Restitution in [0,1].
    pub restitution: f32,
    /// Detect overlaps without contact response.
    pub sensor: bool,
    /// Collision membership bitfield.
    #[ara(default = 4294967295_u64)]
    pub memberships: u32,
    /// Accepted collision membership bitfield.
    #[ara(default = 4294967295_u64)]
    pub filter: u32,
}
impl Default for Collider2d {
    fn default() -> Self {
        Self {
            shape: Shape2d::Box,
            half_extents: [0.5; 2],
            radius: 0.5,
            half_height: 0.5,
            points: Vec::new(),
            density: 1.,
            friction: 0.5,
            restitution: 0.,
            sensor: false,
            memberships: u32::MAX,
            filter: u32::MAX,
        }
    }
}
/// Kinematic controller input/output. Gravity and jumping belong to gameplay systems.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Character2d {
    /// Requested movement this fixed tick, in metres; consumed on successful step.
    pub translation: [f32; 2],
    /// Last movement was grounded; snapshotted for gameplay decisions.
    pub grounded: bool,
    /// Maximum climbable slope in radians.
    #[ara(default = 0.7853982)]
    pub max_slope: f32,
    /// Maximum step height in metres; zero disables stepping.
    #[ara(default = 0.25)]
    pub step_height: f32,
    /// Ground snap distance in metres; zero disables snapping.
    #[ara(default = 0.1)]
    pub snap: f32,
}
impl Default for Character2d {
    fn default() -> Self {
        Self {
            translation: [0.; 2],
            grounded: false,
            max_slope: std::f32::consts::FRAC_PI_4,
            step_height: 0.25,
            snap: 0.1,
        }
    }
}
