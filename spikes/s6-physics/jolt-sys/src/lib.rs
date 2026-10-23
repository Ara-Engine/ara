//! Raw FFI to Ara's C shim over Jolt Physics (`shim/ara_jolt.h`). Hand-written, no bindgen:
//! the shim is small and ECS-shaped, so the declarations stay readable. The `layout` test
//! asserts every struct size against the C compiler's.
//!
//! All functions are `unsafe`: the caller guarantees the world pointer is live and that
//! handles came from that world.
#![allow(non_camel_case_types)]

/// Opaque Jolt world (PhysicsSystem plus job system, allocators, characters and vehicles).
#[repr(C)]
pub struct AraWorld {
    _private: [u8; 0],
}

/// Plain 3-vector, matches `AraVec3`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AraVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Quaternion (x, y, z, w), matches `AraQuat`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AraQuat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for AraQuat {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }
}

pub const ARA_LAYER_STATIC: u8 = 0;
pub const ARA_LAYER_MOVING: u8 = 1;
pub const ARA_MOTION_STATIC: u8 = 0;
pub const ARA_MOTION_KINEMATIC: u8 = 1;
pub const ARA_MOTION_DYNAMIC: u8 = 2;
pub const ARA_INVALID: u32 = 0xFFFF_FFFF;

/// World creation parameters, matches `AraWorldDesc`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AraWorldDesc {
    pub max_bodies: u32,
    pub max_body_pairs: u32,
    pub max_contact_constraints: u32,
    /// 0/1 = single-threaded job system; N = N-1 workers plus the calling thread.
    pub threads: u32,
    pub temp_allocator_mb: u32,
    pub gravity: AraVec3,
}

/// Body creation parameters, matches `AraBodyDesc`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AraBodyDesc {
    pub shape: u32,
    pub position: AraVec3,
    pub rotation: AraQuat,
    pub motion: u8,
    pub layer: u8,
    pub allow_sleep: u8,
    pub _pad: u8,
    pub friction: f32,
    pub restitution: f32,
    /// <= 0 means "compute from the shape".
    pub mass: f32,
    pub linear_velocity: AraVec3,
}

/// Body state read back in bulk, matches `AraBodyState`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AraBodyState {
    pub position: AraVec3,
    pub rotation: AraQuat,
    pub linear_velocity: AraVec3,
    pub angular_velocity: AraVec3,
    pub active: u32,
}

/// Query hit, matches `AraHit`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AraHit {
    pub body: u32,
    pub fraction: f32,
    pub point: AraVec3,
    pub normal: AraVec3,
}

/// CharacterVirtual parameters, matches `AraCharacterDesc`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AraCharacterDesc {
    pub radius: f32,
    pub half_height: f32,
    pub max_slope_deg: f32,
    pub max_strength: f32,
    pub padding: f32,
    pub position: AraVec3,
}

/// CharacterVirtual state, matches `AraCharacterState`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AraCharacterState {
    pub position: AraVec3,
    pub linear_velocity: AraVec3,
    pub ground_velocity: AraVec3,
    pub ground_normal: AraVec3,
    pub ground_body: u32,
    /// 0 OnGround, 1 OnSteepGround, 2 NotSupported, 3 InAir.
    pub ground_state: u32,
    pub supported: u32,
}

/// Wheeled vehicle parameters, matches `AraVehicleDesc`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AraVehicleDesc {
    pub position: AraVec3,
    pub half_extents: AraVec3,
    pub mass: f32,
    pub wheel_radius: f32,
    pub wheel_width: f32,
    pub wheel_offsets: [AraVec3; 4],
    pub suspension_min: f32,
    pub suspension_max: f32,
    pub suspension_frequency: f32,
    pub suspension_damping: f32,
    pub max_steer_deg: f32,
    pub engine_max_torque: f32,
    pub rear_wheel_drive: u32,
}

/// Vehicle state, matches `AraVehicleState`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AraVehicleState {
    pub chassis: u32,
    pub wheels_in_contact: u32,
    pub engine_rpm: f32,
    pub gear: i32,
}

