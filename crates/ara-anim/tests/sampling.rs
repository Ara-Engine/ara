#![allow(clippy::unwrap_used)]
extern crate glam34 as glam;
mod common;
use ara_anim::{
    data::*,
    glam::{Quat, Vec3},
    *,
};

#[test]
fn quantized_sampling_matches_reference_and_certified_bounds() {
    let skeleton = common::skeleton(65);
    let source = common::clip(&skeleton, 1.0);
    let settings = CompressionSettings::default();
    let (clip, report) = CookedClip::cook(&source, &skeleton, settings).unwrap();
    assert!(report.translation_bound <= settings.translation);
    assert!(report.rotation_bound <= settings.rotation);
    assert!(report.scale_bound <= settings.scale);
    assert!(report.translation_error <= report.translation_bound);
    assert!(report.rotation_error <= report.rotation_bound);
    assert!(report.source_bytes > report.cooked_bytes * 2);
    assert!(report.source_bytes >= report.cooked_bytes * 10);
    let bytes = clip.to_bytes().unwrap();
    let decoded = CookedClip::from_bytes(&bytes, &skeleton).unwrap();
    let mut sampler = Sampler::new(&decoded);
    let mut pose = Pose::new(65);
    // Prime, reverse, seek and exact endpoint exercise cache invalidation and brackets.
    for tick in (0..=48000).step_by(137).chain([48000, 10000, 0, 24000]) {
        let mut curves = [0.0];
        sampler
            .sample(&decoded, &skeleton, tick, &mut pose, &mut curves)
            .unwrap();
        for bone in 0..65 {
            let track = &source.tracks[bone];
            let frame = (tick / 800).min(59) as usize;
            let t = (tick - frame as u32 * 800) as f32 / 800.0;
            let a = Vec3::from_array(track.translations[frame].value);
            let b = Vec3::from_array(track.translations[frame + 1].value);
            let actual: Vec3 = pose.translations()[bone].into();
            assert!(actual.distance(a.lerp(b, t)) <= settings.translation);
            let a = Quat::from_array(track.rotations[frame].value);
            let mut b = Quat::from_array(track.rotations[frame + 1].value);
            if a.dot(b) < 0.0 {
                b = -b;
            }
            let expected = (a * (1.0 - t) + b * t).normalize();
            let actual = pose.rotations()[bone];
            assert!(
                (actual - expected)
                    .length()
                    .min((actual + expected).length())
                    * 2.0
                    <= settings.rotation
            );
        }
        common::near(curves[0], tick as f32 / 48000.0, settings.curve);
    }
    assert_eq!(bytes, decoded.to_bytes().unwrap());
    assert!(CookedClip::from_bytes(&bytes[..bytes.len() - 1], &skeleton).is_err());
    assert!(CookedClip::from_bytes(&[bytes.as_slice(), &[0]].concat(), &skeleton).is_err());
    assert!(CookedClip::from_bytes(&bytes, &common::skeleton(64)).is_err());
}

#[test]
fn presentation_lod_and_compatible_clip_rebinding_preserve_accuracy() {
    let s = common::skeleton(5);
    let empty = ClipData {
        duration: 48000,
        tracks: vec![JointTrack::default(); 5],
        ..Default::default()
    };
    let (empty, _) = CookedClip::cook(&empty, &s, CompressionSettings::default()).unwrap();
    let mut source = common::clip(&s, 1.0);
    source.curves.clear();
    let (clip, _) = CookedClip::cook(&source, &s, CompressionSettings::default()).unwrap();
    let mut reused = Sampler::new(&empty);
    let mut fresh = Sampler::new(&clip);
    let mut actual = Pose::new(5);
    let mut expected = Pose::new(5);
    for tick in [0, 137, 32000, 48000, 17] {
        reused
            .sample(&clip, &s, tick, &mut actual, &mut [])
            .unwrap();
        fresh
            .sample(&clip, &s, tick, &mut expected, &mut [])
            .unwrap();
        for i in 0..5 {
            assert!(actual.translations()[i].distance(expected.translations()[i]) < 1e-6);
            assert!((actual.rotations()[i] - expected.rotations()[i]).length() < 1e-6);
        }
        for prefix in [0, 1, 3, 4, 5] {
            fresh
                .sample_prefix(&clip, &s, tick, prefix, &mut actual, &mut [])
                .unwrap();
            for i in 0..prefix {
                assert_eq!(actual.joint(i), expected.joint(i));
            }
            for i in prefix..5 {
                assert_eq!(actual.joint(i), s.bind_pose().joint(i));
            }
        }
        reused
            .sample(&empty, &s, tick, &mut actual, &mut [])
            .unwrap();
        assert_eq!(actual, *s.bind_pose());
    }
    let before = actual.clone();
    assert!(
        fresh
            .sample_prefix(&clip, &s, 0, 6, &mut actual, &mut [])
            .is_err()
    );
    assert_eq!(before, actual);
}

