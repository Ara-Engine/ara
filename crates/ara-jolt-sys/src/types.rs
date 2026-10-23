//! Plain data records shared with the C shim.
/// Three-dimensional SI vector.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraVec3 {
    /// X component.
    pub x: f32,
    /// Y component.
    pub y: f32,
    /// Z component.
    pub z: f32,
}

/// Unit quaternion in XYZW order.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraQuat {
    /// X component.
    pub x: f32,
    /// Y component.
    pub y: f32,
    /// Z component.
    pub z: f32,
    /// Scalar component.
    pub w: f32,
}

/// Allocation, collision and solver configuration.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraWorldDesc {
    /// Maximum simultaneous bodies.
    pub max_bodies: u32,
    /// Maximum broadphase pairs.
    pub max_pairs: u32,
    /// Maximum contact constraints.
    pub max_contacts: u32,
    /// Total simulation threads, including the caller.
    pub threads: u32,
    /// Initial scratch space in MiB, with malloc fallback.
    pub scratch_mb: u32,
    /// Velocity solver iterations; 15 holds the S6 tower.
    pub velocity_steps: u32,
    /// Position solver iterations.
    pub position_steps: u32,
    /// Maximum events per tick; zero disables contact collection.
    pub event_capacity: u32,
    /// Symmetric 16-layer collision matrix; layer zero is static.
    pub layer_masks: [u32; 16],
    /// Gravity in meters per second squared.
    pub gravity: AraVec3,
}

/// Shape construction parameters; geometry arrays are separate borrowed slices.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraShapeDesc {
    /// Box=0, sphere=1, capsule=2, cylinder=3, hull=4, mesh=5, heightfield=6, compound=7.
    pub kind: u32,
    /// Box half extents, or heightfield XYZ sample scale.
    pub size: AraVec3,
    /// Sphere, capsule or cylinder radius.
    pub radius: f32,
    /// Capsule cylinder half height or cylinder half height.
    pub half_height: f32,
    /// Heightfield offset.
    pub offset: AraVec3,
    /// Square heightfield edge sample count.
    pub sample_count: u32,
}

/// One immutable compound child, referencing an existing world-owned shape.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraShapePart {
    /// Existing shape handle.
    pub shape: u32,
    /// Local position.
    pub position: AraVec3,
    /// Local rotation.
    pub rotation: AraQuat,
}

/// Complete initial rigid body state.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraBodyDesc {
    /// Shape handle.
    pub shape: u32,
    /// Static=0, kinematic=1, dynamic=2.
    pub motion: u32,
    /// Collision layer in 0..16.
    pub layer: u32,
    /// Sensor bit 0, sleeping bit 1, linear-cast CCD bit 2.
    pub flags: u32,
    /// Body origin in meters.
    pub position: AraVec3,
    /// Unit rotation.
    pub rotation: AraQuat,
    /// Initial linear velocity.
    pub linear: AraVec3,
    /// Initial angular velocity.
    pub angular: AraVec3,
    /// Coulomb friction coefficient.
    pub friction: f32,
    /// Restitution in 0..1.
    pub restitution: f32,
    /// Dynamic mass in kg; zero uses shape density.
    pub mass: f32,
    /// Stable application identity, copied to queries and events.
    pub user_data: u64,
}

/// Bulk ECS extraction record, including stable body identity.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraBodyState {
    /// Body ID with sequence number.
    pub body: u32,
    /// Awake flag.
    pub active: u32,
    /// Body origin, rather than center of mass.
    pub position: AraVec3,
    /// Unit rotation.
    pub rotation: AraQuat,
    /// Linear velocity.
    pub linear: AraVec3,
    /// Angular velocity.
    pub angular: AraVec3,
}

/// Ordered runtime body intent, applied only at a tick boundary.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraBodyCommand {
    /// Body handle.
    pub body: u32,
    /// Teleport=0, kinematic target=1, velocity=2, impulse=3, force=4, wake=5.
    pub kind: u32,
    /// Target position or world impulse point.
    pub position: AraVec3,
    /// Target rotation.
    pub rotation: AraQuat,
    /// Linear velocity, impulse or force.
    pub linear: AraVec3,
    /// Angular velocity or angular impulse/torque.
    pub angular: AraVec3,
}

/// Narrowphase query, usable individually or in a batch.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraQuery {
    /// Ray=0, shape cast=1, overlap=2.
    pub kind: u32,
    /// Shape handle for shape casts and overlaps.
    pub shape: u32,
    /// Layer inclusion mask.
    pub layer_mask: u32,
    /// Include sensors when nonzero.
    pub sensors: u32,
    /// Optional ignored body, or INVALID.
    pub ignore: u32,
    /// Origin of the ray or shape (not its center of mass).
    pub position: AraVec3,
    /// Shape rotation.
    pub rotation: AraQuat,
    /// Complete displacement, not a normalized direction.
    pub displacement: AraVec3,
}

/// Deterministically sorted narrowphase hit.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraHit {
    /// Body handle.
    pub body: u32,
    /// Leaf subshape identity.
    pub subshape: u32,
    /// Fraction along displacement; zero for overlaps.
    pub fraction: f32,
    /// Penetration depth in meters for overlap/start-inside.
    pub depth: f32,
    /// World-space surface point.
    pub point: AraVec3,
    /// Outward normal of the hit body.
    pub normal: AraVec3,
    /// Stable application identity.
    pub user_data: u64,
}

