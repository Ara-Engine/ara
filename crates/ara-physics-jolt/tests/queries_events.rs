mod support;
use ara_physics::*;
use ara_physics_jolt::JoltWorld;
use support::*;

#[test]
fn filtered_single_and_batched_ray_shape_cast_and_overlap() {
    let mut w = world(1, 32);
    let floor = ground(&mut w, 20.0);
    let upper = add_box(
        &mut w,
        Vec3::new(0.0, 3.0, 0.0),
        Quat::IDENTITY,
        Vec3::splat(0.5),
        MotionType::Static,
    );
    let sphere = w
        .backend_mut()
        .create_shape(Shape::Sphere { radius: 0.25 })
        .expect("query sphere");
    let filter = QueryFilter::default();
    let queries = [
        Query::Ray {
            origin: [0.0, 10.0, 0.0],
            displacement: [0.0, -20.0, 0.0],
            filter,
        },
        Query::Cast {
            shape: sphere,
            pose: pose(Vec3::Y * 10.0, Quat::IDENTITY),
            displacement: [0.0, -20.0, 0.0],
            filter,
        },
        Query::Overlap {
            shape: sphere,
            pose: pose(Vec3::Y * 3.0, Quat::IDENTITY),
            filter,
        },
    ];
    let mut batch = QueryResults::default();
    w.backend().queries(&queries, &mut batch).expect("batch");
    assert_eq!(batch.get(0).first().map(|h| h.body), Some(upper));
    assert_eq!(batch.get(0).last().map(|h| h.body), Some(floor));
    assert_eq!(batch.get(1).first().map(|h| h.body), Some(upper));
    assert!(batch.get(1)[0].fraction < batch.get(0)[0].fraction);
    assert_eq!(batch.get(2).first().map(|h| h.body), Some(upper));
    assert_eq!(
        w.query_first(queries[0]).expect("single"),
        batch.get(0).first().copied()
    );
    let ignored = Query::Ray {
        origin: [0.0, 10.0, 0.0],
        displacement: [0.0, -20.0, 0.0],
        filter: QueryFilter {
            ignore: Some(upper),
            ..filter
        },
    };
    assert_eq!(
        w.query_first(ignored).expect("ignore").map(|h| h.body),
        Some(floor)
    );
    assert!(
        w.query_first(Query::Ray {
            origin: [0.0, 10.0, 0.0],
            displacement: [0.0, -20.0, 0.0],
            filter: QueryFilter {
                layer_mask: 2,
                ..filter
            }
        })
        .expect("layer exclusion")
        .is_none()
    );
}