#[test]
fn irregular_step_scale_and_full_rotation_packets_match_after_loading() {
    let s = common::skeleton(5);
    let axis = Vec3::new(1.0, 2.0, 3.0).normalize();
    let source = ClipData {
        duration: 100,
        tracks: (0..5)
            .map(|joint| JointTrack {
                translations: [0, 31 + joint, 100]
                    .into_iter()
                    .map(|tick| VectorKey {
                        tick,
                        value: [tick as f32 * 0.001, joint as f32, 0.0],
                    })
                    .collect(),
                rotations: [0, 27, 61, 100]
                    .into_iter()
                    .zip([-3.0, -1.0, 1.0, 3.0])
                    .map(|(tick, angle)| RotationKey {
                        tick,
                        value: Quat::from_axis_angle(axis, angle).to_array(),
                    })
                    .collect(),
                scales: [0, 19 + joint, 100]
                    .into_iter()
                    .map(|tick| VectorKey {
                        tick,
                        value: [1.0 + tick as f32 * 0.0001, 1.0, 1.0],
                    })
                    .collect(),
                translation_interpolation: if joint % 2 == 0 {
                    Interpolation::Step
                } else {
                    Interpolation::Linear
                },
                rotation_interpolation: Interpolation::Linear,
                scale_interpolation: Interpolation::Linear,
            })
            .collect(),
        ..Default::default()
    };
    let (clip, report) = CookedClip::cook(&source, &s, CompressionSettings::default()).unwrap();
    assert!(report.translation_error <= report.translation_bound);
    assert!(report.rotation_error <= report.rotation_bound);
    assert!(report.scale_error <= report.scale_bound);
    let loaded = CookedClip::from_bytes(&clip.to_bytes().unwrap(), &s).unwrap();
    let mut warm = Sampler::new(&clip);
    let mut cold = Sampler::new(&loaded);
    let mut a = Pose::new(5);
    let mut b = Pose::new(5);
    for tick in (0..=100).chain([90, 27, 0, 100]) {
        warm.sample(&clip, &s, tick, &mut a, &mut []).unwrap();
        cold.reset();
        cold.sample(&loaded, &s, tick, &mut b, &mut []).unwrap();
        assert_eq!(a, b);
    }
}

#[test]
fn sparse_step_constant_and_half_turn_channels() {
    let s = common::skeleton(2);
    let source = ClipData {
        duration: 100,
        tracks: vec![
            JointTrack {
                translations: vec![
                    VectorKey {
                        tick: 25,
                        value: [1.0, 2.0, 3.0],
                    },
                    VectorKey {
                        tick: 75,
                        value: [4.0, 2.0, 3.0],
                    },
                ],
                translation_interpolation: Interpolation::Step,
                ..Default::default()
            },
            JointTrack {
                rotations: vec![
                    RotationKey {
                        tick: 0,
                        value: Quat::IDENTITY.to_array(),
                    },
                    RotationKey {
                        tick: 100,
                        value: Quat::from_rotation_x(std::f32::consts::PI).to_array(),
                    },
                ],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let (c, _) = CookedClip::cook(&source, &s, CompressionSettings::default()).unwrap();
    let mut sampler = Sampler::new(&c);
    let mut p = Pose::new(2);
    for (t, expected) in [(0, 1.0), (74, 1.0), (75, 4.0), (100, 4.0), (24, 1.0)] {
        sampler.sample(&c, &s, t, &mut p, &mut []).unwrap();
        common::near(p.translations()[0].x, expected, 1e-4);
        assert_eq!(p.scales(), s.bind_pose().scales());
    }
    let mut invalid = source.clone();
    invalid.tracks[0].translations[1].tick = 25;
    assert!(CookedClip::cook(&invalid, &s, CompressionSettings::default()).is_err());
    invalid = source;
    invalid.tracks[0].translations[0].value[0] = f32::NAN;
    assert!(CookedClip::cook(&invalid, &s, CompressionSettings::default()).is_err());
}

#[test]
fn root_motion_composes_rotation_and_translation_across_many_loops() {
    let s = common::skeleton(1);
    let data = ClipData {
        duration: 100,
        tracks: vec![JointTrack {
            translations: vec![
                VectorKey {
                    tick: 0,
                    value: [3.0, 0.0, 0.0],
                },
                VectorKey {
                    tick: 100,
                    value: [4.0, 0.0, 0.0],
                },
            ],
            rotations: vec![
                RotationKey {
                    tick: 0,
                    value: Quat::from_rotation_y(0.3).to_array(),
                },
                RotationKey {
                    tick: 100,
                    value: Quat::from_rotation_y(0.3 + std::f32::consts::FRAC_PI_2).to_array(),
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let (c, _) = CookedClip::cook(&data, &s, CompressionSettings::default()).unwrap();
    let whole = c.root_motion(&s, 25, 425, true).unwrap();
    let mut parts = RootMotion::default();
    for tick in (25..425).step_by(10) {
        parts = parts.compose(c.root_motion(&s, tick, tick + 10, true).unwrap());
    }
    assert!(parts.translation.distance(whole.translation) < 1e-4);
    assert!(parts.rotation.dot(whole.rotation).abs() > 0.99999);
    assert!(whole.translation.length() < 0.001); // Four turning loops close the square.
    let once = c.root_motion(&s, 0, 100, false).unwrap();
    let stopped = c.root_motion(&s, 0, 10000, false).unwrap();
    assert_eq!(once, stopped);
    assert_eq!(
        c.root_motion(&s, 100, 101, false).unwrap(),
        RootMotion::default()
    );
}
