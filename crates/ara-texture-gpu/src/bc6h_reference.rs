//! Integer BC6H UF16 reference for `shaders/bc6h.slang`.
//!
//! The metric is 4 * squared unsigned-half-code error plus squared linear-light
//! error, with linear light scaled once per block to at most 15 significant bits.
//! This preserves dark detail without ignoring bright outliers. No host floats,
//! wave operations, or implementation-dependent arithmetic affect the result.
//! HQ considers all 14 modes / 32 partitions with two seeds and four fit updates.
//! Balanced uses the best eight ranked partitions, four two-subset modes, all
//! single-subset modes, one seed and two updates. Fast uses mode 11 with no update.
//!
//! For valid input, half codes are <=31743 and scaled linear values <=32752.
//! Block error is <2^38; covariance entries are <2^34; the largest PCA intermediate
//! is <2^56; least-squares numerators are <2^48. Thus signed fits and unsigned error
//! sums fit in 64 bits. Integer division truncates toward zero on both CPU and GPU;
//! fit rounding is explicit. Ties use indices first, then output words 0 through 3.

const WEIGHT3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const WEIGHT4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

#[derive(Clone, Copy)]
struct Block {
    half: [[u32; 3]; 16],
    linear: [[u32; 3]; 16],
    shift: u32,
}

#[derive(Clone, Copy)]
struct Candidate {
    error: u64,
    words: [u32; 4],
}

impl Candidate {
    const WORST: Self = Self {
        error: u64::MAX,
        words: [u32::MAX; 4],
    };

    fn keep(&mut self, other: Self) {
        if (other.error, other.words) < (self.error, self.words) {
            *self = other;
        }
    }
}

fn linear(half: u32, shift: u32) -> u32 {
    let exponent = half >> 10;
    let mantissa = half & 1023;
    let left = exponent.saturating_sub(1);
    let value = u64::from(mantissa + if exponent > 0 { 1024 } else { 0 }) << left;
    (value >> shift) as u32
}

fn load(pixels: &[u32; 32]) -> Block {
    let half: [[u32; 3]; 16] = std::array::from_fn(|i| {
        [
            pixels[i * 2] & 65535,
            pixels[i * 2] >> 16,
            pixels[i * 2 + 1] & 65535,
        ]
    });
    let exponent = half.iter().flatten().copied().max().unwrap_or(0) >> 10;
    let shift = exponent.saturating_sub(5);
    let linear = half.map(|p| p.map(|h| linear(h, shift)));
    Block {
        half,
        linear,
        shift,
    }
}

fn unquantize(q: u32, bits: u32) -> u32 {
    if bits == 16 {
        q
    } else if q == 0 {
        0
    } else if q == (1 << bits) - 1 {
        65535
    } else {
        ((q << 16) + 32768) >> bits
    }
}

fn quantize(half: u32, bits: u32) -> u32 {
    let target = (half * 64 + 15) / 31;
    let maximum = (1 << bits) - 1;
    let center = (((u64::from(target) << bits) + 32768) >> 16).min(u64::from(maximum)) as u32;
    let mut best = (u32::MAX, 0);
    for q in center.saturating_sub(2)..=(center + 2).min(maximum) {
        let decoded = (unquantize(q, bits) * 31) >> 6;
        let key = (decoded.abs_diff(half), q);
        if key < best {
            best = key;
        }
    }
    best.1
}

fn subset(partition: u32, pixel: usize) -> usize {
    ((MASKS[partition as usize] >> pixel) & 1) as usize
}

fn weight(mode: usize, index: u32) -> u32 {
    if mode < 10 {
        WEIGHT3[index as usize]
    } else {
        WEIGHT4[index as usize]
    }
}

fn distance(block: &Block, pixel: usize, half: [u32; 3], light: [u32; 3]) -> u64 {
    let mut error = 0;
    for c in 0..3 {
        let a = u64::from(block.half[pixel][c].abs_diff(half[c]));
        let b = u64::from(block.linear[pixel][c].abs_diff(light[c]));
        error += a * a * 4 + b * b;
    }
    error
}

