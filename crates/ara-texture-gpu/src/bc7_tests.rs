//! Independent bcdec decoding checks packing, fix-ups and the encoder's exact metric.
use super::*;

fn decoded(words: [u32; 4]) -> [u8; 64] {
    let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    let mut rgba = [0; 64];
    bcdec_rs::bc7(&bytes, &mut rgba, 16);
    rgba
}

fn error(pixels: &[u32; 16], words: [u32; 4]) -> u32 {
    let rgba = decoded(words);
    pixels
        .iter()
        .enumerate()
        .map(|(i, pixel)| {
            (0..4)
                .map(|c| {
                    let d = ((pixel >> (c * 8)) & 255) as i32 - i32::from(rgba[i * 4 + c]);
                    (d * d) as u32
                })
                .sum::<u32>()
        })
        .sum()
}

fn corpus() -> Vec<[u32; 16]> {
    let mut blocks = Vec::new();
    for color in [0, u32::MAX, 0xff000000, 0xff808080, 0x0180ff00, 0x7f0180fe] {
        blocks.push([color; 16]);
    }
    blocks.push(std::array::from_fn(|i| {
        let x = (i % 4) as u32;
        let y = (i / 4) as u32;
        (x * 85) | ((y * 85) << 8) | (((x + y) * 42) << 16) | 0xff000000
    }));
    blocks.push(std::array::from_fn(|i| {
        let x = (i % 4) as u32;
        let y = (i / 4) as u32;
        (96 + x * 21) | ((96 + y * 21) << 8) | ((255 - (x + y) * 7) << 16) | 0xff000000
    }));
    for mask in [0x0001_u32, 0x8000, 0xaaaa, 0x5a5a, 0x6996, 0xfffe] {
        blocks.push(std::array::from_fn(|i| {
            if mask & (1 << i) == 0 {
                0x00f3124c
            } else {
                0xff17e891
            }
        }));
    }
    // Smooth RGB with an unrelated alpha mask exercises separate index streams.
    blocks.push(std::array::from_fn(|i| {
        let v = i as u32 * 17;
        v | (v << 8) | ((255 - v) << 16) | (if i % 3 == 0 { 255 << 24 } else { 0 })
    }));
    let mut state = 0x9e3779b9_u32;
    for n in 0..12 {
        blocks.push(std::array::from_fn(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if n % 2 == 0 {
                state | 0xff000000
            } else {
                state
            }
        }));
    }
    blocks
}

#[test]
fn every_mode_partition_rotation_and_index_selection_matches_independent_decoder() {
    // Force all candidates, including ones which lose the final search. This
    // catches wrong anchor tables/endpoint order even when mode 6 would hide it.
    for (block_id, pixels) in corpus().iter().enumerate() {
        for id in 0..candidate_count(2) {
            let (mode, partition, rotation, selection) = candidate_parameters(id, 2);
            let (metric, words) = candidate(pixels, mode, partition, rotation, selection, 2);
            assert_eq!(words[0].trailing_zeros(), mode as u32);
            assert_eq!(
                error(pixels, words),
                metric,
                "block {block_id}, mode {mode}, partition {partition}, rotation {rotation}, selection {selection}"
            );
        }
    }
}

#[test]
fn presets_are_nested_and_repeat_byte_for_byte() {
    for pixels in corpus() {
        let fast = encode_block(&pixels, 0);
        let balanced = encode_block(&pixels, 1);
        let hq = encode_block(&pixels, 2);
        assert!(error(&pixels, balanced) <= error(&pixels, fast));
        assert!(error(&pixels, hq) <= error(&pixels, balanced));
        assert_eq!(encode_block(&pixels, 2), hq);
        assert_eq!(encode_block(&pixels, u32::MAX), hq);
    }
}

#[test]
fn candidate_enumeration_is_complete_and_unique() {
    let mut counts = [0; 8];
    let mut seen = std::collections::BTreeSet::new();
    for id in 0..candidate_count(2) {
        let parameters = candidate_parameters(id, 2);
        assert!(seen.insert(parameters));
        counts[parameters.0] += 1;
    }
    assert_eq!(counts, [16, 64, 64, 64, 8, 4, 1, 64]);
    for id in 0..candidate_count(1) {
        assert!(seen.contains(&candidate_parameters(id, 1)));
    }
}

