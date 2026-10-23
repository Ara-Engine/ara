//! Reflected 2D physics vocabulary, mirroring ara-physics' 3D component split:
//! authoring settings (`RigidBody2d`, `Collider2d`, `CharacterController2d`), per-tick
//! intent (`CharacterInput2d`) and mirrored state (`PhysicsPose2d`, `Velocity2d`,
//! `CharacterMotion2d`). Backend handles never appear in these values.
use ara_schema::Schema;
use ara_sim::bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// World-space 2D pose, written back by the solver at every completed boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PhysicsPose2d {
    /// Body origin or character capsule centre, in world space.
    #[ara(unit = Meters, replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub position: [f32; 2],
    /// Counter-clockwise rotation about +Z.
    #[ara(unit = Radians, replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub rotation: f32,
}

/// Authoritative velocity, synchronized at tick boundaries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Velocity2d {
    /// World-space linear velocity.
    #[ara(unit = MetersPerSecond, replicate(audience = Relevant, rollback))]
    pub linear: [f32; 2],
    /// Counter-clockwise angular velocity.
    #[ara(unit = RadiansPerSecond, replicate(audience = Relevant, rollback))]
    pub angular: f32,
}

/// How a rigid body moves; kinematic bodies follow their written pose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "physics2d")]
#[serde(rename_all = "camelCase")]
pub enum MotionType2d {
    /// Infinite mass, fixed scenery.
    #[default]
    Static,
    /// Infinite mass; moves to its `PhysicsPose2d` each tick and pushes dynamic bodies.
    Kinematic,
    /// Finite mass, simulated by the solver.
    Dynamic,
}

/// Rigid-body authoring settings; simulation state is written to pose and velocity.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RigidBody2d {
    /// Static, kinematic, or dynamic motion.
    pub motion: MotionType2d,
    /// Mass in kilograms; zero computes mass from the collider's 1 kg/m2 area density.
    #[ara(unit = Kilograms, range(min = 0))]
    pub mass: f32,
    /// Permit inactive bodies to sleep.
    #[ara(default = true)]
    pub allow_sleep: bool,
    /// Use swept collision detection for fast bodies.
    pub continuous: bool,
    /// Prevent physical rotation, for upright props and platforms.
    pub lock_rotation: bool,
}
impl Default for RigidBody2d {
    fn default() -> Self {
        Self {
            motion: MotionType2d::Static,
            mass: 0.,
            allow_sleep: true,
            continuous: false,
            lock_rotation: false,
        }
    }
}

/// Convex 2D geometry selection; 2D never reuses the 3D shape vocabulary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "physics2d")]
#[serde(rename_all = "camelCase")]
pub enum ColliderKind2d {
    /// Box using half extents.
    #[default]
    Box,
    /// Circle using radius.
    Circle,
    /// Y-axis capsule using segment half height and radius.
    Capsule,
    /// Convex hull of `points` (3 to 64 local-space points).
    Polygon,
}

/// Collision geometry, material, layer and trigger settings for a `RigidBody2d`.
#[derive(Debug, Clone, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Collider2d {
    /// Primitive type or convex polygon.
    pub kind: ColliderKind2d,
    /// Box half extents; scale must be baked before physics extraction.
    #[ara(default = [0.5, 0.5], unit = Meters, range(min = 0.0001))]
    pub half_extents: [f32; 2],
    /// Circle or capsule radius.
    #[ara(default = 0.5, unit = Meters, range(min = 0.0001))]
    pub radius: f32,
    /// Capsule segment half height, excluding the end caps.
    #[ara(default = 0.5, unit = Meters, range(min = 0))]
    pub half_height: f32,
    /// Local convex hull input for polygons, in metres.
    pub points: Vec<[f32; 2]>,
    /// Zero selects static/moving defaults from the body; 1..15 select an explicit group.
    #[ara(range(min = 0, max = 15))]
    pub layer: u32,
    /// Report overlaps without a collision response.
    pub sensor: bool,
    /// Coulomb friction coefficient.
    #[ara(default = 0.6, range(min = 0))]
    pub friction: f32,
    /// Elasticity: zero is inelastic, one is perfectly elastic.
    #[ara(range(min = 0, max = 1))]
    pub restitution: f32,
}
impl Default for Collider2d {
    fn default() -> Self {
        Self {
            kind: ColliderKind2d::Box,
            half_extents: [0.5; 2],
            radius: 0.5,
            half_height: 0.5,
            points: Vec::new(),
            layer: 0,
            sensor: false,
            friction: 0.6,
            restitution: 0.,
        }
    }
}

/// Kinematic capsule character: slopes, steps, ground snapping and pushing.
/// The entity needs no RigidBody2d/Collider2d; the controller owns its capsule.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterController2d {
    /// Capsule radius.
    #[ara(default = 0.3, unit = Meters, range(min = 0.001))]
    pub radius: f32,
    /// Complete capsule height; must exceed twice the radius.
    #[ara(default = 1.8, unit = Meters, range(min = 0.001))]
    pub height: f32,
    /// Maximum walkable slope angle.
    #[ara(default = 45, unit = Degrees, range(min = 0.1, max = 89.9))]
    pub max_slope_deg: f32,
    /// Maximum step height climbed automatically; zero disables stepping.
    #[ara(default = 0.25, unit = Meters, range(min = 0))]
    pub step_height: f32,
    /// Distance searched below the feet to stay on descending ground; zero disables it.
    #[ara(default = 0.2, unit = Meters, range(min = 0))]
    pub stick_down: f32,
    /// Collision padding kept between the capsule and the world.
    #[ara(default = 0.01, unit = Meters, range(min = 0.0001))]
    pub padding: f32,
    /// Collision group used for movement queries.
    #[ara(default = 1, range(min = 1, max = 15))]
    pub layer: u32,
}
impl Default for CharacterController2d {
    fn default() -> Self {
        Self {
            radius: 0.3,
            height: 1.8,
            max_slope_deg: 45.,
            step_height: 0.25,
            stick_down: 0.2,
            padding: 0.01,
            layer: 1,
        }
    }
}

/// Per-tick character intent, read exactly once by each boundary. Gameplay writes it
/// before the physics system; it is not cleared, so held input repeats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterInput2d {
    /// Desired world-space velocity. With gravity it must be perpendicular to gravity
    /// (vertical motion comes from gravity and jumps); zero-gravity worlds use both axes.
    #[ara(unit = MetersPerSecond, replicate(audience = Owner, rollback))]
    pub desired_velocity: [f32; 2],
    /// Jump launch speed, applied only while grounded; zero means no jump this tick.
    #[ara(unit = MetersPerSecond, range(min = 0), replicate(audience = Owner, rollback))]
    pub jump_speed: f32,
}

/// Character support mirrored at every fixed boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "physics2d")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterMotion2d {
    /// Walkable ground support after the last move.
    #[ara(replicate(audience = Owner, rollback))]
    pub grounded: bool,
    /// Supporting surface normal; zero while airborne.
    #[ara(replicate(audience = Owner, rollback))]
    pub ground_normal: [f32; 2],
}