// Seed 0 is a covariance-oriented box; seed 1 uses three integer PCA iterations
// and the actual texels at its extreme projections. All ties retain the first.
fn seed(block: &Block, partition: u32, two: bool, kind: u32) -> [u32; 12] {
    let mut endpoints = [0; 12];
    for s in 0..if two { 2 } else { 1 } {
        let mut lo = [31743; 3];
        let mut hi = [0; 3];
        let mut mean = [0; 3];
        let mut n = 0;
        for i in 0..16 {
            if two && subset(partition, i) != s {
                continue;
            }
            n += 1;
            for c in 0..3 {
                lo[c] = lo[c].min(block.half[i][c]);
                hi[c] = hi[c].max(block.half[i][c]);
                mean[c] += block.half[i][c];
            }
        }
        for v in &mut mean {
            *v /= n;
        }
        let mut dominant = 0;
        for c in 1..3 {
            if hi[c] - lo[c] > hi[dominant] - lo[dominant] {
                dominant = c;
            }
        }
        let mut covariance = [[0i64; 3]; 3];
        for i in 0..16 {
            if two && subset(partition, i) != s {
                continue;
            }
            for c in 0..3 {
                for d in 0..3 {
                    covariance[c][d] += (i64::from(block.half[i][c]) - i64::from(mean[c]))
                        * (i64::from(block.half[i][d]) - i64::from(mean[d]));
                }
            }
        }
        if kind == 0 {
            for c in 0..3 {
                let reverse = covariance[dominant][c] < 0;
                endpoints[s * 6 + c] = if reverse { hi[c] } else { lo[c] };
                endpoints[s * 6 + 3 + c] = if reverse { lo[c] } else { hi[c] };
            }
        } else {
            let mut direction = [0i64; 3];
            direction[dominant] = 1024;
            for _ in 0..3 {
                let mut next = [0i64; 3];
                for c in 0..3 {
                    for d in 0..3 {
                        next[c] += covariance[c][d] * direction[d];
                    }
                }
                let scale = next.iter().map(|v| v.abs()).max().unwrap_or(0);
                if scale != 0 {
                    for c in 0..3 {
                        direction[c] = next[c] * 1024 / scale;
                    }
                }
            }
            let mut low = i64::MAX;
            let mut high = i64::MIN;
            let mut low_i = 0;
            let mut high_i = 0;
            for i in 0..16 {
                if two && subset(partition, i) != s {
                    continue;
                }
                let mut projection = 0;
                for c in 0..3 {
                    projection += (i64::from(block.half[i][c]) - i64::from(mean[c])) * direction[c];
                }
                if projection < low {
                    low = projection;
                    low_i = i;
                }
                if projection > high {
                    high = projection;
                    high_i = i;
                }
            }
            endpoints[s * 6..s * 6 + 3].copy_from_slice(&block.half[low_i]);
            endpoints[s * 6 + 3..s * 6 + 6].copy_from_slice(&block.half[high_i]);
        }
    }
    endpoints
}

fn orient(block: &Block, endpoints: &mut [u32; 12], partition: u32, two: bool) {
    for s in 0..if two { 2 } else { 1 } {
        let anchor = if s == 0 {
            0
        } else {
            ANCHORS[partition as usize] as usize
        };
        let a = std::array::from_fn(|c| endpoints[s * 6 + c]);
        let b = std::array::from_fn(|c| endpoints[s * 6 + 3 + c]);
        if distance(block, anchor, b, b.map(|h| linear(h, block.shift)))
            < distance(block, anchor, a, a.map(|h| linear(h, block.shift)))
        {
            for c in 0..3 {
                endpoints.swap(s * 6 + c, s * 6 + 3 + c);
            }
        }
    }
}

