#![allow(dead_code)]
use ara_anim::{
    data::*,
    glam::{Quat, Vec3},
    *,
};
use ara_schema::AssetRef;
pub const STEP: u32 = 800;
pub fn skeleton(scale: f32, count: usize) -> Result<Skeleton, Error> {
    let bones = [
        ("root", None, [0.0, 0.0, 0.0]),
        ("Hips", Some(0), [0.0, 1.0, 0.0]),
        ("Spine", Some(1), [0.0, 0.18, 0.0]),
        ("Chest", Some(2), [0.0, 0.18, 0.0]),
        ("Neck", Some(3), [0.0, 0.15, 0.0]),
        ("Head", Some(4), [0.0, 0.14, 0.0]),
        ("LeftShoulder", Some(3), [-0.12, 0.1, 0.0]),
        ("LeftUpperArm", Some(6), [-0.12, 0.0, 0.0]),
        ("LeftLowerArm", Some(7), [-0.28, 0.0, 0.0]),
        ("LeftHand", Some(8), [-0.25, 0.0, 0.0]),
        ("RightShoulder", Some(3), [0.12, 0.1, 0.0]),
        ("RightUpperArm", Some(10), [0.12, 0.0, 0.0]),
        ("RightLowerArm", Some(11), [0.28, 0.0, 0.0]),
        ("RightHand", Some(12), [0.25, 0.0, 0.0]),
        ("LeftUpperLeg", Some(1), [-0.1, -0.06, 0.0]),
        ("LeftLowerLeg", Some(14), [0.0, -0.45, 0.0]),
        ("LeftFoot", Some(15), [0.0, -0.45, 0.0]),
        ("RightUpperLeg", Some(1), [0.1, -0.06, 0.0]),
        ("RightLowerLeg", Some(17), [0.0, -0.45, 0.0]),
        ("RightFoot", Some(18), [0.0, -0.45, 0.0]),
    ];
    let mut joints: Vec<_> = bones
        .into_iter()
        .map(|(name, parent, t)| Joint {
            id: name.into(),
            name: name.into(),
            parent,
            bind: AnimTransform {
                translation: (Vec3::from(t) * scale).to_array(),
                ..Default::default()
            },
            ..Default::default()
        })
        .collect();
    for i in joints.len()..count {
        joints.push(Joint {
            id: format!("helper{i}"),
            name: format!("Helper {i}"),
            parent: Some(3),
            ..Default::default()
        });
    }
    Skeleton::cook(SkeletonData {
        joints,
        ..Default::default()
    })
}
pub fn reference(index: usize) -> AssetRef {
    AssetRef(format!("@locomotion.animation#an_clip{index}"))
}
pub fn clip(s: &Skeleton, direction: Vec3, speed: f32, turn: f32) -> ClipData {
    let mut clip = ClipData {
        duration: 192000,
        tracks: vec![JointTrack::default(); s.len()],
        ..Default::default()
    };
    for frame in 0..=240 {
        let t = frame as f32 / 60.0;
        let phase = t * std::f32::consts::TAU * if speed > 2.0 { 2.0 } else { 1.0 };
        let yaw = turn * t;
        let translation = if turn.abs() < 1e-6 {
            direction * speed * t
        } else {
            Vec3::new(
                (1.0 - yaw.cos()) * speed / turn,
                0.0,
                yaw.sin() * speed / turn,
            )
        };
        clip.tracks[0].translations.push(VectorKey {
            tick: frame * STEP,
            value: translation.to_array(),
        });
        clip.tracks[0].rotations.push(RotationKey {
            tick: frame * STEP,
            value: Quat::from_rotation_y(yaw).to_array(),
        });
        for (joint, offset) in [(14, 0.0), (17, std::f32::consts::PI)] {
            let swing = (phase + offset).sin() * 0.45 * (speed / 1.2).min(1.5);
            clip.tracks[joint].rotations.push(RotationKey {
                tick: frame * STEP,
                value: Quat::from_rotation_x(swing).to_array(),
            });
            clip.tracks[joint + 1].rotations.push(RotationKey {
                tick: frame * STEP,
                value: Quat::from_rotation_x((-swing).max(0.0) * 1.5).to_array(),
            });
        }
        for (joint, offset) in [(7, std::f32::consts::PI), (11, 0.0)] {
            clip.tracks[joint].rotations.push(RotationKey {
                tick: frame * STEP,
                value: Quat::from_rotation_x((phase + offset).sin() * 0.2 * speed.min(2.0))
                    .to_array(),
            });
        }
    }
    clip
}
pub fn locomotion(s: &Skeleton) -> Result<Vec<CookedClip>, Error> {
    [
        (Vec3::Z, 1.2, 0.0),
        (Vec3::Z, 3.2, 0.0),
        (Vec3::X, 1.2, 0.0),
        (-Vec3::X, 1.2, 0.0),
        (Vec3::Z, 1.2, 0.8),
        (Vec3::Z, 1.2, -0.8),
        (-Vec3::Z, 1.0, 0.0),
        (Vec3::ZERO, 0.0, 0.0),
    ]
    .into_iter()
    .map(|(d, speed, turn)| {
        CookedClip::cook(&clip(s, d, speed, turn), s, CompressionSettings::default()).map(|c| c.0)
    })
    .collect()
}
