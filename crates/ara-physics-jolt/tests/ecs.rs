use ara_physics::*;
use ara_physics_jolt::{JoltBackend, JoltWorld};
use bevy_ecs::prelude::{Entity, Schedule, World};

fn spawn(world: &mut World, id: u64, position: [f32; 3], motion: MotionType) -> Entity {
    world
        .spawn((
            PhysicsId { value: id },
            PhysicsPose {
                position,
                ..Default::default()
            },
            RigidBody {
                motion,
                ..Default::default()
            },
            Collider {
                layer: if motion == MotionType::Static { 0 } else { 1 },
                ..Default::default()
            },
            Velocity::default(),
        ))
        .id()
}
fn setup(reverse: bool) -> (World, Schedule, Entity, Entity) {
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register::<JoltBackend>(
        &mut world,
        &mut schedule,
        PhysicsConfig {
            max_bodies: 64,
            ..Default::default()
        },
    )
    .expect("register");
    let (floor, body) = if reverse {
        let b = spawn(&mut world, 2, [0.0, 5.0, 0.0], MotionType::Dynamic);
        let f = spawn(&mut world, 1, [0.0, -0.5, 0.0], MotionType::Static);
        (f, b)
    } else {
        let f = spawn(&mut world, 1, [0.0, -0.5, 0.0], MotionType::Static);
        let b = spawn(&mut world, 2, [0.0, 5.0, 0.0], MotionType::Dynamic);
        (f, b)
    };
    world
        .get_mut::<Collider>(floor)
        .expect("floor collider")
        .half_extents = [20.0, 0.5, 20.0];
    (world, schedule, floor, body)
}

#[test]
fn ecs_identity_order_lifecycle_commands_and_snapshot_resimulation() {
    let (mut w, mut schedule, _, body) = setup(false);
    let (mut reverse, mut reverse_schedule, _, other) = setup(true);
    for _ in 0..30 {
        schedule.run(&mut w);
        reverse_schedule.run(&mut reverse);
    }
    assert!(w.resource::<PhysicsStatus>().error.is_none());
    assert_eq!(
        w.get::<PhysicsPose>(body),
        reverse.get::<PhysicsPose>(other)
    );
    assert_eq!(
        body_for_id(&w, 2),
        body_for_id(&reverse, 2),
        "creation follows stable IDs, not Bevy entity order"
    );
    let snap = snapshot_ecs::<JoltBackend>(&mut w).expect("ECS/native checkpoint");
    let command = BodyIntent::Impulse {
        impulse: [0.0, 1000.0, 0.0],
        point: [0.0, 4.0, 0.0],
        angular: [0.0; 3],
    };
    w.resource_mut::<PhysicsCommands>()
        .bodies
        .push((2, command));
    for _ in 0..90 {
        schedule.run(&mut w);
    }
    let expected = *w.get::<PhysicsPose>(body).expect("writeback");
    restore_ecs::<JoltBackend>(&mut w, &snap).expect("restore native and components");
    assert_eq!(w.resource::<JoltWorld>().tick(), 30);
    w.resource_mut::<PhysicsCommands>()
        .bodies
        .push((2, command));
    for _ in 0..90 {
        schedule.run(&mut w);
    }
    assert_eq!(*w.get::<PhysicsPose>(body).expect("replay"), expected);
    let old = body_for_id(&w, 2).expect("old handle");
    w.get_mut::<Collider>(body).expect("collider").radius = 0.8;
    schedule.run(&mut w);
    assert!(w.resource::<PhysicsStatus>().error.is_none());
    let replacement = body_for_id(&w, 2).expect("replacement");
    assert_ne!(old, replacement);
    assert!(w.resource::<JoltWorld>().backend().body_state(old).is_err());
    w.despawn(body);
    schedule.run(&mut w);
    assert!(w.resource::<PhysicsStatus>().error.is_none());
    assert!(body_for_id(&w, 2).is_none());
}