// Transformed endpoints use modular signed deltas. Clamping the shortest modular
// delta makes every searched mode legal, including at zero / maximum half values.
fn quantized(endpoints: &[u32; 12], mode: usize) -> [u32; 12] {
    let bits = PRECISION[mode];
    let mask = (1u32 << bits) - 1;
    let mut q = [0; 12];
    let count = if mode < 10 { 12 } else { 6 };
    for c in 0..count {
        q[c] = quantize(endpoints[c], bits);
    }
    if mode != 9 && mode != 10 {
        for e in 1..count / 3 {
            for c in 0..3 {
                let delta_bits = DELTA[mode][c];
                let limit = 1i32 << (delta_bits - 1);
                let wrapped = (q[e * 3 + c]
                    .wrapping_sub(q[c])
                    .wrapping_add(1 << (bits - 1)))
                    & mask;
                let delta = (wrapped as i32 - (1 << (bits - 1))).clamp(-limit, limit - 1);
                q[e * 3 + c] = ((q[c] as i32 + delta) as u32) & mask;
            }
        }
    }
    q
}

fn assign(block: &Block, q: &[u32; 12], mode: usize, partition: u32) -> (u64, [u32; 16]) {
    let mut indices = [0; 16];
    let mut error = 0;
    let count = if mode < 10 { 8 } else { 16 };
    let decoded = q.map(|v| unquantize(v, PRECISION[mode]));
    for s in 0..if mode < 10 { 2 } else { 1 } {
        let mut palette = [[0; 3]; 16];
        let mut light = [[0; 3]; 16];
        for j in 0..count {
            for c in 0..3 {
                let w = weight(mode, j as u32);
                palette[j][c] =
                    ((((64 - w) * decoded[s * 6 + c] + w * decoded[s * 6 + 3 + c] + 32) >> 6) * 31)
                        >> 6;
                light[j][c] = linear(palette[j][c], block.shift);
            }
        }
        for (i, index) in indices.iter_mut().enumerate() {
            if mode < 10 && subset(partition, i) != s {
                continue;
            }
            let anchor = i == 0 || (mode < 10 && i == ANCHORS[partition as usize] as usize);
            let mut best = (u64::MAX, 0);
            for j in 0..if anchor { count / 2 } else { count } {
                let key = (distance(block, i, palette[j], light[j]), j as u32);
                if key < best {
                    best = key;
                }
            }
            error += best.0;
            *index = best.1;
        }
    }
    (error, indices)
}

fn round_div(numerator: i64, denominator: i64) -> i64 {
    if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        -((-numerator + denominator / 2) / denominator)
    }
}

fn refine(
    block: &Block,
    endpoints: &mut [u32; 12],
    indices: &[u32; 16],
    mode: usize,
    partition: u32,
) {
    for s in 0..if mode < 10 { 2 } else { 1 } {
        let mut aa = 0i64;
        let mut ab = 0i64;
        let mut bb = 0i64;
        let mut ax = [0i64; 3];
        let mut bx = [0i64; 3];
        for (i, &index) in indices.iter().enumerate() {
            if mode < 10 && subset(partition, i) != s {
                continue;
            }
            let b = i64::from(weight(mode, index));
            let a = 64 - b;
            aa += a * a;
            ab += a * b;
            bb += b * b;
            for c in 0..3 {
                ax[c] += a * i64::from(block.half[i][c]) * 64;
                bx[c] += b * i64::from(block.half[i][c]) * 64;
            }
        }
        let determinant = aa * bb - ab * ab;
        if determinant != 0 {
            for c in 0..3 {
                endpoints[s * 6 + c] =
                    round_div(ax[c] * bb - bx[c] * ab, determinant).clamp(0, 31743) as u32;
                endpoints[s * 6 + 3 + c] =
                    round_div(bx[c] * aa - ax[c] * ab, determinant).clamp(0, 31743) as u32;
            }
        }
    }
}

fn put(words: &mut [u32; 4], offset: u32, value: u32, count: u32) {
    let word = (offset >> 5) as usize;
    let shift = offset & 31;
    words[word] |= value << shift;
    if shift + count > 32 {
        words[word + 1] |= value >> (32 - shift);
    }
}

