//! Raw, world-owned C ABI. Prefer `ara-physics-jolt` for checked, safe access.
//!
//! No callback enters Rust. All calls are synchronous; a world must not be mutated
//! concurrently. Pointer/length pairs must be valid for the complete call. Native
//! exceptions are contained and reported through sentinel returns and `ara_last_error`.

use core::ffi::c_char;

#[cfg(test)]
mod layout_test;

/// Invalid native handle (valid handles may include zero).
pub const INVALID: u32 = u32::MAX;

/// Opaque owner of one isolated native physics world.
#[repr(C)]
pub struct AraWorld {
    _private: [u8; 0],
}

mod types;
pub use types::*;

unsafe extern "C" {
    /// Copies the thread-local error; returns required bytes including NUL.
    /// # Safety
    /// `out` must have `capacity` writable bytes, or be null when capacity is zero.
    pub fn ara_last_error(out: *mut c_char, capacity: usize) -> usize;
    /// Writes size, alignment and field offsets for all ABI records.
    /// # Safety
    /// `out` must contain `capacity` writable u32 elements.
    pub fn ara_abi_layout(out: *mut u32, capacity: u32) -> u32;
    /// Returns whether cross-platform deterministic floating-point rules are enabled.
    pub fn ara_deterministic() -> u32;
    /// Creates an isolated owner; null indicates a contained native failure.
    /// # Safety
    /// `desc` must reference an initialized, valid configuration.
    pub fn ara_world_create(desc: *const AraWorldDesc) -> *mut AraWorld;
    /// Destroys a world exactly once after all outstanding operations finish.
    /// # Safety
    /// `world` must be null or an exclusively owned world returned by create.
    pub fn ara_world_destroy(world: *mut AraWorld);
    /// Runs a fixed tick; zero succeeds, high bit means native failure/event overflow.
    /// # Safety
    /// `world` must be live and exclusively borrowed; dt positive and finite.
    pub fn ara_world_step(world: *mut AraWorld, dt: f32) -> u32;
    /// Optimizes broadphase after scene construction.
    /// # Safety
    /// `world` must be live and exclusively borrowed.
    pub fn ara_world_optimize(world: *mut AraWorld) -> u32;
    /// Reads simultaneous awake body count.
    /// # Safety
    /// `world` must be live with no concurrent mutation.
    pub fn ara_world_active(world: *const AraWorld) -> u32;
    /// Creates an immutable shape. Returns INVALID on failure.
    /// # Safety
    /// World/descriptor and pointer/count pairs must be valid; indices and shape references checked.
    pub fn ara_shape_create(
        world: *mut AraWorld,
        desc: *const AraShapeDesc,
        vertices: *const AraVec3,
        vertex_count: u32,
        indices: *const u32,
        index_count: u32,
        samples: *const f32,
        sample_len: u32,
        parts: *const AraShapePart,
        part_count: u32,
    ) -> u32;
    /// Creates an all-or-none deterministic body batch; returns 1 on success.
    /// # Safety
    /// `descs` and `out` must hold count elements; world exclusively borrowed.
    pub fn ara_bodies_create(
        world: *mut AraWorld,
        descs: *const AraBodyDesc,
        count: u32,
        out: *mut u32,
    ) -> u32;
    /// Removes and destroys a body, refusing live vehicle/joint dependencies.
    /// # Safety
    /// World must be live and exclusively borrowed.
    pub fn ara_body_destroy(world: *mut AraWorld, body: u32) -> u32;
    /// Extracts body states in input order; rejects stale handles.
    /// # Safety
    /// Pointer/count pairs must be valid and world cannot be mutated concurrently.
    pub fn ara_bodies_read(
        world: *const AraWorld,
        bodies: *const u32,
        count: u32,
        out: *mut AraBodyState,
    ) -> u32;
    /// Applies a validated, ordered command batch.
    /// # Safety
    /// World exclusively borrowed and commands references count initialized records.
    pub fn ara_bodies_apply(
        world: *mut AraWorld,
        commands: *const AraBodyCommand,
        count: u32,
        dt: f32,
    ) -> u32;
    /// Runs narrowphase queries and writes at most capacity hits, with per-query offsets.
    /// # Safety
    /// Queries has count records, offsets has count+1 elements; hits has capacity records.
    pub fn ara_queries(
        world: *const AraWorld,
        queries: *const AraQuery,
        count: u32,
        hits: *mut AraHit,
        capacity: u32,
        offsets: *mut u32,
    ) -> u32;
    /// Drains contact notifications between ticks; returns count, or INVALID on overflow.
    /// # Safety
    /// World exclusively borrowed; out holds capacity records.
    pub fn ara_contacts(world: *mut AraWorld, out: *mut AraContact, capacity: u32) -> u32;
    /// Creates a capsule character. Returns INVALID on failure.
    /// # Safety
    /// World exclusively borrowed and descriptor references valid settings.
    pub fn ara_character_create(world: *mut AraWorld, desc: *const AraCharacterDesc) -> u32;
    /// Removes a character without reusing its handle.
    /// # Safety
    /// World must be live and exclusively borrowed.
    pub fn ara_character_destroy(world: *mut AraWorld, character: u32) -> u32;
    /// Runs the complete deterministic movement feature once before the rigid-body step.
    /// # Safety
    /// World exclusively borrowed; input valid and dt positive and finite.
    pub fn ara_character_step(
        world: *mut AraWorld,
        input: *const AraCharacterInput,
        dt: f32,
    ) -> u32;
    /// Reads character state.
    /// # Safety
    /// Live world, valid output record, no concurrent mutation.
    pub fn ara_character_read(
        world: *const AraWorld,
        character: u32,
        out: *mut AraCharacterState,
    ) -> u32;
    /// Creates a wheeled engine/gearbox vehicle. Returns INVALID on failure.
    /// # Safety
    /// World exclusively borrowed and descriptor valid.
    pub fn ara_vehicle_create(world: *mut AraWorld, desc: *const AraVehicleDesc) -> u32;
    /// Removes a vehicle and its chassis, refusing live joint dependencies.
    /// # Safety
    /// World must be live and exclusively borrowed.
    pub fn ara_vehicle_destroy(world: *mut AraWorld, vehicle: u32) -> u32;
    /// Sets driver intent for the upcoming step.
    /// # Safety
    /// World exclusively borrowed and input valid.
    pub fn ara_vehicle_input(world: *mut AraWorld, input: *const AraVehicleInput) -> u32;
    /// Reads drivetrain and wheel state.
    /// # Safety
    /// Live world, valid output record, no concurrent mutation.
    pub fn ara_vehicle_read(world: *const AraWorld, vehicle: u32, out: *mut AraVehicleState)
    -> u32;
    /// Creates a two-body joint. Returns INVALID on failure.
    /// # Safety
    /// World exclusively borrowed and descriptor valid.
    pub fn ara_joint_create(world: *mut AraWorld, desc: *const AraJointDesc) -> u32;
    /// Removes a joint without reusing its handle.
    /// # Safety
    /// World must be live and exclusively borrowed.
    pub fn ara_joint_destroy(world: *mut AraWorld, joint: u32) -> u32;
    /// Saves all native state plus character shape/input policy. Bytes live until next save.
    /// # Safety
    /// World exclusively borrowed and len writable; copy result before another native call.
    pub fn ara_world_save(world: *mut AraWorld, len: *mut usize) -> *const u8;
    /// Restores trusted bytes from save with matching topology; rejects malformed input.
    /// # Safety
    /// World exclusively borrowed; data has len bytes produced by save, not arbitrary network data.
    pub fn ara_world_restore(world: *mut AraWorld, data: *const u8, len: usize) -> u32;
}