#[test]
fn ecs_character_vehicle_and_joint_are_synchronized() {
    let (mut world, mut schedule, _, body) = setup(false);
    world.entity_mut(body).insert(Joint {
        anchor_a: [0.0, 5.0, 0.0],
        anchor_b: [0.0, 5.0, 0.0],
        ..Default::default()
    });
    let character = world
        .spawn((
            PhysicsId { value: 3 },
            PhysicsPose::default(),
            CharacterController::default(),
            CharacterInput {
                desired_velocity: [0.0, 0.0, 2.0],
                ..Default::default()
            },
        ))
        .id();
    let car = world
        .spawn((
            PhysicsId { value: 4 },
            PhysicsPose {
                position: [10.0, 1.0, 0.0],
                ..Default::default()
            },
            Vehicle::default(),
            VehicleInput {
                throttle: 1.0,
                ..Default::default()
            },
        ))
        .id();
    for _ in 0..120 {
        schedule.run(&mut world);
    }
    assert!(
        world.resource::<PhysicsStatus>().error.is_none(),
        "{:?}",
        world.resource::<PhysicsStatus>().error
    );
    assert!(
        world
            .get::<PhysicsPose>(character)
            .expect("character pose")
            .position[2]
            > 3.0
    );
    assert!(
        world
            .get::<CharacterMotion>(character)
            .expect("support")
            .grounded
    );
    assert!(
        world
            .get::<VehicleTelemetry>(car)
            .expect("car state")
            .engine_rpm
            > 0.0
    );
    assert!((world.get::<PhysicsPose>(body).expect("joint").position[1] - 5.0).abs() < 0.01);
    world.entity_mut(body).remove::<Joint>();
    schedule.run(&mut world);
    assert!(world.resource::<PhysicsStatus>().error.is_none());
    let snap = snapshot_ecs::<JoltBackend>(&mut world).expect("all feature snapshot");
    for _ in 0..60 {
        schedule.run(&mut world);
    }
    let p = *world.get::<PhysicsPose>(character).expect("future pose");
    let v = *world.get::<VehicleTelemetry>(car).expect("future gearbox");
    restore_ecs::<JoltBackend>(&mut world, &snap).expect("all feature restore");
    for _ in 0..60 {
        schedule.run(&mut world);
    }
    assert_eq!(
        *world
            .get::<PhysicsPose>(character)
            .expect("replayed character"),
        p
    );
    assert_eq!(
        *world
            .get::<VehicleTelemetry>(car)
            .expect("replayed vehicle"),
        v
    );
}

#[test]
fn ecs_rejects_duplicate_identity_and_retains_last_good_tick() {
    let (mut world, mut schedule, _, _) = setup(false);
    schedule.run(&mut world);
    let tick = world.resource::<JoltWorld>().tick();
    spawn(&mut world, 2, [2.0, 5.0, 0.0], MotionType::Dynamic);
    schedule.run(&mut world);
    assert!(world.resource::<PhysicsStatus>().error.is_some());
    assert_eq!(world.resource::<JoltWorld>().tick(), tick);
    schedule.run(&mut world);
    assert_eq!(world.resource::<JoltWorld>().tick(), tick);
}

#[test]
fn tick_zero_checkpoint_prepares_structure_without_consuming_intent() {
    let (mut world, mut schedule, _, body) = setup(false);
    world.resource_mut::<PhysicsCommands>().bodies.push((
        2,
        BodyIntent::Velocity(Velocity {
            linear: [1.0, 0.0, 0.0],
            angular: [0.0; 3],
        }),
    ));
    let initial = snapshot_ecs::<JoltBackend>(&mut world).expect("initial checkpoint");
    assert_eq!(initial.tick(), 0);
    assert_eq!(world.resource::<PhysicsCommands>().bodies.len(), 1);
    for _ in 0..30 {
        schedule.run(&mut world);
    }
    let expected = *world.get::<PhysicsPose>(body).expect("first path");
    restore_ecs::<JoltBackend>(&mut world, &initial).expect("restore initial checkpoint");
    for _ in 0..30 {
        schedule.run(&mut world);
    }
    assert_eq!(
        *world.get::<PhysicsPose>(body).expect("initial replay"),
        expected
    );
}
