//! Private format tests exercise selected bit rates, segments and raw fallbacks.
use crate::{data::Interpolation, quantization::Packed};

#[test]
fn segmented_sub_byte_keys_round_trip_with_conservative_error() {
    let keys: Vec<_> = (0..1024)
        .map(|i| {
            let base = (i / 32 % 2) as f32;
            (
                i * 3,
                [base + (i % 32) as f32 * 0.00001, base, base * 0.5, 0.0],
            )
        })
        .collect();
    let packed = Packed::cook(keys.clone(), 3, Interpolation::Linear, 0.0001, false);
    assert!(packed.segments.len() > 1);
    assert!(
        packed
            .segments
            .iter()
            .any(|s| s.bits.iter().any(|b| (1..8).contains(b)))
    );
    let bytes = postcard::to_allocvec(&packed).expect("test fixture serializes");
    let decoded: Packed = postcard::from_bytes(&bytes).expect("valid packed cache");
    decoded.validate(4000, 3).expect("valid segmented track");
    for (i, key) in keys.iter().enumerate() {
        assert_eq!(packed.decode(i), decoded.decode(i));
        let error =
            glam::Vec3::from_slice(&key.1).distance(glam::Vec3::from_slice(&decoded.decode(i)));
        assert!(error <= decoded.bound());
    }
}

#[test]
fn full_precision_and_constant_channels_round_trip() {
    let keys = vec![
        (7, [1000000.0, 1e-10, 0.0, 0.0]),
        (13, [-1000000.0, 2e-10, 0.0, 0.0]),
    ];
    let packed = Packed::cook(
        keys.clone(),
        3,
        Interpolation::Step,
        f32::MIN_POSITIVE,
        false,
    );
    assert!(packed.segments.iter().all(|s| s.bits.contains(&32)));
    let bytes = postcard::to_allocvec(&packed).expect("test fixture serializes");
    let decoded: Packed = postcard::from_bytes(&bytes).expect("valid raw cache");
    decoded.validate(100, 3).expect("valid raw track");
    for (i, key) in keys.iter().enumerate() {
        assert_eq!(key.1, decoded.decode(i));
    }
    let constant = Packed::cook(
        (0..100).map(|i| (i, [0.0, 1.0, 0.2, 0.0])).collect(),
        3,
        Interpolation::Linear,
        0.0001,
        false,
    );
    assert_eq!(constant.times.len(), 1);
    assert_eq!(constant.decode(0), [0.0, 1.0, 0.2, 0.0]);
}

#[test]
fn dominant_rotation_and_strict_budget_fallback_are_checked() {
    let keys: Vec<_> = (0..20)
        .map(|i| (i, glam::Quat::from_rotation_z(i as f32 * 0.02).to_array()))
        .collect();
    for budget in [0.00001, 0.0000001] {
        let packed = Packed::cook(keys.clone(), 4, Interpolation::Linear, budget, true);
        packed.validate(100, 4).expect("valid quaternion encoding");
        assert_eq!(packed.omitted < 4, budget > 8.0 * f32::EPSILON);
        let bytes = postcard::to_allocvec(&packed).expect("test fixture serializes");
        let decoded: Packed = postcard::from_bytes(&bytes).expect("valid rotation cache");
        for i in 0..keys.len() {
            assert_eq!(packed.decode(i), decoded.decode(i));
        }
    }
}

#[test]
fn time_major_packets_preserve_different_track_segment_boundaries() {
    let tracks: Vec<_> = (0..4)
        .map(|lane| {
            let keys = (0..1024)
                .map(|i| {
                    let base = (i / (8 << lane) % 2) as f32;
                    (
                        i * 3,
                        [base + (i % 8) as f32 * 0.00001, base, base * 0.5, 0.0],
                    )
                })
                .collect();
            crate::compression::Track {
                translation: Some(Packed::cook(keys, 3, Interpolation::Linear, 0.0001, false)),
                rotation: None,
                scale: None,
            }
        })
        .collect();
    let block = crate::packed_block::BlockKeys::new(
        [0, 1, 2, 3],
        crate::sampling_plan::Channel::Translation,
        &tracks,
    );
    assert!(
        tracks
            .iter()
            .all(|t| t.translation.as_ref().is_some_and(|t| t.segments.len() > 1))
    );
    for key in 0..1024 {
        let actual = block.decode(key, false);
        for (lane, track) in tracks.iter().enumerate() {
            let expected = track
                .translation
                .as_ref()
                .expect("fixture translation")
                .decode(key);
            for c in 0..3 {
                assert_eq!(actual[c][lane], expected[c]);
            }
        }
    }
}
