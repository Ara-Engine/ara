mod support;
use ara_physics::*;
use support::*;

#[test]
fn s6_tall_stack_and_pyramid_hold_with_production_defaults() {
    let mut w = world(1, 1024);
    ground(&mut w, 60.0);
    let pyramid = pyramid(&mut w);
    let tower: Vec<_> = (0..20)
        .map(|i| {
            add_box(
                &mut w,
                Vec3::new(15.0, 0.5 + i as f32, 0.0),
                Quat::IDENTITY,
                Vec3::splat(0.5),
                MotionType::Dynamic,
            )
        })
        .collect();
    let initial: Vec<_> = pyramid.iter().map(|id| position(&w, *id)).collect();
    w.backend_mut().optimize().expect("broadphase");
    for _ in 0..600 {
        w.step().expect("stacking tick");
    }
    let lean = pyramid
        .iter()
        .zip(initial)
        .map(|(id, start)| {
            let d = position(&w, *id) - start;
            Vec3::new(d.x, 0.0, d.z).length()
        })
        .fold(0.0, f32::max);
    let tower_lean = tower
        .iter()
        .map(|id| {
            let p = position(&w, *id);
            Vec3::new(p.x - 15.0, 0.0, p.z).length()
        })
        .fold(0.0, f32::max);
    let top = position(&w, tower[19]).y;
    println!(
        "stack: pyramid lean={lean:.6}m, tower lean={tower_lean:.6}m, top={top:.5}m, awake={}",
        w.backend().active_count()
    );
    assert!(lean < 0.1 && tower_lean < 0.1 && top > 19.0);
    assert_eq!(w.backend().active_count(), 0);
}

#[test]
fn s6_vehicle_drives_turns_and_brakes_on_defaults() {
    let mut w = world(1, 64);
    ground(&mut w, 400.0);
    let car = car(&mut w, Vec3::Y);
    for _ in 0..60 {
        w.backend_mut()
            .drive(car, VehicleInput::default())
            .expect("idle");
        w.step().expect("settle");
    }
    let chassis = w.backend().vehicle_state(car).expect("car").chassis;
    let start = position(&w, chassis);
    let mut min_up = 1.0f32;
    let mut contacts = 0;
    let mut turn = 0.0f32;
    let mut last_heading = 0.0f32;
    let mut speed = 0.0f32;
    for tick in 0..600 {
        let input = if tick < 300 {
            VehicleInput {
                throttle: 1.0,
                ..Default::default()
            }
        } else {
            VehicleInput {
                throttle: 0.5,
                steer: 1.0,
                ..Default::default()
            }
        };
        w.backend_mut().drive(car, input).expect("drive");
        w.step().expect("vehicle tick");
        let v = w.backend().vehicle_state(car).expect("telemetry");
        let b = w.backend().body_state(chassis).expect("chassis");
        let q = Quat::from_array(b.pose.rotation.0);
        min_up = min_up.min((q * Vec3::Y).y);
        contacts += v.wheels_in_contact;
        let forward = q * Vec3::Z;
        let heading = forward.x.atan2(forward.z);
        let dh = (heading - last_heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        last_heading = heading;
        if tick >= 300 {
            turn += dh;
        }
        if tick == 299 {
            speed = Vec3::from_array(b.velocity.linear).length();
            assert!((position(&w, chassis).x - start.x).abs() < 0.5);
        }
    }
    println!(
        "car: 5s speed={speed:.3}m/s, turn={}deg, min up={min_up}, mean grounded={}",
        turn.to_degrees(),
        contacts as f32 / 600.0
    );
    assert!(speed > 5.0 && turn < -0.1 && min_up > 0.9 && contacts as f32 / 600.0 > 3.5);
    for _ in 0..240 {
        w.backend_mut()
            .drive(
                car,
                VehicleInput {
                    brake: 1.0,
                    ..Default::default()
                },
            )
            .expect("brake");
        w.step().expect("braking tick");
    }
    assert!(
        Vec3::from_array(
            w.backend()
                .body_state(chassis)
                .expect("stopped car")
                .velocity
                .linear
        )
        .length()
            < 0.5
    );
}

#[test]
fn cooks_all_geometry_and_rejects_invalid_meshes_without_panicking() {
    let mut w = world(1, 64);
    let vertices = [
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [0.0, 2.0, 0.0],
    ];
    let box_shape = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [0.5; 3],
        })
        .expect("box");
    let sphere = w
        .backend_mut()
        .create_shape(Shape::Sphere { radius: 0.5 })
        .expect("sphere");
    let capsule = w
        .backend_mut()
        .create_shape(Shape::Capsule {
            radius: 0.3,
            half_height: 0.6,
        })
        .expect("capsule");
    let cylinder = w
        .backend_mut()
        .create_shape(Shape::Cylinder {
            radius: 0.5,
            half_height: 0.5,
        })
        .expect("cylinder");
    let hull = w
        .backend_mut()
        .create_shape(Shape::ConvexHull {
            vertices: &vertices,
        })
        .expect("hull");
    let mesh = w
        .backend_mut()
        .create_shape(Shape::Mesh {
            vertices: &vertices,
            indices: &[0, 2, 1],
        })
        .expect("mesh");
    let terrain = w
        .backend_mut()
        .create_shape(Shape::HeightField {
            side: 4,
            heights: &[0.0; 16],
            scale: [1.0; 3],
            offset: [0.0; 3],
        })
        .expect("heightfield");
    let parts = [
        ShapePart {
            shape: box_shape,
            pose: PhysicsPose::default(),
        },
        ShapePart {
            shape: sphere,
            pose: pose(Vec3::Y, Quat::IDENTITY),
        },
    ];
    let compound = w
        .backend_mut()
        .create_shape(Shape::Compound { parts: &parts })
        .expect("compound");
    for (i, shape) in [
        box_shape, sphere, capsule, cylinder, hull, mesh, terrain, compound,
    ]
    .into_iter()
    .enumerate()
    {
        add_body(
            &mut w,
            shape,
            Vec3::new(i as f32 * 4.0, 1.0, 0.0),
            Quat::IDENTITY,
            if i == 5 || i == 6 {
                MotionType::Static
            } else {
                MotionType::Dynamic
            },
        );
    }
    assert!(
        w.backend_mut()
            .create_shape(Shape::Mesh {
                vertices: &vertices,
                indices: &[0, 1, 99]
            })
            .is_err()
    );
    assert!(
        w.backend_mut()
            .create_shape(Shape::ConvexHull {
                vertices: &[[0.0; 3]; 4]
            })
            .is_err()
    );
    assert!(
        w.backend_mut()
            .create_shape(Shape::HeightField {
                side: 4,
                heights: &[0.0; 15],
                scale: [1.0; 3],
                offset: [0.0; 3]
            })
            .is_err()
    );
    assert!(
        w.backend_mut()
            .create_shape(Shape::Box {
                half_extents: [f32::NAN; 3]
            })
            .is_err()
    );
    for _ in 0..10 {
        w.step().expect("geometry tick");
    }
    let mut draw = Vec::new();
    w.backend().debug_draw(&mut draw).expect("debug stream");
    assert_eq!(
        draw.iter()
            .filter(|d| matches!(d, DebugPrimitive::Shape { .. }))
            .count(),
        8
    );
    assert!(w.backend().debug_geometry(compound).is_some());
}

