mod support;
use ara_physics::*;
use ara_physics_jolt::JoltWorld;
use support::*;

#[test]
fn validates_world_settings_and_rejects_unsupported_dimensions() {
    assert!(
        JoltWorld::new(PhysicsConfig {
            fixed_dt: f32::NAN,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        JoltWorld::new(PhysicsConfig {
            threads: 65,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        JoltWorld::new(PhysicsConfig {
            max_bodies: 0,
            ..Default::default()
        })
        .is_err()
    );
    let mut config = PhysicsConfig::default();
    config.layer_masks[2] &= !(1 << 1);
    assert!(JoltWorld::new(config).is_err());
    let w = world(1, 4);
    assert!(w.backend().supports(Feature::Character));
    assert!(!w.backend().supports(Feature::Physics2d));
    assert_eq!(
        w.backend().deterministic(),
        cfg!(feature = "cross-platform-deterministic")
    );
}

#[test]
fn body_batches_and_runtime_intents_validate_before_mutation() {
    let mut w = world(1, 2);
    let shape = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [0.5; 3],
        })
        .expect("shape");
    let desc = BodyDesc {
        shape,
        user_data: 1,
        pose: PhysicsPose::default(),
        body: RigidBody {
            motion: MotionType::Dynamic,
            ..Default::default()
        },
        collider: Collider {
            layer: 1,
            ..Default::default()
        },
        velocity: Velocity::default(),
    };
    let mut out = Vec::new();
    assert!(
        w.backend_mut()
            .create_bodies(&[desc.clone(), desc.clone(), desc.clone()], &mut out)
            .is_err()
    );
    assert!(out.is_empty());
    w.backend_mut()
        .create_bodies(&[desc], &mut out)
        .expect("failed oversized batch must leave capacity available");
    let body = out[0];
    let initial = w.backend().body_state(body).expect("initial state");
    assert!(
        w.apply(&[
            BodyCommand::Velocity {
                body,
                velocity: Velocity {
                    linear: [3.0, 0.0, 0.0],
                    angular: [0.0; 3]
                }
            },
            BodyCommand::Force {
                body,
                force: [f32::NAN, 0.0, 0.0],
                torque: [0.0; 3]
            }
        ])
        .is_err()
    );
    assert_eq!(
        w.backend().body_state(body).expect("rejected command"),
        initial
    );
    w.backend_mut().destroy_body(body).expect("destroy");
    assert!(w.backend().body_state(body).is_err());
    let mut stale = Vec::new();
    assert!(w.backend_mut().read_bodies(&[body], &mut stale).is_err());
    let replacement = add_box(
        &mut w,
        Vec3::ZERO,
        Quat::IDENTITY,
        Vec3::splat(0.5),
        MotionType::Dynamic,
    );
    assert_ne!(replacement, body);
    assert!(w.apply(&[BodyCommand::Wake(body)]).is_err());
}

#[test]
fn snapshot_topology_depends_on_body_order_not_batch_boundaries() {
    let mut a = world(1, 16);
    let mut b = world(1, 16);
    let shape = a
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [0.5; 3],
        })
        .expect("shape A");
    assert_eq!(
        b.backend_mut()
            .create_shape(Shape::Box {
                half_extents: [0.5; 3]
            })
            .expect("shape B"),
        shape
    );
    let bodies: Vec<_> = (0..3)
        .map(|i| BodyDesc {
            shape,
            user_data: i + 1,
            pose: pose(Vec3::new(i as f32 * 2.0, 5.0, 0.0), Quat::IDENTITY),
            body: RigidBody {
                motion: MotionType::Dynamic,
                ..Default::default()
            },
            collider: Collider {
                layer: 1,
                ..Default::default()
            },
            velocity: Velocity::default(),
        })
        .collect();
    let mut ids = Vec::new();
    a.backend_mut()
        .create_bodies(&bodies, &mut ids)
        .expect("one batch");
    let mut other = Vec::new();
    for body in &bodies {
        b.backend_mut()
            .create_bodies(std::slice::from_ref(body), &mut other)
            .expect("three batches");
    }
    for _ in 0..30 {
        a.step().expect("tick A");
    }
    let checkpoint = a.snapshot().expect("checkpoint");
    b.restore(&checkpoint)
        .expect("same creation order with different grouping is identical topology");
    for _ in 0..30 {
        a.step().expect("future A");
        b.step().expect("future B");
    }
    assert_eq!(hash(&a, &ids, &[], &[]), hash(&b, &other, &[], &[]));
}

#[test]
fn dynamic_component_defaults_choose_the_moving_collision_group() {
    let mut w = world(1, 8);
    ground(&mut w, 10.0);
    let shape = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [0.5; 3],
        })
        .expect("box");
    let mut ids = Vec::new();
    w.backend_mut()
        .create_bodies(
            &[BodyDesc {
                shape,
                user_data: 1,
                pose: pose(Vec3::Y * 3.0, Quat::IDENTITY),
                body: RigidBody {
                    motion: MotionType::Dynamic,
                    ..Default::default()
                },
                collider: Collider::default(),
                velocity: Velocity::default(),
            }],
            &mut ids,
        )
        .expect("default dynamic body");
    for _ in 0..120 {
        w.step().expect("fall and settle");
    }
    assert!((position(&w, ids[0]).y - 0.5).abs() < 0.03);
}
