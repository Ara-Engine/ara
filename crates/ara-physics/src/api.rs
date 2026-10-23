//! Backend-neutral runtime descriptors. All dimensions are SI and rotations XYZW.
use crate::{CharacterController, Collider, PhysicsPose, RigidBody, Vehicle, Velocity};
use bevy_ecs::prelude::Component;

macro_rules! handle {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Component)]
        pub struct $name(u32);
        impl $name {
            /// Constructs a backend handle. Operations validate it against their world.
            pub const fn from_raw(raw: u32) -> Self {
                Self(raw)
            }
            /// Backend identity; never persist or replicate this value.
            pub const fn raw(self) -> u32 {
                self.0
            }
        }
    };
}
handle!(
    BodyHandle,
    "World-scoped runtime body identity, including its reuse generation."
);
handle!(
    ShapeHandle,
    "Immutable geometry owned by one backend world."
);
handle!(
    CharacterHandle,
    "World-scoped capsule character identity; never reused."
);
handle!(
    VehicleHandle,
    "World-scoped vehicle identity; never reused."
);
handle!(
    JointHandle,
    "World-scoped constraint identity; never reused."
);

/// Runtime asset resolution result; the authoring reference remains in `Collider.asset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Component)]
pub struct CookedCollider(pub ShapeHandle);

/// Immutable geometry to cook on the importer/worker thread, then install at a boundary.
#[derive(Debug, Clone, Copy)]
pub enum Shape<'a> {
    /// Box half extents.
    Box {
        /// Half size along each axis.
        half_extents: [f32; 3],
    },
    /// Sphere radius.
    Sphere {
        /// Radius in meters.
        radius: f32,
    },
    /// Y-axis capsule.
    Capsule {
        /// Hemisphere radius.
        radius: f32,
        /// Half height of its cylindrical section.
        half_height: f32,
    },
    /// Y-axis cylinder.
    Cylinder {
        /// Radius in meters.
        radius: f32,
        /// Half height in meters.
        half_height: f32,
    },
    /// Convex hull generation from an unordered point cloud.
    ConvexHull {
        /// Finite model-space positions.
        vertices: &'a [[f32; 3]],
    },
    /// Static triangle mesh. Winding is counterclockwise from the colliding side.
    Mesh {
        /// Model-space positions.
        vertices: &'a [[f32; 3]],
        /// Triangle triplets, flattened.
        indices: &'a [u32],
    },
    /// Square, row-major heightfield (static only).
    HeightField {
        /// Square edge sample count, at least four.
        side: u32,
        /// Exactly side*side samples.
        heights: &'a [f32],
        /// XYZ sample scale.
        scale: [f32; 3],
        /// Local origin offset.
        offset: [f32; 3],
    },
    /// Immutable compound of existing shapes; may be dynamic when all children permit it.
    Compound {
        /// Nonempty child list.
        parts: &'a [ShapePart],
    },
}

/// One compound child referencing an already cooked shape in the same world.
#[derive(Debug, Clone, Copy)]
pub struct ShapePart {
    /// World-owned immutable shape.
    pub shape: ShapeHandle,
    /// Local transform.
    pub pose: PhysicsPose,
}

/// Initial rigid-body description, extracted from authoring components.
#[derive(Debug, Clone)]
pub struct BodyDesc {
    /// Cooked shape in the target world.
    pub shape: ShapeHandle,
    /// Stable application identity, independent of backend handles.
    pub user_data: u64,
    /// Initial world-space pose.
    pub pose: PhysicsPose,
    /// Motion and mass settings.
    pub body: RigidBody,
    /// Material, layer and trigger settings (shape fields are ignored here).
    pub collider: Collider,
    /// Initial linear and angular velocities.
    pub velocity: Velocity,
}

/// Extracted authoritative body state, in caller-defined order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyState {
    /// Body identity.
    pub body: BodyHandle,
    /// Body origin and rotation.
    pub pose: PhysicsPose,
    /// Authoritative velocity.
    pub velocity: Velocity,
    /// Whether the body is currently simulated rather than asleep.
    pub active: bool,
}

