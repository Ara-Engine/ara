//! `RigidBody.linearDamping` and `angularDamping` reach the native body.
mod support;
use ara_physics::*;
use support::*;

/// A sphere thrown sideways and spinning, with these damping values; returns its horizontal
/// speed and spin after one second of free flight.
fn thrown(linear_damping: f32, angular_damping: f32) -> (f32, f32) {
    let mut w = world(1, 64);
    let shape = w
        .backend_mut()
        .create_shape(Shape::Sphere { radius: 0.5 })
        .expect("valid sphere");
    let mut ids = Vec::new();
    w.backend_mut()
        .create_bodies(
            &[BodyDesc {
                shape,
                user_data: 0,
                pose: pose(Vec3::new(0.0, 100.0, 0.0), Quat::IDENTITY),
                body: RigidBody {
                    motion: MotionType::Dynamic,
                    allow_sleep: false,
                    linear_damping,
                    angular_damping,
                    ..Default::default()
                },
                collider: Collider {
                    layer: 1,
                    ..Default::default()
                },
                velocity: Velocity {
                    linear: [20.0, 0.0, 0.0],
                    angular: [0.0, 10.0, 0.0],
                },
            }],
            &mut ids,
        )
        .expect("valid body");
    for _ in 0..60 {
        w.step().expect("flight tick");
    }
    let state = w.backend().body_state(ids[0]).expect("live body");
    (state.velocity.linear[0], state.velocity.angular[1])
}

#[test]
fn zero_damping_keeps_a_thrown_body_on_its_arc() {
    let (speed, spin) = thrown(0.0, 0.0);
    assert!((speed - 20.0).abs() < 1e-3, "horizontal speed {speed}");
    assert!((spin - 10.0).abs() < 1e-3, "spin {spin}");
}

#[test]
fn the_default_is_the_five_percent_a_second_bodies_always_had() {
    let default = RigidBody::default();
    assert_eq!(
        (default.linear_damping, default.angular_damping),
        (0.05, 0.05)
    );
    let (speed, spin) = thrown(default.linear_damping, default.angular_damping);
    // v' = v (1 - c dt) each tick: about 4.9 percent lost after a second.
    let kept = (1.0f32 - 0.05 * DT).powi(60);
    assert!(
        (speed - 20.0 * kept).abs() < 0.02,
        "horizontal speed {speed}"
    );
    assert!((spin - 10.0 * kept).abs() < 0.02, "spin {spin}");
}

#[test]
fn damping_is_separate_for_motion_and_spin_and_rejects_nonsense() {
    let (speed, spin) = thrown(0.5, 0.0);
    assert!(speed < 13.0 && (spin - 10.0).abs() < 1e-3, "{speed} {spin}");
    let mut w = world(1, 64);
    let shape = w
        .backend_mut()
        .create_shape(Shape::Sphere { radius: 0.5 })
        .expect("valid sphere");
    let mut ids = Vec::new();
    let negative = w.backend_mut().create_bodies(
        &[BodyDesc {
            shape,
            user_data: 0,
            pose: pose(Vec3::ZERO, Quat::IDENTITY),
            body: RigidBody {
                motion: MotionType::Dynamic,
                linear_damping: -1.0,
                ..Default::default()
            },
            collider: Collider {
                layer: 1,
                ..Default::default()
            },
            velocity: Velocity::default(),
        }],
        &mut ids,
    );
    assert!(negative.is_err());
}