#[test]
fn six_joint_kinds_constrain_and_snapshot_restores_them() {
    for kind in [
        JointKind::Fixed,
        JointKind::Hinge,
        JointKind::Slider,
        JointKind::Distance,
        JointKind::Point,
        JointKind::SixDof,
    ] {
        let mut w = world(1, 16);
        let body = add_box(
            &mut w,
            Vec3::Y * 2.0,
            Quat::IDENTITY,
            Vec3::splat(0.25),
            MotionType::Dynamic,
        );
        let settings = Joint {
            kind,
            anchor_a: [0.0, 2.0, 0.0],
            anchor_b: [0.0, 2.0, 0.0],
            rotation_min: [-0.5; 3],
            rotation_max: [0.5; 3],
            translation_max: if kind == JointKind::Distance {
                [0.5, 0.0, 0.0]
            } else {
                [0.0; 3]
            },
            ..Default::default()
        };
        let joint = w
            .backend_mut()
            .create_joint(JointDesc {
                body_a: None,
                body_b: body,
                joint: settings,
            })
            .expect("joint");
        let snap = w.snapshot().expect("joint snapshot");
        for _ in 0..120 {
            w.step().expect("joint tick");
        }
        let first = hash(&w, &[body], &[], &[]);
        assert!(
            position(&w, body).y > 1.4,
            "{kind:?}: {:?}",
            position(&w, body)
        );
        w.restore(&snap).expect("restore constraint warm start");
        for _ in 0..120 {
            w.step().expect("replay");
        }
        assert_eq!(hash(&w, &[body], &[], &[]), first, "{kind:?}");
        assert!(w.backend_mut().destroy_body(body).is_err());
        w.backend_mut().destroy_joint(joint).expect("detach");
        w.backend_mut().destroy_body(body).expect("remove body");
        assert!(w.backend().body_state(body).is_err());
    }
}