/// Typed runtime intent; these never edit authoring documents.
#[derive(Debug, Clone, Copy)]
pub enum BodyCommand {
    /// Explicitly teleport an entity and invalidate its contact cache.
    Teleport {
        /// Target body.
        body: BodyHandle,
        /// New world-space pose.
        pose: PhysicsPose,
    },
    /// Move a kinematic body over the upcoming fixed tick; riders inherit velocity.
    KinematicTarget {
        /// Target kinematic body.
        body: BodyHandle,
        /// Desired pose at the end of the tick.
        pose: PhysicsPose,
    },
    /// Set authoritative velocity.
    Velocity {
        /// Target body.
        body: BodyHandle,
        /// Linear and angular velocity.
        velocity: Velocity,
    },
    /// Apply an impulse at a world-space point and an additional angular impulse.
    Impulse {
        /// Target dynamic body.
        body: BodyHandle,
        /// Linear impulse in kg*m/s.
        impulse: [f32; 3],
        /// World-space application point.
        point: [f32; 3],
        /// Angular impulse in kg*m2/s.
        angular: [f32; 3],
    },
    /// Apply force/torque during one fixed tick.
    Force {
        /// Target dynamic body.
        body: BodyHandle,
        /// Force in newtons.
        force: [f32; 3],
        /// Torque in newton-meters.
        torque: [f32; 3],
    },
    /// Wake a body.
    Wake(BodyHandle),
}

/// Character spawn description, with a foot-space pose and stable application identity.
#[derive(Debug, Clone, Copy)]
pub struct CharacterDesc {
    /// Stable application identity, retained in contact effects after despawn.
    pub user_data: u64,
    /// Initial feet position; capsules remain upright.
    pub position: [f32; 3],
    /// Predicted movement configuration.
    pub controller: CharacterController,
}

/// Character state mirrored into ECS after its step. Native contacts are in the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Component)]
pub struct CharacterState {
    /// Foot position.
    pub position: [f32; 3],
    /// Authoritative movement velocity.
    pub velocity: [f32; 3],
    /// Supporting platform surface velocity.
    pub ground_velocity: [f32; 3],
    /// Supporting surface normal.
    pub ground_normal: [f32; 3],
    /// Supporting body, if any.
    pub ground_body: Option<BodyHandle>,
    /// Walkable support rather than steep/airborne contact.
    pub grounded: bool,
    /// Actual crouch state, including failed stand-up requests.
    pub crouched: bool,
}

/// Complete car spawn description. Wheel offsets allow custom chassis layouts.
#[derive(Debug, Clone, Copy)]
pub struct VehicleDesc {
    /// Body origin/rotation at spawn.
    pub pose: PhysicsPose,
    /// Stable chassis application identity.
    pub user_data: u64,
    /// Engine, suspension and wheel settings.
    pub vehicle: Vehicle,
    /// Local FL, FR, RL, RR attachment positions.
    pub wheel_offsets: [[f32; 3]; 4],
}
impl VehicleDesc {
    /// S6-proven wheel placement for the specified chassis dimensions.
    pub fn new(vehicle: Vehicle, pose: PhysicsPose, user_data: u64) -> Self {
        let [x, y, z] = vehicle.half_extents;
        Self {
            pose,
            user_data,
            vehicle,
            wheel_offsets: [
                [x, -0.9 * y, z - 0.6],
                [-x, -0.9 * y, z - 0.6],
                [x, -0.9 * y, -z + 0.6],
                [-x, -0.9 * y, -z + 0.6],
            ],
        }
    }
}

/// Vehicle drivetrain and wheel state, mirrored into ECS.
#[derive(Debug, Clone, Copy, PartialEq, Component)]
pub struct VehicleState {
    /// Chassis body identity.
    pub chassis: BodyHandle,
    /// Number of grounded wheels.
    pub wheels_in_contact: u32,
    /// Engine revolutions per minute.
    pub engine_rpm: f32,
    /// Automatic gearbox gear.
    pub gear: i32,
    /// Wheel centers in world space.
    pub wheel_positions: [[f32; 3]; 4],
}

/// Runtime joint creation. The reflected `Joint` contains world-space anchor settings.
#[derive(Debug, Clone, Copy)]
pub struct JointDesc {
    /// Connected body, or the fixed world.
    pub body_a: Option<BodyHandle>,
    /// Body carrying the joint.
    pub body_b: BodyHandle,
    /// Anchors, axes and limit settings.
    pub joint: crate::Joint,
}