unsafe extern "C" {
    pub fn ara_world_create(desc: *const AraWorldDesc) -> *mut AraWorld;
    pub fn ara_world_destroy(world: *mut AraWorld);
    pub fn ara_world_step(world: *mut AraWorld, dt: f32, collision_steps: i32) -> u32;
    pub fn ara_world_optimize_broadphase(world: *mut AraWorld);
    pub fn ara_world_set_solver_steps(
        world: *mut AraWorld,
        velocity_steps: u32,
        position_steps: u32,
    );
    pub fn ara_world_active_body_count(world: *const AraWorld) -> u32;
    pub fn ara_world_body_count(world: *const AraWorld) -> u32;
    pub fn ara_cross_platform_deterministic() -> u32;
    pub fn ara_noop(value: u32) -> u32;
    pub fn ara_abi_sizes(out: *mut u32, capacity: u32) -> u32;

    pub fn ara_shape_box(world: *mut AraWorld, half_extents: AraVec3) -> u32;
    pub fn ara_shape_sphere(world: *mut AraWorld, radius: f32) -> u32;
    pub fn ara_shape_mesh(
        world: *mut AraWorld,
        vertices: *const AraVec3,
        vertex_count: u32,
        indices: *const u32,
        triangle_count: u32,
    ) -> u32;

    pub fn ara_bodies_add(
        world: *mut AraWorld,
        descs: *const AraBodyDesc,
        count: u32,
        out_ids: *mut u32,
    ) -> u32;
    pub fn ara_bodies_read(
        world: *const AraWorld,
        ids: *const u32,
        count: u32,
        out: *mut AraBodyState,
    );
    pub fn ara_body_position(world: *const AraWorld, id: u32, out: *mut AraVec3);
    pub fn ara_body_move_kinematic(
        world: *mut AraWorld,
        id: u32,
        position: AraVec3,
        rotation: AraQuat,
        dt: f32,
    );
    pub fn ara_body_set_linear_velocity(world: *mut AraWorld, id: u32, velocity: AraVec3);

    pub fn ara_cast_ray(
        world: *const AraWorld,
        origin: AraVec3,
        direction: AraVec3,
        out: *mut AraHit,
    ) -> u32;
    pub fn ara_cast_sphere(
        world: *const AraWorld,
        radius: f32,
        origin: AraVec3,
        direction: AraVec3,
        out: *mut AraHit,
    ) -> u32;

    pub fn ara_world_save_state(world: *mut AraWorld, out_len: *mut usize) -> *const u8;
    pub fn ara_world_restore_state(world: *mut AraWorld, data: *const u8, len: usize) -> u32;

    pub fn ara_character_create(world: *mut AraWorld, desc: *const AraCharacterDesc) -> u32;
    pub fn ara_character_state(world: *const AraWorld, character: u32, out: *mut AraCharacterState);
    pub fn ara_character_update_ground_velocity(world: *mut AraWorld, character: u32);
    pub fn ara_character_set_linear_velocity(
        world: *mut AraWorld,
        character: u32,
        velocity: AraVec3,
    );
    pub fn ara_character_extended_update(
        world: *mut AraWorld,
        character: u32,
        dt: f32,
        step_up: f32,
        stick_down: f32,
        allow_sliding: u32,
    );

    pub fn ara_vehicle_create(world: *mut AraWorld, desc: *const AraVehicleDesc) -> u32;
    pub fn ara_vehicle_set_input(
        world: *mut AraWorld,
        vehicle: u32,
        forward: f32,
        right: f32,
        brake: f32,
        hand_brake: f32,
    );
    pub fn ara_vehicle_state(world: *const AraWorld, vehicle: u32, out: *mut AraVehicleState);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn layout_matches_c() {
        let mut sizes = [0u32; 16];
        // SAFETY: the buffer holds 16 entries and the capacity says so.
        let n = unsafe { ara_abi_sizes(sizes.as_mut_ptr(), sizes.len() as u32) } as usize;
        let rust = [
            size_of::<AraVec3>(),
            size_of::<AraQuat>(),
            size_of::<AraWorldDesc>(),
            size_of::<AraBodyDesc>(),
            size_of::<AraBodyState>(),
            size_of::<AraHit>(),
            size_of::<AraCharacterDesc>(),
            size_of::<AraCharacterState>(),
            size_of::<AraVehicleDesc>(),
            size_of::<AraVehicleState>(),
        ];
        assert_eq!(n, rust.len());
        for (i, (c, r)) in sizes.iter().zip(rust).enumerate() {
            assert_eq!(*c as usize, r, "struct #{i} differs between C and Rust");
        }
    }

    #[test]
    fn world_steps_and_restores() {
        let desc = AraWorldDesc {
            max_bodies: 64,
            max_body_pairs: 256,
            max_contact_constraints: 256,
            threads: 0,
            temp_allocator_mb: 8,
            gravity: AraVec3 {
                x: 0.0,
                y: -9.81,
                z: 0.0,
            },
        };
        // SAFETY: the world is created, used and destroyed on this thread only.
        unsafe {
            let w = ara_world_create(&desc);
            let ground = ara_shape_box(
                w,
                AraVec3 {
                    x: 10.0,
                    y: 0.5,
                    z: 10.0,
                },
            );
            let ball = ara_shape_sphere(w, 0.5);
            let body = |shape, y, motion, layer| AraBodyDesc {
                shape,
                position: AraVec3 { x: 0.0, y, z: 0.0 },
                rotation: AraQuat::default(),
                motion,
                layer,
                allow_sleep: 1,
                _pad: 0,
                friction: 0.5,
                restitution: 0.0,
                mass: 0.0,
                linear_velocity: AraVec3::default(),
            };
            let descs = [
                body(ground, -0.5, ARA_MOTION_STATIC, ARA_LAYER_STATIC),
                body(ball, 5.0, ARA_MOTION_DYNAMIC, ARA_LAYER_MOVING),
            ];
            let mut ids = [0u32; 2];
            assert_eq!(ara_bodies_add(w, descs.as_ptr(), 2, ids.as_mut_ptr()), 2);
            for _ in 0..30 {
                assert_eq!(ara_world_step(w, 1.0 / 60.0, 1), 0);
            }
            let mut len = 0usize;
            let ptr = ara_world_save_state(w, &mut len);
            let saved = std::slice::from_raw_parts(ptr, len).to_vec();
            let mut at_save = AraVec3::default();
            ara_body_position(w, ids[1], &mut at_save);
            for _ in 0..30 {
                ara_world_step(w, 1.0 / 60.0, 1);
            }
            assert_eq!(ara_world_restore_state(w, saved.as_ptr(), saved.len()), 1);
            let mut restored = AraVec3::default();
            ara_body_position(w, ids[1], &mut restored);
            assert_eq!(at_save, restored);
            assert!(at_save.y < 5.0);
            ara_world_destroy(w);
        }
    }
}
