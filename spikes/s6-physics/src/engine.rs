//! The spike's test-harness interface: the primitives the scenarios need, implemented once
//! per engine (`jolt.rs`, `rapier.rs`). Scenario logic is written once against it so both
//! engines see identical scenes, inputs and metrics.
//!
//! This is a harness, not the proposed `ara-physics` API (see RESULT.md for that sketch).

use crate::common::{Quat, Vec3};

/// Index into the backend's own body table (creation order).
pub type BodyId = u32;

/// World sizing and threading.
#[derive(Clone, Copy, Debug)]
pub struct WorldConfig {
    pub max_bodies: u32,
    /// 0/1 = single-threaded; N = N worker threads in total.
    pub threads: usize,
}

#[derive(Clone, Copy, Debug)]
pub enum ShapeDesc {
    Box { half: Vec3 },
    Sphere { radius: f32 },
}

/// A dynamic rigid body to create.
#[derive(Clone, Copy, Debug)]
pub struct DynamicDesc {
    pub shape: ShapeDesc,
    pub position: Vec3,
    pub rotation: Quat,
    pub friction: f32,
}

/// Body state as read back for ECS sync and hashing.
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyState {
    pub position: Vec3,
    pub rotation: Quat,
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
    pub active: bool,
}

/// Kinematic character controller parameters (capsule, feet at `position`).
#[derive(Clone, Copy, Debug)]
pub struct CharacterDesc {
    pub position: Vec3,
    pub radius: f32,
    pub half_height: f32,
    pub max_slope_deg: f32,
    pub step_height: f32,
    pub stick_down: f32,
    pub padding: f32,
}

/// Character state after an update.
#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterState {
    pub position: Vec3,
    pub velocity: Vec3,
    pub grounded: bool,
    pub ground_velocity: Vec3,
}

/// Four-wheel car parameters; forward is +Z, wheels FL, FR, RL, RR.
#[derive(Clone, Copy, Debug)]
pub struct VehicleDesc {
    pub position: Vec3,
    pub half_extents: Vec3,
    pub mass: f32,
    pub wheel_radius: f32,
    pub wheel_width: f32,
    pub wheel_offsets: [Vec3; 4],
    pub suspension_rest: f32,
    pub suspension_travel: f32,
    pub suspension_frequency: f32,
    pub suspension_damping_ratio: f32,
    pub max_steer_deg: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VehicleState {
    pub chassis: BodyState,
    pub wheels_in_contact: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub distance: f32,
    pub normal: Vec3,
}

/// What every backend implements. Methods map 1:1 to native calls where possible.
pub trait Engine {
    const NAME: &'static str;

    fn new(cfg: WorldConfig) -> Self;

    fn add_static_box(&mut self, position: Vec3, rotation: Quat, half: Vec3) -> BodyId;
    fn add_static_mesh(&mut self, vertices: &[Vec3], triangles: &[[u32; 3]]) -> BodyId;
    fn add_kinematic_box(&mut self, position: Vec3, half: Vec3) -> BodyId;
    /// Batch creation; ids follow slice order.
    fn add_dynamic(&mut self, bodies: &[DynamicDesc]) -> Vec<BodyId>;
    /// Called once after scene construction (broadphase build, etc.).
    fn finish_setup(&mut self);
    /// Overrides the solver iteration count (engine-specific meaning; see backends).
    fn set_solver_iterations(&mut self, n: u32);

    /// Sets a kinematic body's target for the next step (velocity derived from DT).
    fn move_kinematic(&mut self, id: BodyId, position: Vec3);
    fn step(&mut self);

    fn read(&self, ids: &[BodyId], out: &mut Vec<BodyState>);
    /// Accumulated step error flags (Jolt: EPhysicsUpdateError bits); 0 when healthy.
    fn step_errors(&self) -> u32 {
        0
    }
    fn active_count(&self) -> usize;

    fn add_character(&mut self, desc: &CharacterDesc) -> u32;
    /// Runs one tick of the engine's idiomatic character update (before `step`).
    fn update_character(&mut self, character: u32, desired_velocity: Vec3) -> CharacterState;
    fn character_state(&self, character: u32) -> CharacterState;

    fn add_vehicle(&mut self, desc: &VehicleDesc) -> u32;
    /// Driver input for the next step. `forward` in [-1, 1]; `right` in [-1, 1], +1 = turn right.
    fn drive(&mut self, vehicle: u32, forward: f32, right: f32);
    fn vehicle_state(&self, vehicle: u32) -> VehicleState;
    fn vehicle_chassis(&self, vehicle: u32) -> BodyId;

    /// Full simulation state as bytes (rollback contract): bodies, contacts, constraints,
    /// characters and vehicles.
    fn save(&mut self) -> Vec<u8>;
    /// Restores bytes from `save` on a world built the same way.
    fn restore(&mut self, bytes: &[u8]);

    fn cast_ray(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Hit>;
    fn cast_sphere(
        &self,
        radius: f32,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<Hit>;

    /// Cheapest single-body read (FFI overhead probe).
    fn body_position(&self, id: BodyId) -> Vec3;
}