#[test]
fn contacts_triggers_filtering_and_effects_replay() {
    let mut config = PhysicsConfig {
        max_bodies: 32,
        event_capacity: 4096,
        ..Default::default()
    };
    // Layer 2 never collides with layer 1; the matrix must remain symmetric.
    config.layer_masks[1] &= !(1 << 2);
    config.layer_masks[2] &= !(1 << 1);
    let mut w = JoltWorld::new(config).expect("event world");
    ground(&mut w, 20.0);
    let shape = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [1.0; 3],
        })
        .expect("trigger box");
    let mut ids = Vec::new();
    w.backend_mut()
        .create_bodies(
            &[BodyDesc {
                shape,
                user_data: 10,
                pose: pose(Vec3::Y * 3.0, Quat::IDENTITY),
                body: RigidBody::default(),
                collider: Collider {
                    sensor: true,
                    ..Default::default()
                },
                velocity: Velocity::default(),
            }],
            &mut ids,
        )
        .expect("trigger");
    let mut ignored = Vec::new();
    w.backend_mut()
        .create_bodies(
            &[BodyDesc {
                shape,
                user_data: 11,
                pose: pose(Vec3::Y * 4.5, Quat::IDENTITY),
                body: RigidBody::default(),
                collider: Collider {
                    layer: 2,
                    ..Default::default()
                },
                velocity: Velocity::default(),
            }],
            &mut ignored,
        )
        .expect("excluded layer platform");
    let falling = add_box(
        &mut w,
        Vec3::Y * 6.0,
        Quat::IDENTITY,
        Vec3::splat(0.25),
        MotionType::Dynamic,
    );
    let mut started = false;
    let mut ended = false;
    let mut contacted = false;
    for _ in 0..180 {
        w.step().expect("contact tick");
        assert!(
            !w.events()
                .contacts
                .iter()
                .any(|e| e.body_a == ignored[0] || e.body_b == ignored[0])
        );
        started |= w
            .events()
            .triggers
            .iter()
            .any(|e| e.contact.phase == ContactPhase::Started);
        ended |= w
            .events()
            .triggers
            .iter()
            .any(|e| e.contact.phase == ContactPhase::Stopped);
        contacted |= w
            .events()
            .contacts
            .iter()
            .any(|e| e.phase == ContactPhase::Started);
    }
    assert!(started && ended && contacted);
    assert!(position(&w, falling).y < 0.3);
    let query = Query::Overlap {
        shape,
        pose: pose(Vec3::Y * 3.0, Quat::IDENTITY),
        filter: QueryFilter {
            sensors: false,
            ..Default::default()
        },
    };
    let mut results = QueryResults::default();
    w.backend()
        .queries(&[query], &mut results)
        .expect("omit sensor");
    assert!(!results.hits.iter().any(|h| h.body == ids[0]));
    w.apply(&[BodyCommand::Teleport {
        body: falling,
        pose: pose(Vec3::Y * 3.0, Quat::IDENTITY),
    }])
    .expect("enter trigger");
    w.step().expect("contact tick");
    let snap = w.snapshot().expect("active trigger snapshot");
    let mut first = Vec::new();
    for _ in 0..90 {
        w.step().expect("first path");
        first.push(w.events().clone());
    }
    w.restore(&snap).expect("restore contact metadata");
    let mut second = Vec::new();
    for _ in 0..90 {
        w.step().expect("replay");
        second.push(w.events().clone());
    }
    assert_eq!(first, second);
}

#[test]
fn virtual_character_trigger_events_restore_without_callback_history() {
    let mut w = JoltWorld::new(PhysicsConfig {
        max_bodies: 32,
        event_capacity: 4096,
        ..Default::default()
    })
    .expect("event world");
    ground(&mut w, 20.0);
    let shape = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [1.0; 3],
        })
        .expect("trigger shape");
    let mut ids = Vec::new();
    w.backend_mut()
        .create_bodies(
            &[BodyDesc {
                shape,
                user_data: 20,
                pose: pose(Vec3::new(0.0, 1.0, 2.0), Quat::IDENTITY),
                body: RigidBody::default(),
                collider: Collider {
                    sensor: true,
                    ..Default::default()
                },
                velocity: Velocity::default(),
            }],
            &mut ids,
        )
        .expect("trigger body");
    let c = character(&mut w, Vec3::ZERO);
    let mut entered = false;
    for _ in 0..60 {
        w.step_character(c, input(Vec3::Z * 2.0)).expect("walk");
        w.step().expect("tick");
        entered |= w.events().character_contacts.iter().any(|e| {
            e.sensor && e.phase == ContactPhase::Started && e.other == PhysicsActor::Body(ids[0])
        });
    }
    assert!(entered, "virtual characters must report trigger entry");
    let snap = w.snapshot().expect("character/trigger checkpoint");
    let mut first = Vec::new();
    for _ in 0..90 {
        w.step_character(c, input(Vec3::Z * 2.0)).expect("walk out");
        w.step().expect("tick");
        first.push(w.events().clone());
    }
    assert!(
        first
            .iter()
            .flat_map(|e| &e.character_contacts)
            .any(|e| e.sensor && e.phase == ContactPhase::Stopped)
    );
    w.restore(&snap).expect("restore active trigger");
    let mut second = Vec::new();
    for _ in 0..90 {
        w.step_character(c, input(Vec3::Z * 2.0))
            .expect("replayed movement");
        w.step().expect("tick");
        second.push(w.events().clone());
    }
    assert_eq!(first, second);
}

#[test]
fn static_query_geometry_returns_a_diagnostic() {
    let mut w = world(1, 8);
    let shape = w
        .backend_mut()
        .create_shape(Shape::Mesh {
            vertices: &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            indices: &[0, 2, 1],
        })
        .expect("mesh");
    assert!(
        w.query_first(Query::Overlap {
            shape,
            pose: PhysicsPose::default(),
            filter: QueryFilter::default()
        })
        .is_err()
    );
}
