//! Minimal Jolt user for binary size / clean build time: a box falls onto the ground.
use ara_jolt_sys::*;

fn main() {
    let desc = AraWorldDesc {
        max_bodies: 16,
        max_body_pairs: 64,
        max_contact_constraints: 64,
        threads: 0,
        temp_allocator_mb: 4,
        gravity: AraVec3 {
            x: 0.0,
            y: -9.81,
            z: 0.0,
        },
    };
    // SAFETY: single-threaded use of one world, destroyed at the end.
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
        let cube = ara_shape_box(
            w,
            AraVec3 {
                x: 0.5,
                y: 0.5,
                z: 0.5,
            },
        );
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
            body(cube, 5.0, ARA_MOTION_DYNAMIC, ARA_LAYER_MOVING),
        ];
        let mut ids = [0u32; 2];
        ara_bodies_add(w, descs.as_ptr(), 2, ids.as_mut_ptr());
        for _ in 0..60 {
            ara_world_step(w, 1.0 / 60.0, 1);
        }
        let mut p = AraVec3::default();
        ara_body_position(w, ids[1], &mut p);
        println!("{}", p.y);
        ara_world_destroy(w);
    }
}