fn pack(q: &[u32; 12], indices: &[u32; 16], mode: usize, partition: u32) -> [u32; 4] {
    let mut raw = *q;
    if mode != 9 && mode != 10 {
        for e in 1..if mode < 10 { 4 } else { 2 } {
            for c in 0..3 {
                raw[e * 3 + c] = q[e * 3 + c].wrapping_sub(q[c]) & ((1 << DELTA[mode][c]) - 1);
            }
        }
    }
    let mut words = [PREFIXES[mode], 0, 0, 0];
    let start = if mode < 2 { 2 } else { 5 };
    let count = if mode < 2 {
        75
    } else if mode < 10 {
        72
    } else {
        60
    };
    for i in 0..count {
        let source = LAYOUT[mode][i as usize];
        put(
            &mut words,
            start + i,
            (raw[(source >> 4) as usize] >> (source & 15)) & 1,
            1,
        );
    }
    let mut offset = 65;
    if mode < 10 {
        put(&mut words, 77, partition, 5);
        offset = 82;
    }
    for (i, &index) in indices.iter().enumerate() {
        let anchor = i == 0 || (mode < 10 && i == ANCHORS[partition as usize] as usize);
        let bits = if mode < 10 { 3 } else { 4 } - u32::from(anchor);
        put(&mut words, offset, index, bits);
        offset += bits;
    }
    debug_assert_eq!(offset, 128);
    words
}

fn search(
    block: &Block,
    initial: [u32; 12],
    mode: usize,
    partition: u32,
    rounds: u32,
) -> Candidate {
    let mut endpoints = initial;
    let mut best = Candidate::WORST;
    for round in 0..rounds {
        orient(block, &mut endpoints, partition, mode < 10);
        let q = quantized(&endpoints, mode);
        let (error, indices) = assign(block, &q, mode, partition);
        if error <= best.error {
            best.keep(Candidate {
                error,
                words: pack(&q, &indices, mode, partition),
            });
        }
        if round + 1 < rounds {
            refine(block, &mut endpoints, &indices, mode, partition);
        }
    }
    best
}

/// Encodes one BC6H UF16 block, exactly matching the integer GPU encoder.
///
/// Input is sixteen pairs `(Rhalf | Ghalf << 16, Bhalf)` with finite, nonnegative
/// half bits in `0..=0x7bff`. The upper half of each B word is ignored. Presets are
/// 0 = Fast, 1 = Balanced, 2 = HQ; larger values use HQ. Output is little endian.
pub fn encode_block(pixels: &[u32; 32], preset: u32) -> [u32; 4] {
    let block = load(pixels);
    let rounds = if preset == 0 {
        1
    } else if preset == 1 {
        3
    } else {
        5
    };
    let mut best = Candidate::WORST;
    let seeds = if preset >= 2 { 2 } else { 1 };
    for kind in 0..seeds {
        let initial = seed(&block, 0, false, kind);
        for mode in 10..if preset == 0 { 11 } else { 14 } {
            best.keep(search(&block, initial, mode, 0, rounds));
        }
    }
    if preset != 0 {
        let mut scores = [(0u64, 0u32); 32];
        let initial: [[u32; 12]; 32] = std::array::from_fn(|p| seed(&block, p as u32, true, 0));
        if preset == 1 {
            for p in 0..32 {
                scores[p] = (search(&block, initial[p], 9, p as u32, 1).error, p as u32);
            }
            scores.sort_unstable();
        }
        for p in 0..32u32 {
            if preset == 1 && !scores[..8].iter().any(|&(_, partition)| partition == p) {
                continue;
            }
            for kind in 0..seeds {
                let endpoints = if kind == 0 {
                    initial[p as usize]
                } else {
                    seed(&block, p, true, kind)
                };
                for mode in 0..10 {
                    if preset == 1 && ![0, 1, 5, 9].contains(&mode) {
                        continue;
                    }
                    best.keep(search(&block, endpoints, mode, p, rounds));
                }
            }
        }
    }
    best.words
}

