#![allow(clippy::unwrap_used)]
use ara_anim::{
    data::*,
    glam::{Mat4, Quat, Vec3},
    ik::*,
    pose::model_matrices,
    retarget::*,
    *,
};

fn limb() -> Skeleton {
    Skeleton::cook(SkeletonData {
        joints: (0..3)
            .map(|i| Joint {
                id: format!("j{i}"),
                parent: (i > 0).then(|| i - 1),
                bind: AnimTransform {
                    translation: if i == 0 { [0.0; 3] } else { [0.8, 0.0, 0.0] },
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    })
    .unwrap()
}
fn data() -> ChainData {
    ChainData {
        joints: vec!["j0".into(), "j1".into(), "j2".into()],
        ..Default::default()
    }
}
#[test]
fn reach_convergence_unreachable_and_replay() {
    let s = limb();
    for solver in [ChainSolver::Fabrik, ChainSolver::Ccd] {
        for goal in [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.9, 0.7, 0.0),
            Vec3::new(-0.5, 0.8, 0.4),
            Vec3::new(4.0, 0.0, 0.0),
        ] {
            let mut a = s.bind_pose().clone();
            let mut b = s.bind_pose().clone();
            let mut chain = Chain::cook(&s, &data()).unwrap();
            let settings = SolverSettings {
                iterations: 128,
                ..Default::default()
            };
            let report = chain.solve(&s, &mut a, goal, solver, settings).unwrap();
            chain.solve(&s, &mut b, goal, solver, settings).unwrap();
            assert_eq!(a, b);
            if goal.length() < 1.6 {
                assert!(report.error < 0.001, "{solver:?} {report:?}");
            } else {
                assert!((report.error - 2.4).abs() < 0.001);
            }
            let mut model = vec![Mat4::IDENTITY; 3];
            model_matrices(&s, &a, &mut model).unwrap();
            assert!((model[1].w_axis.distance(model[2].w_axis) - 0.8).abs() < 1e-5);
        }
    }
}
#[test]
fn hinge_and_swing_twist_limits_are_geometric() {
    let limit = JointLimit {
        kind: LimitKind::Hinge,
        axis: [0.0, 0.0, 1.0],
        min: -0.2,
        max: 0.4,
        ..Default::default()
    };
    let q = limit
        .project(Quat::from_rotation_z(1.0) * Quat::from_rotation_x(0.7))
        .unwrap();
    assert!(q.angle_between(Quat::from_rotation_z(0.4)) < 0.001);
    let cone = JointLimit {
        kind: LimitKind::Cone,
        axis: [1.0, 0.0, 0.0],
        swing: 0.3,
        min: -0.2,
        max: 0.2,
    };
    let q = cone
        .project(Quat::from_rotation_y(1.0) * Quat::from_rotation_x(0.9))
        .unwrap();
    assert!((q * Vec3::X).angle_between(Vec3::X) <= 0.3001);
    let s = limb();
    for solver in [ChainSolver::Fabrik, ChainSolver::Ccd] {
        let mut chain = Chain::cook(
            &s,
            &ChainData {
                limits: vec![limit; 2],
                ..data()
            },
        )
        .unwrap();
        let mut pose = s.bind_pose().clone();
        chain
            .solve(
                &s,
                &mut pose,
                Vec3::new(0.0, 1.6, 0.7),
                solver,
                SolverSettings::default(),
            )
            .unwrap();
        for q in pose.rotations().iter().take(2) {
            assert!(q.to_scaled_axis().z <= 0.40001);
            assert!(q.x.abs() < 1e-6 && q.y.abs() < 1e-6);
        }
        let mut chain = Chain::cook(
            &s,
            &ChainData {
                limits: vec![
                    JointLimit {
                        kind: LimitKind::Hinge,
                        axis: [0.0, 0.0, 1.0],
                        ..Default::default()
                    };
                    2
                ],
                ..data()
            },
        )
        .unwrap();
        let mut pose = s.bind_pose().clone();
        let report = chain
            .solve(
                &s,
                &mut pose,
                -Vec3::X * 4.0,
                solver,
                SolverSettings {
                    iterations: 128,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            (report.error - 2.4).abs() < 0.001,
            "opposed hinge target: {solver:?} {report:?}"
        );
    }
}
#[test]
fn shared_chain_priority_and_prop_grip() {
    let s = limb();
    let mut body = FullBody::new(&s, 0, &[data()], 2).unwrap();
    let high = Vec3::new(0.9, 0.7, 0.0);
    let goals = [
        BodyGoal {
            chain: 0,
            position: high,
            rotation: None,
            priority: 100,
            pelvis_weight: 0.0,
        },
        BodyGoal {
            chain: 0,
            position: Vec3::new(0.5, -0.7, 0.0),
            rotation: None,
            priority: 0,
            pelvis_weight: 0.0,
        },
    ];
    let mut pose = s.bind_pose().clone();
    body.solve(&s, &mut pose, &goals, 0.0, SolverSettings::default())
        .unwrap();
    let mut model = vec![Mat4::IDENTITY; 3];
    model_matrices(&s, &pose, &mut model).unwrap();
    assert!(model[2].w_axis.truncate().distance(high) < 0.001);
    let grip = HandGrip {
        prop: RootMotion {
            translation: high,
            rotation: Quat::from_rotation_z(0.2),
        },
        local: RootMotion::default(),
    };
    grip.solve(
        RootMotion::default(),
        &mut Chain::cook(&s, &data()).unwrap(),
        &s,
        &mut pose,
        &mut model,
    )
    .unwrap();
    model_matrices(&s, &pose, &mut model).unwrap();
    assert!(
        model[2]
            .to_scale_rotation_translation()
            .1
            .angle_between(grip.prop.rotation)
            < 0.001
    );
}
fn human(scale: f32, ue: bool) -> Skeleton {
    let names = if ue {
        ["pelvis", "thigh_l", "calf_l", "foot_l"]
    } else {
        [
            "mixamorig:Hips",
            "mixamorig:LeftUpLeg",
            "unnamed",
            "mixamorig:LeftFoot",
        ]
    };
    Skeleton::cook(SkeletonData {
        joints: names
            .iter()
            .enumerate()
            .map(|(i, n)| Joint {
                id: format!("h{i}"),
                name: (*n).into(),
                parent: (i > 0).then(|| (i - 1) as u32),
                bind: AnimTransform {
                    translation: if i == 0 {
                        [0.0, scale, 0.0]
                    } else if i == 1 {
                        [0.1 * scale, 0.0, 0.0]
                    } else {
                        [0.0, -0.5 * scale, 0.0]
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
#[test]
fn automap_topology_and_retarget_plants() {
    let source = human(1.0, false);
    let target = human(1.4, true);
    let sr = RigMap::auto(&source).unwrap();
    let tr = RigMap::auto(&target).unwrap();
    assert_eq!(sr.joint(&source, BoneRole::LeftLowerLeg), Some(2));
    let mut retarget = Retargeter::cook(&source, &sr, &target, &tr).unwrap();
    let mut pose = target.bind_pose().clone();
    retarget
        .apply(&source, source.bind_pose(), &target, &mut pose, true)
        .unwrap();
    let mut model = vec![Mat4::IDENTITY; 4];
    model_matrices(&target, &pose, &mut model).unwrap();
    let millimeters = model[3].w_axis.y.abs() * 1000.0;
    assert!(millimeters < 0.1, "plant error {millimeters} mm");
    let mut source_pose = source.bind_pose().clone();
    let mut source_model = vec![Mat4::IDENTITY; source.len()];
    let mut max_error = 0.0_f32;
    for frame in 0..60 {
        source_pose.copy_from(source.bind_pose()).unwrap();
        let phase = frame as f32 / 60.0 * std::f32::consts::TAU;
        let mut hips = source_pose.joint(0).unwrap();
        hips.translation += glam::Vec3A::new(phase.sin() * 0.1, -0.08, 0.0);
        source_pose.set_joint(0, hips).unwrap();
        model_matrices(&source, &source_pose, &mut source_model).unwrap();
        let (a, b) = two_bone(
            source_model[1],
            source_model[2],
            source_model[3],
            Vec3::new(0.1, 0.0, 0.0),
            Vec3::Z,
            1.0,
        )
        .unwrap();
        apply_model_rotation(&source, &mut source_pose, &source_model, 1, a).unwrap();
        model_matrices(&source, &source_pose, &mut source_model).unwrap();
        apply_model_rotation(&source, &mut source_pose, &source_model, 2, b).unwrap();
        retarget
            .apply(&source, &source_pose, &target, &mut pose, true)
            .unwrap();
        model_matrices(&target, &pose, &mut model).unwrap();
        max_error = max_error.max(
            model[3]
                .w_axis
                .truncate()
                .distance(Vec3::new(0.14, 0.0, 0.0))
                * 1000.0,
        );
    }
    println!("retarget_60_planted_poses_1.4x_max_error_mm={max_error:.6}");
    assert!(max_error < 0.2, "animated plant error {max_error} mm");
}

struct Plane;
impl GroundQuery for Plane {
    fn probe(&mut self, origin: Vec3, displacement: Vec3) -> Result<Option<GroundHit>, Error> {
        let normal = Vec3::new(0.0, 1.0, -0.2).normalize();
        let t = -origin.dot(normal) / displacement.dot(normal);
        Ok((0.0..=1.0).contains(&t).then_some(GroundHit {
            point: origin + displacement * t,
            normal,
        }))
    }
}
#[test]
fn feet_probe_world_space_adjust_pelvis_and_align_slopes() {
    let s = human(1.0, true);
    let mut placement = FootPlacement::new(
        &s,
        0,
        &[Foot {
            chain: ChainData {
                joints: vec!["h1".into(), "h2".into(), "h3".into()],
                ..Default::default()
            },
            ..Default::default()
        }],
        FootSettings::default(),
    )
    .unwrap();
    let mut pose = s.bind_pose().clone();
    let actor = RootMotion {
        translation: Vec3::new(0.0, 0.15, 0.0),
        rotation: Quat::from_rotation_y(0.3),
    };
    let offset = placement.solve(&s, &mut pose, actor, &mut Plane).unwrap();
    assert!((offset + 0.15591).abs() < 0.01);
    let mut matrices = vec![Mat4::IDENTITY; s.len()];
    model_matrices(&s, &pose, &mut matrices).unwrap();
    let foot = actor.translation + actor.rotation * matrices[3].w_axis.truncate();
    assert!(foot.dot(Vec3::new(0.0, 1.0, -0.2).normalize()).abs() < 0.001);
    let rotation = matrices[3].to_scale_rotation_translation().1;
    assert!(
        (actor.rotation * rotation * Vec3::Y).distance(Vec3::new(0.0, 1.0, -0.2).normalize())
            < 0.001
    );
}

#[test]
fn offline_retarget_bakes_checked_clip_keys_and_preserves_timebase() {
    let source = human(1.0, false);
    let target = human(1.4, true);
    let mut retarget = Retargeter::cook(
        &source,
        &RigMap::auto(&source).unwrap(),
        &target,
        &RigMap::auto(&target).unwrap(),
    )
    .unwrap();
    let clip = ClipData {
        duration: 4800,
        tracks: vec![JointTrack::default(); source.len()],
        ..Default::default()
    };
    let baked = retarget.bake(&source, &clip, &target, 800).unwrap();
    assert_eq!(baked.duration, clip.duration);
    assert_eq!(baked.ticks_per_second, clip.ticks_per_second);
    assert_eq!(baked.tracks[0].translations.len(), 7);
    let cooked = CookedClip::cook(&baked, &target, CompressionSettings::default())
        .unwrap()
        .0;
    let mut sampler = Sampler::new(&cooked);
    let mut pose = Pose::new(target.len());
    sampler
        .sample(&cooked, &target, 2400, &mut pose, &mut [])
        .unwrap();
    let mut model = vec![Mat4::IDENTITY; target.len()];
    model_matrices(&target, &pose, &mut model).unwrap();
    assert!(model[3].w_axis.y.abs() < 0.0002);
}