/// Native contact notification; removals are completed from the previous contact cache.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraContact {
    /// First body, ordered by native ID.
    pub body_a: u32,
    /// Second body.
    pub body_b: u32,
    /// First leaf subshape.
    pub subshape_a: u32,
    /// Second leaf subshape.
    pub subshape_b: u32,
    /// Body=0, character=1 for the first actor.
    pub kind_a: u32,
    /// Body=0, character=1 for the second actor.
    pub kind_b: u32,
    /// Added=0, persisted=1, removed=2.
    pub phase: u32,
    /// Nonzero for trigger contacts.
    pub sensor: u32,
    /// Surface point.
    pub point: AraVec3,
    /// Normal from body A towards body B.
    pub normal: AraVec3,
}

/// Capsule character settings; position is at the feet.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraCharacterDesc {
    /// Standing capsule radius.
    pub radius: f32,
    /// Standing complete height.
    pub height: f32,
    /// Crouching complete height.
    pub crouch_height: f32,
    /// Walkable slope in degrees.
    pub max_slope_deg: f32,
    /// Maximum force exerted on dynamic bodies.
    pub max_strength: f32,
    /// Character mass in kilograms.
    pub mass: f32,
    /// Collision padding in meters.
    pub padding: f32,
    /// Step-up distance.
    pub step_up: f32,
    /// Stick-to-floor distance.
    pub stick_down: f32,
    /// Initial feet position.
    pub position: AraVec3,
    /// Collision layer for the character.
    pub layer: u32,
}

/// One replayable character input.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraCharacterInput {
    /// Character handle.
    pub character: u32,
    /// Desired horizontal velocity.
    pub desired: AraVec3,
    /// Jump launch speed; zero means no jump.
    pub jump_speed: f32,
    /// Desired crouch state, with standing clearance tested natively.
    pub crouch: u32,
}

/// All character state visible to ECS consumers.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraCharacterState {
    /// Feet position.
    pub position: AraVec3,
    /// Linear velocity.
    pub linear: AraVec3,
    /// Supporting body's surface velocity.
    pub ground_velocity: AraVec3,
    /// Supporting surface normal.
    pub ground_normal: AraVec3,
    /// Ground body or INVALID.
    pub ground_body: u32,
    /// OnGround=0, steep=1, unsupported=2, air=3.
    pub ground_state: u32,
    /// Actual crouch state (standing can be blocked).
    pub crouched: u32,
}

/// Four-wheel vehicle settings; forward is +Z.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraVehicleDesc {
    /// Initial body origin.
    pub position: AraVec3,
    /// Initial rotation.
    pub rotation: AraQuat,
    /// Chassis half extents.
    pub half_extents: AraVec3,
    /// Chassis mass in kg.
    pub mass: f32,
    /// Wheel radius.
    pub wheel_radius: f32,
    /// Wheel width.
    pub wheel_width: f32,
    /// FL, FR, RL, RR wheel offsets.
    pub wheel_offsets: [AraVec3; 4],
    /// Minimum suspension length.
    pub suspension_min: f32,
    /// Maximum suspension length.
    pub suspension_max: f32,
    /// Suspension frequency in Hz.
    pub suspension_frequency: f32,
    /// Suspension damping ratio.
    pub suspension_damping: f32,
    /// Front steering lock in degrees.
    pub max_steer_deg: f32,
    /// Peak engine torque in Nm.
    pub engine_max_torque: f32,
    /// Rear wheel drive when nonzero.
    pub rear_wheel_drive: u32,
    /// Chassis collision layer.
    pub layer: u32,
    /// Application identity of the chassis.
    pub user_data: u64,
}

/// One replayable driver input.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraVehicleInput {
    /// Vehicle handle.
    pub vehicle: u32,
    /// Throttle in -1..1.
    pub throttle: f32,
    /// Steering in -1..1.
    pub steer: f32,
    /// Brake in 0..1.
    pub brake: f32,
    /// Hand brake in 0..1.
    pub hand_brake: f32,
}

/// Drivetrain and wheel extraction record.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraVehicleState {
    /// Chassis body handle.
    pub chassis: u32,
    /// Wheels touching the ground.
    pub wheels_in_contact: u32,
    /// Engine revolutions per minute.
    pub engine_rpm: f32,
    /// Current gearbox gear.
    pub gear: i32,
    /// Wheel center positions in world space.
    pub wheel_positions: [AraVec3; 4],
}

/// Joint settings, with world-space anchors and orthonormal axes.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AraJointDesc {
    /// Fixed=0, hinge=1, slider=2, distance=3, point=4, six-DOF=5.
    pub kind: u32,
    /// First body; INVALID anchors to the world.
    pub body_a: u32,
    /// Second body.
    pub body_b: u32,
    /// Anchor on body A in world space.
    pub anchor_a: AraVec3,
    /// Anchor on body B in world space.
    pub anchor_b: AraVec3,
    /// Primary axis (hinge or slide direction).
    pub axis: AraVec3,
    /// Perpendicular normal axis.
    pub normal: AraVec3,
    /// XYZ translation and XYZ angular lower limits (meters, radians).
    pub lower: [f32; 6],
    /// XYZ translation and XYZ angular upper limits.
    pub upper: [f32; 6],
}