// The bit layout / partition constants below retain bcdec's MIT notice.
// Their inverse mapping is shared with bc6h_tables.slangh; fitting is Ara code.

#[cfg(test)]
#[path = "bc6h_tests.rs"]
mod tests;
// BC6H layout/partition data derived from bcdec (https://github.com/iOrange/bcdec).
// bcdec revision 80859ed3b7afb1c527a2a99d70c61457bea72d0c (via bcdec_rs 0.2.1).
// Copyright (c) 2022 Sergii Kudlai
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of
// this software and associated documentation files (the "Software"), to deal in
// the Software without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
// of the Software, and to permit persons to whom the Software is furnished to do
// so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

const MASKS: [u32; 32] = [
    52428, 34952, 61166, 60616, 51328, 65260, 65224, 60544, 51200, 65516, 65152, 59392, 65512,
    65280, 65520, 61440, 63248, 142, 28928, 2254, 140, 29456, 12544, 36046, 2188, 12560, 26214,
    13932, 6120, 4080, 29070, 14748,
];
const ANCHORS: [u32; 32] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8,
    2, 2, 8, 8, 2, 2,
];
const PREFIXES: [u32; 14] = [0, 1, 2, 6, 10, 14, 18, 22, 26, 30, 3, 7, 11, 15];
const PRECISION: [u32; 14] = [10, 7, 11, 11, 11, 9, 8, 8, 8, 6, 10, 11, 12, 16];
const DELTA: [[u32; 3]; 14] = [
    [5, 5, 5],
    [6, 6, 6],
    [5, 4, 4],
    [4, 5, 4],
    [4, 4, 5],
    [5, 5, 5],
    [6, 5, 5],
    [5, 6, 5],
    [5, 5, 6],
    [6, 6, 6],
    [10, 10, 10],
    [9, 9, 9],
    [8, 8, 8],
    [4, 4, 4],
];
const LAYOUT: [[u32; 75]; 14] = [
    [
        116, 132, 180, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32,
        33, 34, 35, 36, 37, 38, 39, 40, 41, 48, 49, 50, 51, 52, 164, 112, 113, 114, 115, 64, 65,
        66, 67, 68, 176, 160, 161, 162, 163, 80, 81, 82, 83, 84, 177, 128, 129, 130, 131, 96, 97,
        98, 99, 100, 178, 144, 145, 146, 147, 148, 179,
    ],
    [
        117, 164, 165, 0, 1, 2, 3, 4, 5, 6, 176, 177, 132, 16, 17, 18, 19, 20, 21, 22, 133, 178,
        116, 32, 33, 34, 35, 36, 37, 38, 179, 181, 180, 48, 49, 50, 51, 52, 53, 112, 113, 114, 115,
        64, 65, 66, 67, 68, 69, 160, 161, 162, 163, 80, 81, 82, 83, 84, 85, 128, 129, 130, 131, 96,
        97, 98, 99, 100, 101, 144, 145, 146, 147, 148, 149,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32, 33, 34, 35, 36,
        37, 38, 39, 40, 41, 48, 49, 50, 51, 52, 10, 112, 113, 114, 115, 64, 65, 66, 67, 26, 176,
        160, 161, 162, 163, 80, 81, 82, 83, 42, 177, 128, 129, 130, 131, 96, 97, 98, 99, 100, 178,
        144, 145, 146, 147, 148, 179, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32, 33, 34, 35, 36,
        37, 38, 39, 40, 41, 48, 49, 50, 51, 10, 164, 112, 113, 114, 115, 64, 65, 66, 67, 68, 26,
        160, 161, 162, 163, 80, 81, 82, 83, 42, 177, 128, 129, 130, 131, 96, 97, 98, 99, 176, 178,
        144, 145, 146, 147, 116, 179, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32, 33, 34, 35, 36,
        37, 38, 39, 40, 41, 48, 49, 50, 51, 10, 132, 112, 113, 114, 115, 64, 65, 66, 67, 26, 176,
        160, 161, 162, 163, 80, 81, 82, 83, 84, 42, 128, 129, 130, 131, 96, 97, 98, 99, 177, 178,
        144, 145, 146, 147, 180, 179, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 132, 16, 17, 18, 19, 20, 21, 22, 23, 24, 116, 32, 33, 34, 35,
        36, 37, 38, 39, 40, 180, 48, 49, 50, 51, 52, 164, 112, 113, 114, 115, 64, 65, 66, 67, 68,
        176, 160, 161, 162, 163, 80, 81, 82, 83, 84, 177, 128, 129, 130, 131, 96, 97, 98, 99, 100,
        178, 144, 145, 146, 147, 148, 179, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 164, 132, 16, 17, 18, 19, 20, 21, 22, 23, 178, 116, 32, 33, 34, 35,
        36, 37, 38, 39, 179, 180, 48, 49, 50, 51, 52, 53, 112, 113, 114, 115, 64, 65, 66, 67, 68,
        176, 160, 161, 162, 163, 80, 81, 82, 83, 84, 177, 128, 129, 130, 131, 96, 97, 98, 99, 100,
        101, 144, 145, 146, 147, 148, 149, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 176, 132, 16, 17, 18, 19, 20, 21, 22, 23, 117, 116, 32, 33, 34, 35,
        36, 37, 38, 39, 165, 180, 48, 49, 50, 51, 52, 164, 112, 113, 114, 115, 64, 65, 66, 67, 68,
        69, 160, 161, 162, 163, 80, 81, 82, 83, 84, 177, 128, 129, 130, 131, 96, 97, 98, 99, 100,
        178, 144, 145, 146, 147, 148, 179, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 177, 132, 16, 17, 18, 19, 20, 21, 22, 23, 133, 116, 32, 33, 34, 35,
        36, 37, 38, 39, 181, 180, 48, 49, 50, 51, 52, 164, 112, 113, 114, 115, 64, 65, 66, 67, 68,
        176, 160, 161, 162, 163, 80, 81, 82, 83, 84, 85, 128, 129, 130, 131, 96, 97, 98, 99, 100,
        178, 144, 145, 146, 147, 148, 179, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 164, 176, 177, 132, 16, 17, 18, 19, 20, 21, 117, 133, 178, 116, 32, 33,
        34, 35, 36, 37, 165, 179, 181, 180, 48, 49, 50, 51, 52, 53, 112, 113, 114, 115, 64, 65, 66,
        67, 68, 69, 160, 161, 162, 163, 80, 81, 82, 83, 84, 85, 128, 129, 130, 131, 96, 97, 98, 99,
        100, 101, 144, 145, 146, 147, 148, 149, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32, 33, 34, 35, 36,
        37, 38, 39, 40, 41, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 64, 65, 66, 67, 68, 69, 70, 71,
        72, 73, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32, 33, 34, 35, 36,
        37, 38, 39, 40, 41, 48, 49, 50, 51, 52, 53, 54, 55, 56, 10, 64, 65, 66, 67, 68, 69, 70, 71,
        72, 26, 80, 81, 82, 83, 84, 85, 86, 87, 88, 42, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32, 33, 34, 35, 36,
        37, 38, 39, 40, 41, 48, 49, 50, 51, 52, 53, 54, 55, 11, 10, 64, 65, 66, 67, 68, 69, 70, 71,
        27, 26, 80, 81, 82, 83, 84, 85, 86, 87, 43, 42, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255,
    ],
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 32, 33, 34, 35, 36,
        37, 38, 39, 40, 41, 48, 49, 50, 51, 15, 14, 13, 12, 11, 10, 64, 65, 66, 67, 31, 30, 29, 28,
        27, 26, 80, 81, 82, 83, 47, 46, 45, 44, 43, 42, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255,
    ],
];
