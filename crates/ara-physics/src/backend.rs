//! Static-dispatch boundary. A future 2D backend has its own geometry/dimension contract.
use crate::*;
use ara_diag::Diagnostic;

/// Physics errors have stable diagnostics suitable for the editor and protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicsError(pub String);
impl std::fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PhysicsError {}
impl PhysicsError {
    /// Reports a rejected operation without writing any authoring document.
    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error(ara_diag::Code(7001), self.0.as_str())
    }
}
/// Fallible physics operation.
pub type Result<T> = std::result::Result<T, PhysicsError>;

/// Backend capabilities; a missing feature is rejected, never silently approximated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// The full 3D body/shape/query set in this contract.
    RigidBodies3d,
    /// Six constraint kinds.
    Joints,
    /// Capsule character movement feature.
    Character,
    /// Engine/gearbox four-wheel vehicle.
    WheeledVehicle,
    /// Physics and character rollback.
    Snapshot,
    /// Future M4 two-dimensional toolset; unsupported by the 3D backend.
    Physics2d,
    /// Optional tracked vehicles; unsupported unless implemented explicitly.
    TrackedVehicle,
}

/// Allocation and fixed-step configuration. Equal deterministic flags/config are a peer contract.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsConfig {
    /// Fixed interval in seconds; immutable for the lifetime of the world.
    pub fixed_dt: f32,
    /// Maximum simultaneous rigid bodies.
    pub max_bodies: u32,
    /// Maximum broadphase pairs and contact constraints.
    pub max_pairs: u32,
    /// Maximum simultaneous contact constraints.
    pub max_contacts: u32,
    /// Threads including the calling thread. One avoids a worker pool.
    pub threads: u32,
    /// Initial scratch storage in MiB, with allocator fallback.
    pub scratch_mb: u32,
    /// Velocity solver iterations; S6 requires fifteen to hold a 20-box tower.
    pub velocity_steps: u32,
    /// Position solver iterations.
    pub position_steps: u32,
    /// Per-tick contact capacity; zero disables contacts for throughput-only workloads.
    pub event_capacity: u32,
    /// Symmetric inclusion masks for sixteen layers; static layer zero does not self-collide.
    pub layer_masks: [u32; 16],
    /// Gravity vector in meters per second squared.
    pub gravity: [f32; 3],
}
impl Default for PhysicsConfig {
    fn default() -> Self {
        let mut layer_masks = [0xffff; 16];
        layer_masks[0] = 0xfffe;
        Self {
            fixed_dt: 1.0 / 60.0,
            max_bodies: 32768,
            max_pairs: 262144,
            max_contacts: 262144,
            threads: 1,
            scratch_mb: 64,
            velocity_steps: 15,
            position_steps: 2,
            event_capacity: 65536,
            layer_masks,
            gravity: [0.0, -9.81, 0.0],
        }
    }
}

/// One backend implementation. Native caches are owned per world; no process gameplay globals.
pub trait PhysicsBackend: Send + Sync + Sized + 'static {
    /// Opaque, trusted native state, constructible only by `save`.
    type Snapshot: Clone + Send + Sync;
    /// Creates an isolated world.
    fn new(config: &PhysicsConfig) -> Result<Self>;
    /// Reports authoring/runtime capability before loading a feature.
    fn supports(&self, feature: Feature) -> bool;
    /// Whether the native build uses cross-platform deterministic floating-point rules.
    fn deterministic(&self) -> bool;
    /// Cooks immutable collision geometry from raw asset data.
    fn create_shape(&mut self, shape: Shape<'_>) -> Result<ShapeHandle>;
    /// Atomically creates bodies in slice order; appends handles in the same order.
    fn create_bodies(&mut self, bodies: &[BodyDesc], out: &mut Vec<BodyHandle>) -> Result<()>;
    /// Removes a body, rejecting attached joints/vehicle ownership.
    fn destroy_body(&mut self, body: BodyHandle) -> Result<()>;
    /// Applies runtime commands in their defined slice order.
    fn apply(&mut self, commands: &[BodyCommand], dt: f32) -> Result<()>;
    /// Reads exactly the requested bodies, in order, replacing `out`.
    fn read_bodies(&mut self, bodies: &[BodyHandle], out: &mut Vec<BodyState>) -> Result<()>;
    /// Reads one body's authoritative state.
    fn body_state(&self, body: BodyHandle) -> Result<BodyState>;
    /// Number of currently active bodies (order is never used in simulation).
    fn active_count(&self) -> u32;
    /// Builds the broadphase once after a large scene load, outside the frame loop.
    fn optimize(&mut self) -> Result<()>;
    /// Creates a capsule character.
    fn create_character(&mut self, desc: CharacterDesc) -> Result<CharacterHandle>;
    /// Removes a character.
    fn destroy_character(&mut self, character: CharacterHandle) -> Result<()>;
    /// Runs one replayable movement step, including floor/stair/clearance policies.
    fn step_character(
        &mut self,
        character: CharacterHandle,
        input: CharacterInput,
        dt: f32,
    ) -> Result<()>;
    /// Extracts character state into ECS.
    fn character_state(&self, character: CharacterHandle) -> Result<CharacterState>;
    /// Creates a four-wheel vehicle with engine/transmission defaults.
    fn create_vehicle(&mut self, desc: VehicleDesc) -> Result<VehicleHandle>;
    /// Removes a vehicle and its chassis.
    fn destroy_vehicle(&mut self, vehicle: VehicleHandle) -> Result<()>;
    /// Applies driver intent for the upcoming fixed step.
    fn drive(&mut self, vehicle: VehicleHandle, input: VehicleInput) -> Result<()>;
    /// Extracts drivetrain and wheel state.
    fn vehicle_state(&self, vehicle: VehicleHandle) -> Result<VehicleState>;
    /// Creates one of the six joint kinds.
    fn create_joint(&mut self, desc: JointDesc) -> Result<JointHandle>;
    /// Removes a joint.
    fn destroy_joint(&mut self, joint: JointHandle) -> Result<()>;
    /// Advances rigid bodies and vehicles; character inputs must have run first.
    fn step(&mut self, dt: f32, tick: u64, events: &mut PhysicsEvents) -> Result<()>;
    /// Executes precise, sorted scene queries. No simulation broadphase-only queries.
    fn queries(&self, queries: &[Query], out: &mut QueryResults) -> Result<()>;
    /// Saves all state needed to repeat a future tick, including active contact metadata.
    fn save(&mut self) -> Result<Self::Snapshot>;
    /// Restores trusted state; topology/build/config mismatches are rejected before mutation.
    fn restore(&mut self, snapshot: &Self::Snapshot) -> Result<()>;
    /// Emits a typed renderer stream; geometry is obtained with `debug_geometry`.
    fn debug_draw(&self, out: &mut Vec<DebugPrimitive>) -> Result<()>;
    /// Immutable local-space mesh/primitive data for a shape handle.
    fn debug_geometry(&self, shape: ShapeHandle) -> Option<&DebugGeometry>;
}
