mod support;
use ara_physics::*;
use ara_physics_jolt::{JoltBackend, JoltWorld};
use support::*;

#[test]
fn column_extraction_matches_aos_and_rejects_stale_generation() {
    let mut w = world(4, 128);
    ground(&mut w, 20.0);
    let ids = grid(&mut w, 64, Vec3::Y * 2.0, 8, 7);
    for _ in 0..60 {
        w.step().expect("tick");
    }
    let mut states = Vec::new();
    let mut columns = BodyColumns::default();
    w.backend_mut().read_bodies(&ids, &mut states).expect("AoS");
    w.backend_mut()
        .read_body_columns(&ids, &mut columns)
        .expect("SoA");
    for (i, state) in states.iter().enumerate() {
        assert_eq!(columns.state(i), Some((state.pose, state.velocity)));
        assert_eq!(columns.active[i] != 0, state.active);
    }
    w.backend_mut().destroy_bodies(&ids).expect("batch removal");
    assert!(
        w.backend_mut()
            .read_body_columns(&ids, &mut columns)
            .is_err()
    );
}

#[test]
fn batch_removal_validates_before_mutating_and_rejects_duplicates() {
    let mut w = world(1, 16);
    let ids = grid(&mut w, 4, Vec3::Y * 2.0, 2, 7);
    assert!(w.backend_mut().destroy_bodies(&[ids[0], ids[0]]).is_err());
    assert!(w.backend().body_state(ids[0]).is_ok());
    assert!(
        w.backend_mut()
            .destroy_bodies(&[ids[0], BodyHandle::from_raw(u32::MAX)])
            .is_err()
    );
    assert!(w.backend().body_state(ids[0]).is_ok());
    w.backend_mut().destroy_bodies(&ids).expect("remove once");
}

#[test]
fn quiet_ecs_writeback_preserves_change_detection_and_recreates_missing_columns() {
    use bevy_ecs::prelude::*;
    let mut w = World::new();
    let mut schedule = Schedule::default();
    register::<JoltBackend>(&mut w, &mut schedule, PhysicsConfig::game_default())
        .expect("register");
    let entity = w
        .spawn((
            PhysicsId { value: 1 },
            PhysicsPose::default(),
            Velocity::default(),
            RigidBody::default(),
            Collider::default(),
        ))
        .id();
    tick_ecs::<JoltBackend>(&mut w).expect("create");
    w.clear_trackers();
    tick_ecs::<JoltBackend>(&mut w).expect("quiet tick");
    let mut changed = w.query_filtered::<Entity, Or<(Changed<PhysicsPose>, Changed<Velocity>)>>();
    assert_eq!(
        changed.iter(&w).count(),
        0,
        "quiet physics must not mark ECS components changed"
    );
    w.entity_mut(entity).remove::<Velocity>();
    tick_ecs::<JoltBackend>(&mut w).expect("reinstall missing column");
    assert_eq!(w.get::<Velocity>(entity), Some(&Velocity::default()));
}

#[test]
fn invalid_tuning_is_rejected_at_the_safe_boundary() {
    for value in [f32::NAN, f32::INFINITY, -0.1] {
        assert!(
            JoltWorld::new(PhysicsConfig {
                speculative_contact_distance: value,
                ..PhysicsConfig::game_default()
            })
            .is_err()
        );
    }
}

#[test]
fn corrected_creation_batch_can_be_retried_without_losing_valid_entities() {
    use bevy_ecs::prelude::*;
    let mut w = World::new();
    let mut schedule = Schedule::default();
    register::<JoltBackend>(
        &mut w,
        &mut schedule,
        PhysicsConfig {
            max_bodies: 1,
            ..PhysicsConfig::game_default()
        },
    )
    .expect("register");
    w.spawn((
        PhysicsId { value: 1 },
        RigidBody::default(),
        Collider::default(),
    ));
    let overflow = w
        .spawn((
            PhysicsId { value: 2 },
            RigidBody::default(),
            Collider::default(),
        ))
        .id();
    assert!(prepare_ecs::<JoltBackend>(&mut w).is_err());
    w.despawn(overflow);
    prepare_ecs::<JoltBackend>(&mut w).expect("retry after capacity correction");
    assert!(body_for_id(&w, 1).is_some());
}

#[cfg(feature = "bench")]
#[test]
fn spatial_character_candidates_match_reference_through_crowds_and_restore() {
    fn scene(tree: bool) -> (JoltWorld, Vec<CharacterHandle>) {
        let mut w = world(4, 64);
        ground(&mut w, 50.0);
        let actors = (0..40)
            .map(|i| {
                character(
                    &mut w,
                    Vec3::new((i % 8) as f32 * 0.9, 0.0, (i / 8) as f32 * 0.9),
                )
            })
            .collect();
        w.backend_mut().benchmark_character_tree(tree);
        (w, actors)
    }
    let (mut spatial, mut actors) = scene(true);
    let (mut reference, mut other) = scene(false);
    for tick in 0..120 {
        for (w, ids) in [(&mut spatial, &actors), (&mut reference, &other)] {
            for (i, &id) in ids.iter().enumerate() {
                let phase = tick as f32 * 0.07 + i as f32;
                w.step_character(
                    id,
                    CharacterInput {
                        desired_velocity: [phase.sin() * 8.0, 0.0, phase.cos() * 8.0],
                        crouch: (30..45).contains(&tick),
                        jump_speed: if tick == 20 { 5.0 } else { 0.0 },
                    },
                )
                .expect("crowd movement");
            }
            w.step().expect("crowd tick");
        }
        for (i, (&a, &b)) in actors.iter().zip(&other).enumerate() {
            assert_eq!(
                spatial.backend().character_state(a).expect("spatial state"),
                reference
                    .backend()
                    .character_state(b)
                    .expect("reference state"),
                "tick {tick}, character {i}"
            );
        }
        assert_eq!(
            hash(&spatial, &[], &actors, &[]),
            hash(&reference, &[], &other, &[]),
            "bit-exact crowd tick {tick}"
        );
        if tick == 60 {
            let snapshot = spatial.snapshot().expect("spatial checkpoint");
            spatial
                .restore(&snapshot)
                .expect("invalidate tree after restore");
            let a = actors.pop().expect("last actor");
            let b = other.pop().expect("last reference actor");
            spatial.backend_mut().destroy_character(a).expect("remove");
            reference
                .backend_mut()
                .destroy_character(b)
                .expect("remove reference");
            actors.push(character(&mut spatial, Vec3::new(12.0, 0.0, 0.0)));
            other.push(character(&mut reference, Vec3::new(12.0, 0.0, 0.0)));
        }
    }
}

#[cfg(feature = "bench")]
#[test]
fn host_job_queue_reuses_slots_without_losing_jobs() {
    let mut w = JoltWorld::new(PhysicsConfig {
        threads: 8,
        shared_job_pool: true,
        max_bodies: 64,
        max_pairs: 1024,
        max_contacts: 1024,
        scratch_mb: 8,
        ..PhysicsConfig::game_default()
    })
    .expect("host worker fixture");
    w.backend_mut()
        .benchmark_job_probe(65536)
        .expect("queue wraparound within one boundary");
    w.backend_mut()
        .benchmark_job_probe(65536)
        .expect("queue reuse at the next boundary");
}