#[test]
fn exact_opaque_extremes_and_binary_alpha_survive() {
    for color in [0xff000000, 0xffffffff, 0xff00ff00, 0xffff0000, 0xff0000ff] {
        assert_eq!(error(&[color; 16], encode_block(&[color; 16], 2)), 0);
    }
    for pixels in corpus().into_iter().skip(8).take(6) {
        let rgba = decoded(encode_block(&pixels, 2));
        for i in 0..16 {
            assert_eq!(u32::from(rgba[i * 4 + 3]), pixels[i] >> 24);
        }
    }
}

#[test]
fn singular_fits_preserve_later_pbit_requantization() {
    // Frozen from 2bea58c1. Stopping a constant plane after its first p-bit
    // search chooses mode 6 here; later re-quantization makes mode 7 better.
    assert_eq!(
        encode_block(&[0xf4d8257f; 16], 1),
        [0xf07c0080, 0xd6c85215, 0x3def7b5b, 0xaaaaaaac]
    );
}

#[test]
fn exact_mode6_coarse_fit_is_the_same_full_fit_and_final_winner() {
    let mut rng = 0x6a09e667_u32;
    for case in 0..128_u32 {
        let mut endpoints = [[0_i32; 4]; 2];
        for (e, endpoint) in endpoints.iter_mut().enumerate() {
            let parity = (case >> e) & 1;
            for channel in endpoint {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                *channel = ((rng & 254) | parity) as i32;
            }
        }
        // Ensure a nonuniform palette with both endpoint orders at the anchor.
        endpoints[1][0] = (endpoints[0][0] ^ 128) & 254 | ((case >> 1) & 1) as i32;
        let pixels = std::array::from_fn(|i| {
            let index = if case & 1 == 0 { i } else { 15 - i };
            let weight = WEIGHTS[2][index];
            let mut pixel = 0;
            for (c, (&low, &high)) in endpoints[0].iter().zip(&endpoints[1]).enumerate() {
                let value = ((64 - weight) * low + weight * high + 32) >> 6;
                pixel |= (value as u32) << (c * 8);
            }
            pixel
        });
        assert_ne!(pixels[0], pixels[15]);
        let coarse = candidate(&pixels, 6, 0, 0, 0, 3);
        assert_eq!(
            coarse.0, 0,
            "case {case} must be a lossless nonuniform mode-6 block"
        );
        assert_eq!(error(&pixels, coarse.1), 0);
        for preset in [1, 2] {
            assert_eq!(candidate(&pixels, 6, 0, 0, 0, preset), coarse);
            assert_eq!(encode_block(&pixels, preset), coarse.1);
        }
    }
}

#[test]
fn shader_quantization_table_matches_exhaustive_endpoint_search() {
    let words: Vec<u32> = include_str!("../shaders/bc7_quant.slangh")
        .split(|c: char| c.is_ascii_whitespace() || matches!(c, ',' | '{' | '}' | ';'))
        .filter_map(|word| word.strip_prefix("0x"))
        .map(|hex| {
            u32::from_str_radix(hex, 16).expect("quantization table has valid hexadecimal words")
        })
        .collect();
    assert_eq!(words.len(), 704);
    for table in 0..11 {
        let has_p = table < 8;
        let bits = if has_p { 4 + table / 2 } else { table - 3 } as u32;
        let p = if has_p { table as u32 & 1 } else { 0 };
        let precision = bits + u32::from(has_p);
        for value in 0..256 {
            let expected = (0..1 << bits)
                .min_by_key(|&q| {
                    let v = ((q << u32::from(has_p)) | p) << (8 - precision);
                    let decoded = (v | (v >> precision)) as i32;
                    let d = value as i32 - decoded;
                    (d * d, q)
                })
                .expect("each BC7 endpoint precision has representable values");
            let actual = (words[table * 64 + value / 4] >> ((value & 3) * 8)) & 255;
            assert_eq!(actual, expected, "table {table}, input {value}");
            assert_eq!(quantize(value as i32, bits, has_p, p).0, expected);
        }
    }
}
