#![allow(clippy::unwrap_used)]
use ara_anim::{
    data::*,
    glam::{Mat4, Quat, Vec3},
    ik::{GroundHit, GroundQuery},
    physical::*,
    *,
};
use ara_anim_physics::*;
use ara_physics::*;
use ara_physics_jolt::JoltWorld;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
fn skeleton() -> Skeleton {
    Skeleton::cook(SkeletonData {
        joints: (0..3)
            .map(|i| ara_anim::data::Joint {
                id: format!("bone{i}"),
                parent: (i > 0).then(|| i - 1),
                bind: AnimTransform {
                    translation: if i == 0 {
                        [0.0, 1.4, 0.0]
                    } else {
                        [0.0, 0.4, 0.0]
                    },
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    })
    .unwrap()
}
fn world(threads: u32) -> JoltWorld {
    let mut config = PhysicsConfig {
        threads,
        max_bodies: 128,
        max_pairs: 4096,
        max_contacts: 4096,
        event_capacity: 0,
        ..Default::default()
    };
    config.layer_masks[2] &= !(1 << 2);
    JoltWorld::new(config).unwrap()
}
fn floor(w: &mut JoltWorld) {
    let shape = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [10.0, 0.5, 10.0],
        })
        .unwrap();
    w.backend_mut()
        .create_bodies(
            &[BodyDesc {
                shape,
                user_data: 1,
                pose: PhysicsPose {
                    position: [0.0, -0.5, 0.0],
                    ..Default::default()
                },
                body: RigidBody::default(),
                collider: Collider::default(),
                velocity: Velocity::default(),
            }],
            &mut Vec::new(),
        )
        .unwrap();
}
fn profile() -> RagdollProfile {
    RagdollProfile {
        bones: (0..3)
            .map(|i| RagdollBone {
                joint: format!("bone{i}"),
                parent: (i > 0).then(|| i - 1),
                radius: 0.07,
                half_height: 0.12,
                mass: 2.0,
                ..Default::default()
            })
            .collect(),
    }
}
fn tick(
    w: &mut JoltWorld,
    rig: &JoltRagdoll,
    physical: &mut PhysicalAnimation,
    state: &mut PhysicalState,
    s: &Skeleton,
    pose: &mut Pose,
) {
    state.tick().unwrap();
    physical
        .drive(
            s,
            s.bind_pose(),
            RootMotion::default(),
            state,
            &mut rig.bind(w),
        )
        .unwrap();
    w.step().unwrap();
    physical
        .blend(
            s,
            s.bind_pose(),
            RootMotion::default(),
            state,
            &rig.bind(w),
            pose,
        )
        .unwrap();
}
#[test]
fn real_jolt_queries_powered_hit_death_getup_replay_and_allocations() {
    let s = skeleton();
    let p = profile();
    let mut reference = Vec::new();
    for threads in [1, 4, 8] {
        let mut w = world(threads);
        floor(&mut w);
        let rig = JoltRagdoll::spawn(&mut w, &s, &p, RootMotion::default(), 10, 2).unwrap();
        assert_eq!(rig.bodies().len(), 3);
        let mut physical = PhysicalAnimation::cook(&s, &p).unwrap();
        let mut state = PhysicalState::default();
        let mut pose = Pose::new(s.len());
        let filter = QueryFilter {
            layer_mask: 1,
            sensors: false,
            ignore: None,
        };
        let closest = JoltGround {
            backend: w.backend(),
            filter,
        }
        .probe(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, -4.0, 0.0))
        .unwrap()
        .unwrap();
        assert!(closest.point.y.abs() < 1e-5);
        let mut query = QueryResults::default();
        w.backend()
            .queries(
                &[Query::Ray {
                    origin: [0.0, 3.0, 0.0],
                    displacement: [0.0, -4.0, 0.0],
                    filter,
                }],
                &mut query,
            )
            .unwrap();
        assert_eq!(closest.point.to_array(), query.hits[0].point);
        for _ in 0..20 {
            tick(&mut w, &rig, &mut physical, &mut state, &s, &mut pose);
        }
        let snapshot = w.snapshot().unwrap();
        let saved = state;
        let run = |w: &mut JoltWorld,
                   physical: &mut PhysicalAnimation,
                   state: &mut PhysicalState,
                   pose: &mut Pose| {
            let mut outputs = Vec::new();
            for t in 0..180 {
                if t == 0 {
                    state.request(PhysicalMode::HitReaction, 8, 0.7).unwrap();
                    rig.bind(w)
                        .impulse(2, Vec3::X * 0.8, Vec3::new(0.0, 2.0, 0.0))
                        .unwrap();
                }
                if t == 20 {
                    state.request(PhysicalMode::Dead, 12, 1.0).unwrap();
                }
                if t == 80 {
                    state.request(PhysicalMode::GettingUp, 60, 0.0).unwrap();
                }
                tick(w, &rig, physical, state, &s, pose);
                let body = rig.bind(w).body(0).unwrap();
                outputs.push((body.pose, body.linear, body.angular, *state, pose.clone()));
            }
            outputs
        };
        let expected = run(&mut w, &mut physical, &mut state, &mut pose);
        assert_eq!(state.mode, PhysicalMode::Animated);
        assert_eq!(pose, s.bind_pose().clone());
        if threads == 1 {
            reference = expected.clone();
        } else {
            assert_eq!(expected, reference);
        }
        w.restore(&snapshot).unwrap();
        state = saved;
        assert_eq!(run(&mut w, &mut physical, &mut state, &mut pose), expected);
        let mut fresh = world(threads);
        floor(&mut fresh);
        let fresh_rig =
            JoltRagdoll::spawn(&mut fresh, &s, &p, RootMotion::default(), 10, 2).unwrap();
        assert_eq!(fresh_rig.bodies(), rig.bodies());
        fresh.restore(&snapshot).unwrap();
        state = saved;
        assert_eq!(
            run(&mut fresh, &mut physical, &mut state, &mut pose),
            expected
        );
        // Reuse the warmed native command/pose scratch; no snapshot serialization here.
        let region = Region::new(GLOBAL);
        for _ in 0..10 {
            let hit: Option<GroundHit> = JoltGround {
                backend: w.backend(),
                filter,
            }
            .probe(Vec3::Y * 3.0, -Vec3::Y * 4.0)
            .unwrap();
            std::hint::black_box(hit);
            tick(&mut w, &rig, &mut physical, &mut state, &s, &mut pose);
        }
        let counts = region.change();
        assert_eq!(
            (
                counts.allocations,
                counts.reallocations,
                counts.deallocations
            ),
            (0, 0, 0),
            "{counts:?}"
        );
        let mut model = vec![Mat4::IDENTITY; s.len()];
        ara_anim::pose::model_matrices(&s, &pose, &mut model).unwrap();
        assert!(model.iter().all(|m| m.is_finite()));
        assert!(
            physical
                .get_up_root(
                    RootMotion {
                        translation: Vec3::Y,
                        rotation: Quat::IDENTITY
                    },
                    RootMotion::default()
                )
                .translation
                .is_finite()
        );
    }
}
