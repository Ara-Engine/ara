use super::*;

fn check(world: &mut World, schedule: &mut Schedule) {
    let expected = world_matrices(world);
    schedule.run(world);
    for (entity, matrix) in expected {
        let bits = |pose: Pose| {
            (
                pose.translation.to_array().map(f32::to_bits),
                pose.rotation.to_array().map(f32::to_bits),
                pose.scale.to_array().map(f32::to_bits),
            )
        };
        assert_eq!(
            bits(world.get::<WorldPose>(entity).expect("resolved").0),
            bits(Pose::from_matrix(matrix))
        );
    }
}

#[test]
fn cached_poses_follow_reparenting_missing_transforms_and_recycled_entities() {
    let mut world = World::new();
    let leaf = world.spawn(Transform::default()).id();
    let parent = world
        .spawn(Transform {
            position: [2.0, -1.0, 5.0],
            rotation: [15.0, 30.0, 45.0],
            scale: [2.0, 1.0, 0.5],
        })
        .id();
    // The parent was spawned after the child, so iteration is not topological.
    world.entity_mut(leaf).insert(ChildOf(parent));
    let mut schedule = Schedule::default();
    schedule.add_systems((remember_poses, resolve_poses_cached).chain());
    check(&mut world, &mut schedule);
    let before = world.get::<WorldPose>(leaf).expect("pose").0;
    world
        .get_mut::<Transform>(parent)
        .expect("transform")
        .position[0] = 9.0;
    check(&mut world, &mut schedule);
    assert_eq!(world.get::<PreviousPose>(leaf).expect("previous").0, before);

    let gap = world.spawn(ChildOf(parent)).id();
    world.entity_mut(leaf).insert(ChildOf(gap));
    check(&mut world, &mut schedule);
    assert_eq!(
        world.get::<WorldPose>(leaf).expect("pose").0,
        Pose::IDENTITY
    );
    world.entity_mut(gap).insert(Transform::default());
    check(&mut world, &mut schedule);
    world.entity_mut(gap).remove::<Transform>();
    check(&mut world, &mut schedule);
    world.entity_mut(leaf).remove::<ChildOf>();
    world.despawn(parent);
    let replacement = world
        .spawn(Transform {
            position: [100.0, 0.0, 0.0],
            ..Default::default()
        })
        .id();
    world.entity_mut(leaf).insert(ChildOf(replacement));
    check(&mut world, &mut schedule);
    assert_eq!(
        world.get::<WorldPose>(leaf).expect("pose").0.translation.x,
        100.0
    );
    world.entity_mut(leaf).remove::<ChildOf>();
    world
        .get_mut::<Transform>(leaf)
        .expect("transform")
        .position[0] = -0.0;
    check(&mut world, &mut schedule);
}

#[test]
fn pose_resolution_uses_an_iterative_parent_walk() {
    let mut world = World::new();
    let entities: Vec<_> = (0..4096)
        .map(|_| {
            world
                .spawn(Transform {
                    position: [1.0, 0.0, 0.0],
                    ..Default::default()
                })
                .id()
        })
        .collect();
    for pair in entities.windows(2) {
        world.entity_mut(pair[0]).insert(ChildOf(pair[1]));
    }
    let mut schedule = Schedule::default();
    schedule.add_systems(resolve_poses_cached);
    schedule.run(&mut world);
    assert_eq!(
        world
            .get::<WorldPose>(entities[0])
            .expect("pose")
            .0
            .translation
            .x,
        4096.0
    );
    // Detach before drop so Bevy's recursive relationship teardown is not part of this probe.
    for entity in entities {
        world.entity_mut(entity).remove::<ChildOf>();
    }
}
