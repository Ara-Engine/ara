//! A trusted restore preserves the ECS state seen before the next physics step.
use ara_physics::*;
use ara_physics_jolt::JoltBackend;
use bevy_ecs::prelude::{Schedule, World};

#[test]
fn tick_zero_restore_keeps_absent_mirrors_and_exact_authored_poses() {
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register::<JoltBackend>(&mut world, &mut schedule, PhysicsConfig::default()).expect("physics");
    let pose = PhysicsPose {
        position: [12., 0.008, 7.],
        ..Default::default()
    };
    let body = world
        .spawn((
            PhysicsId { value: 1 },
            pose,
            RigidBody::default(),
            Collider {
                center: [0., 0.7, 0.],
                ..Default::default()
            },
        ))
        .id();
    let actor = world
        .spawn((
            PhysicsId { value: 2 },
            PhysicsPose {
                position: [1., 3., 0.],
                ..Default::default()
            },
            CharacterController::default(),
        ))
        .id();
    let snapshot = snapshot_ecs::<JoltBackend>(&mut world).expect("tick-zero snapshot");
    assert!(world.get::<CharacterMotion>(actor).is_none());
    assert!(world.get::<Velocity>(body).is_none());
    schedule.run(&mut world);
    restore_ecs::<JoltBackend>(&mut world, &snapshot).expect("restore");
    assert!(
        world.get::<CharacterMotion>(actor).is_none(),
        "absence at the boundary must survive restore"
    );
    assert!(
        world.get::<Velocity>(body).is_none(),
        "restore must not introduce new mirrored components"
    );
    assert_eq!(
        world.get::<PhysicsPose>(body),
        Some(&pose),
        "authored pose bits must survive native shape-center conversion"
    );
}
