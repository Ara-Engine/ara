use super::*;

fn decode(words: [u32; 4]) -> [u16; 48] {
    let mut bytes = [0; 16];
    for (dst, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        dst.copy_from_slice(&word.to_le_bytes());
    }
    let mut decoded = [0; 48];
    bcdec_rs::bc6h_half(&bytes, &mut decoded, 12, false);
    decoded
}

fn random(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn input(half: [[u32; 3]; 16]) -> [u32; 32] {
    std::array::from_fn(|i| {
        if i % 2 == 0 {
            half[i / 2][0] | half[i / 2][1] << 16
        } else {
            half[i / 2][2]
        }
    })
}

fn fixture(case: u32) -> [u32; 32] {
    input(std::array::from_fn(|i| {
        std::array::from_fn(|c| match case {
            0 => (i as u32 * 160 + c as u32 * 113) + 0x3800, // HDR gradient.
            1 => {
                if i == 15 {
                    0x7bff - c as u32 * 64
                } else {
                    0x2400 + c as u32 * 100
                }
            } // Bright outlier.
            2 => i as u32 * 37 + c as u32 * 11, // Subnormals and the first normal exponent.
            3 => [0, 1, 0x3ff, 0x400, 0x7bfe, 0x7bff][(i + c) % 6], // Half boundaries.
            4 => {
                if i % 4 < 2 {
                    0x3c00 + c as u32 * 128
                } else {
                    0x6400 - c as u32 * 128
                }
            } // Two regions.
            5 => 0x7bff,                        // Maximum finite half.
            6 => 1,                             // Minimum subnormal.
            _ => 0,
        })
    }))
}

#[test]
fn every_mode_partition_transform_and_anchor_decodes_independently() {
    let mut state = 0x89abcde1;
    for mode in 0..14 {
        let bits = PRECISION[mode];
        let mask = (1 << bits) - 1;
        for partition in 0..if mode < 10 { 32 } else { 1 } {
            for _ in 0..8 {
                let mut q = [0; 12];
                for channel in &mut q[..3] {
                    *channel = random(&mut state) & mask;
                }
                for e in 1..if mode < 10 { 4 } else { 2 } {
                    for c in 0..3 {
                        q[e * 3 + c] = if mode == 9 || mode == 10 {
                            random(&mut state) & mask
                        } else {
                            let delta_bits = DELTA[mode][c];
                            let delta = (random(&mut state) & ((1 << delta_bits) - 1)) as i32
                                - (1 << (delta_bits - 1));
                            ((q[c] as i32 + delta) as u32) & mask
                        };
                    }
                }
                let indices: [u32; 16] = std::array::from_fn(|i| {
                    let anchor = i == 0 || (mode < 10 && i == ANCHORS[partition as usize] as usize);
                    let count = (if mode < 10 { 8 } else { 16 }) >> u32::from(anchor);
                    random(&mut state) & (count - 1)
                });
                let decoded = decode(pack(&q, &indices, mode, partition));
                for i in 0..16 {
                    for c in 0..3 {
                        let s = if mode < 10 { subset(partition, i) } else { 0 };
                        let a = unquantize(q[s * 6 + c], bits);
                        let b = unquantize(q[s * 6 + 3 + c], bits);
                        let w = weight(mode, indices[i]);
                        let expected = ((((64 - w) * a + w * b + 32) >> 6) * 31) >> 6;
                        assert_eq!(
                            u32::from(decoded[i * 3 + c]),
                            expected,
                            "mode {mode}, partition {partition}, pixel {i}, channel {c}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn palette_metric_matches_independent_decoder() {
    for case in 0..8 {
        let block = load(&fixture(case));
        for mode in 0..14 {
            let partition = case * 3;
            let mut endpoints = seed(&block, partition, mode < 10, case % 2);
            orient(&block, &mut endpoints, partition, mode < 10);
            let q = quantized(&endpoints, mode);
            let (error, indices) = assign(&block, &q, mode, partition);
            let decoded = decode(pack(&q, &indices, mode, partition));
            let mut independent = 0;
            for i in 0..16 {
                let h = std::array::from_fn(|c| u32::from(decoded[i * 3 + c]));
                independent += distance(&block, i, h, h.map(|v| linear(v, block.shift)));
            }
            assert_eq!(error, independent, "fixture {case}, mode {mode}");
        }
    }
}

fn decoded_error(pixels: &[u32; 32], words: [u32; 4]) -> u64 {
    let block = load(pixels);
    let decoded = decode(words);
    (0..16)
        .map(|i| {
            let h = std::array::from_fn(|c| u32::from(decoded[i * 3 + c]));
            distance(&block, i, h, h.map(|v| linear(v, block.shift)))
        })
        .sum()
}

#[test]
fn gradients_outliers_dark_hdr_and_half_boundaries_remain_finite() {
    for case in 0..8 {
        let pixels = fixture(case);
        let outputs = [0, 1, 2].map(|preset| encode_block(&pixels, preset));
        let errors = outputs.map(|words| decoded_error(&pixels, words));
        assert!(
            errors[2] <= errors[1] && errors[1] <= errors[0],
            "fixture {case}: {errors:?}"
        );
        for words in outputs {
            assert!(decode(words).iter().all(|&h| h <= 0x7bff));
        }
        if case == 1 {
            for c in 0..3 {
                assert!(
                    u32::from(decode(outputs[2])[45 + c])
                        .abs_diff(pixels[30 + c / 2] >> (16 * (c % 2)) & 65535)
                        < 300
                );
            }
        }
        if case == 5 || case == 6 || case == 7 {
            assert_eq!(errors[2], 0, "constant fixture {case}");
        }
    }
}

#[test]
fn quantization_is_nearest_including_zero_and_maximum() {
    for bits in [6, 7, 8, 9, 10, 11, 12, 16] {
        for h in [0, 1, 1023, 1024, 0x3bff, 0x3c00, 0x7bfe, 0x7bff] {
            let q = quantize(h, bits);
            let error = ((unquantize(q, bits) * 31) >> 6).abs_diff(h);
            for adjacent in q.saturating_sub(3)..=(q + 3).min((1 << bits) - 1) {
                assert!(error <= ((unquantize(adjacent, bits) * 31) >> 6).abs_diff(h));
            }
        }
    }
}

#[test]
fn deterministic_search_has_stable_ties_and_ignores_unused_blue_bits() {
    assert_eq!(encode_block(&[0; 32], 0), [3, 0, 0, 0]);
    assert_eq!(encode_block(&[0; 32], 1), [0; 4]);
    assert_eq!(encode_block(&[0; 32], 2), [0; 4]);
    let mut pixels = fixture(0);
    let expected = encode_block(&pixels, 2);
    for i in 0..16 {
        pixels[i * 2 + 1] |= 0xace10000;
    }
    assert_eq!(encode_block(&pixels, 2), expected);
    assert_eq!(encode_block(&pixels, 99), expected);
}

#[test]
fn seeded_hdr_corpus_hq_contains_balanced_candidates() {
    let mut state = 0x63a173b9;
    for _ in 0..16 {
        let pixels = input(std::array::from_fn(|_| {
            std::array::from_fn(|_| random(&mut state) % 0x7c00)
        }));
        let balanced = decoded_error(&pixels, encode_block(&pixels, 1));
        let hq = decoded_error(&pixels, encode_block(&pixels, 2));
        assert!(hq <= balanced, "{hq} > {balanced}");
    }
}

// Mirrors the specialized shader's normalized 32-bit division. The unchanged
// reference retains native 64-bit division as its independent arithmetic oracle.
fn split_divide(numerator: u64, denominator: u64) -> u32 {
    let shift = (63 - denominator.leading_zeros()).saturating_sub(15);
    let mut quotient = ((numerator >> shift) as u32) / ((denominator >> shift) as u32);
    if u64::from(quotient) * denominator > numerator {
        quotient -= 1;
    }
    quotient
}

#[test]
fn specialized_division_is_exact_for_every_quotient_and_wide_denominator_edges() {
    // Quotients are <=31743 for clamped fitting and <=1024 for PCA direction
    // normalization. After truncating the denominator to 16 significant bits,
    // its normalized value exceeds q+1, so at most one correction is needed.
    for denominator in [
        1,
        3,
        31,
        32767,
        32768,
        65535,
        65536,
        65537,
        (1 << 30) - 1,
        1 << 30,
        (1 << 32) + 1,
        (1 << 46) - 1,
    ] {
        for quotient in 0..=31743u64 {
            for remainder in [0, denominator / 2, denominator - 1] {
                let numerator = quotient * denominator + remainder;
                assert_eq!(
                    u64::from(split_divide(numerator, denominator)),
                    numerator / denominator
                );
            }
        }
    }
    let mut state = 0xa3d75b21;
    for _ in 0..100_000 {
        let denominator = u64::from(random(&mut state) % (1 << 30)) + 1;
        let numerator = i64::from(random(&mut state)) * i64::from(random(&mut state) % 65536)
            - i64::from(random(&mut state));
        let actual = if numerator <= 0 {
            0
        } else {
            let rounded = numerator as u64 + denominator / 2;
            if rounded >= 31743 * denominator {
                31743
            } else {
                split_divide(rounded, denominator)
            }
        };
        assert_eq!(
            i64::from(actual),
            round_div(numerator, denominator as i64).clamp(0, 31743)
        );
        let scale = (u64::from(random(&mut state)) << 14) + 1;
        let value = u64::from(random(&mut state)) % scale;
        assert_eq!(
            u64::from(split_divide(value * 1024, scale)),
            value * 1024 / scale
        );
    }
}

#[test]
fn specialized_quantizer_preserves_every_finite_unsigned_half_and_tie() {
    for bits in [6, 7, 8, 9, 10, 11, 12, 16] {
        for half in 0..=0x7bffu32 {
            let target = (half * 64 + 15) / 31;
            let maximum = (1u32 << bits) - 1;
            let center = (((target << bits) + 32768) >> 16).min(maximum);
            let mut best = (u32::MAX, 0);
            for trial in 0..5 {
                let q = (center as i32 + trial - 2).clamp(0, maximum as i32) as u32;
                let error = ((unquantize(q, bits) * 31) >> 6).abs_diff(half);
                if (error, q) < best {
                    best = (error, q);
                }
            }
            assert_eq!(best.1, quantize(half, bits));
        }
    }
}
