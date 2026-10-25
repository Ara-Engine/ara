//! Integer BC7 oracle for `shaders/bc7.slang`; arithmetic and search order are ABI.
//!
//! Each candidate fits subsets independently with integer PCA and alternating
//! nearest-palette/least-squares steps. Every candidate gets two fitting passes;
//! the best eight get deeper refinement. HQ also refines Balanced's best eight,
//! and both refine mode 6, so preset error cannot increase. This is a full mode/
//! partition search with bounded finalist refinement, not an exhaustive optimizer.
//! Errors include all four byte channels.
//! Ties prefer the lower candidate, seed, iteration, p-bit pattern and selector.
//! No color-space transform is performed: the caller supplies canonical texels.
//! The shader's bounded reciprocal and subtractive division are exact equivalents
//! of the ordinary integer divisions used here.
//! Uniform planes reuse one selector evaluation and the first seed's fit, while
//! preserving every p-bit re-quantization round and the original endpoint ties.

#[derive(Clone, Copy)]
struct Mode {
    subsets: usize,
    partition_bits: u32,
    color_bits: u32,
    alpha_bits: u32,
    index_bits: u32,
    pbits: u32, // 0: none, 1: shared per subset, 2: per endpoint.
}

const MODES: [Mode; 8] = [
    Mode {
        subsets: 3,
        partition_bits: 4,
        color_bits: 4,
        alpha_bits: 0,
        index_bits: 3,
        pbits: 2,
    },
    Mode {
        subsets: 2,
        partition_bits: 6,
        color_bits: 6,
        alpha_bits: 0,
        index_bits: 3,
        pbits: 1,
    },
    Mode {
        subsets: 3,
        partition_bits: 6,
        color_bits: 5,
        alpha_bits: 0,
        index_bits: 2,
        pbits: 0,
    },
    Mode {
        subsets: 2,
        partition_bits: 6,
        color_bits: 7,
        alpha_bits: 0,
        index_bits: 2,
        pbits: 2,
    },
    Mode {
        subsets: 1,
        partition_bits: 0,
        color_bits: 5,
        alpha_bits: 6,
        index_bits: 2,
        pbits: 0,
    },
    Mode {
        subsets: 1,
        partition_bits: 0,
        color_bits: 7,
        alpha_bits: 8,
        index_bits: 2,
        pbits: 0,
    },
    Mode {
        subsets: 1,
        partition_bits: 0,
        color_bits: 7,
        alpha_bits: 7,
        index_bits: 4,
        pbits: 2,
    },
    Mode {
        subsets: 2,
        partition_bits: 6,
        color_bits: 5,
        alpha_bits: 5,
        index_bits: 2,
        pbits: 2,
    },
];

const WEIGHTS: [[i32; 16]; 3] = [
    [0, 21, 43, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 9, 18, 27, 37, 46, 55, 64, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64],
];

#[derive(Clone, Copy)]
struct Fit {
    q: [[u32; 4]; 2],
    ep: [[i32; 4]; 2],
    p: [u32; 2],
    indices: [u32; 16],
    error: u32,
}

impl Fit {
    fn empty() -> Self {
        Self {
            q: [[0; 4]; 2],
            ep: [[0; 4]; 2],
            p: [0; 2],
            indices: [0; 16],
            error: u32::MAX,
        }
    }
}

fn expand(q: u32, bits: u32) -> i32 {
    let v = q << (8 - bits);
    (v | (v >> bits)) as i32
}

fn quantize(value: i32, bits: u32, has_p: bool, p: u32) -> (u32, i32) {
    let precision = bits + u32::from(has_p);
    let raw = ((value as u32 * ((1 << precision) - 1) + 127) / 255) >> u32::from(has_p);
    let mut best = (0, 0);
    let mut error = i32::MAX;
    // Checking neighbours handles endpoint replication and constrained p-bit ties.
    for q in raw.saturating_sub(1)..=(raw + 1).min((1 << bits) - 1) {
        let decoded = expand((q << u32::from(has_p)) | p, precision);
        let delta = value - decoded;
        if delta * delta < error {
            error = delta * delta;
            best = (q, decoded);
        }
    }
    best
}

fn seed(
    pixels: &[[i32; 4]; 16],
    mask: u32,
    first: usize,
    channels: usize,
    bbox: bool,
) -> ([[i32; 4]; 2], bool) {
    let mut lo = [255; 4];
    let mut hi = [0; 4];
    let mut sum = [0; 4];
    let count = mask.count_ones() as i32;
    for (i, pixel) in pixels.iter().enumerate() {
        if mask & (1 << i) != 0 {
            for c in first..first + channels {
                lo[c] = lo[c].min(pixel[c]);
                hi[c] = hi[c].max(pixel[c]);
                sum[c] += pixel[c];
            }
        }
    }
    let uniform = (first..first + channels).all(|c| lo[c] == hi[c]);
    if bbox || uniform {
        return ([lo, hi], uniform);
    }
    let mut axis = [0_i32; 4];
    let mut major = first;
    for c in first..first + channels {
        if hi[c] - lo[c] > hi[major] - lo[major] {
            major = c;
        }
    }
    axis[major] = 64;
    let mut cov = [[0_i32; 4]; 4];
    for (i, pixel) in pixels.iter().enumerate() {
        if mask & (1 << i) != 0 {
            for c in first..first + channels {
                for d in first..first + channels {
                    cov[c][d] += (pixel[c] - sum[c] / count) * (pixel[d] - sum[d] / count);
                }
            }
        }
    }
    // Power-of-two normalization keeps |axis| <=127 without division. The
    // covariance entries are <=260116, so their four-term dot fits int32.
    for _ in 0..4 {
        let mut next = [0_i32; 4];
        let mut scale = 0;
        for c in first..first + channels {
            for d in first..first + channels {
                next[c] += cov[c][d] * axis[d];
            }
            scale = scale.max(next[c].abs());
        }
        if scale != 0 {
            let shift = (32 - (scale as u32).leading_zeros()).saturating_sub(7);
            for c in first..first + channels {
                let magnitude = (next[c].unsigned_abs() >> shift) as i32;
                axis[c] = if next[c] < 0 { -magnitude } else { magnitude };
            }
        }
    }
    let mut endpoints = [[0; 4]; 2];
    let mut minimum = i32::MAX;
    let mut maximum = i32::MIN;
    for (i, pixel) in pixels.iter().enumerate() {
        if mask & (1 << i) != 0 {
            let mut projection = 0;
            for c in first..first + channels {
                projection += pixel[c] * axis[c];
            }
            if projection < minimum {
                minimum = projection;
                endpoints[0] = *pixel;
            }
            if projection > maximum {
                maximum = projection;
                endpoints[1] = *pixel;
            }
        }
    }
    (endpoints, false)
}

fn assign(
    pixels: &[[i32; 4]; 16],
    mask: u32,
    first: usize,
    channels: usize,
    index_bits: u32,
    uniform: bool,
    fit: &mut Fit,
) {
    let weights = &WEIGHTS[(index_bits - 2) as usize];
    let mut palette = [[0_i32; 4]; 16];
    for j in 0..1 << index_bits {
        let w = weights[j];
        for (c, value) in palette[j].iter_mut().enumerate().skip(first).take(channels) {
            *value = ((64 - w) * fit.ep[0][c] + w * fit.ep[1][c] + 32) >> 6;
        }
    }
    if uniform {
        // Inactive channels may vary; only the channels fitted by this plane matter.
        let pixel = pixels[mask.trailing_zeros() as usize];
        let mut error = u32::MAX;
        let mut index = 0;
        for (j, color) in palette.iter().enumerate().take(1 << index_bits) {
            let mut e = 0;
            for c in first..first + channels {
                let d = pixel[c] - color[c];
                e += (d * d) as u32;
            }
            if e < error {
                error = e;
                index = j as u32;
            }
        }
        fit.error = error * mask.count_ones();
        fit.indices = [index; 16];
        return;
    }
    fit.error = 0;
    for (i, pixel) in pixels.iter().enumerate() {
        if mask & (1 << i) != 0 {
            let mut error = u32::MAX;
            for (j, color) in palette.iter().enumerate().take(1 << index_bits) {
                let mut e = 0;
                for c in first..first + channels {
                    let d = pixel[c] - color[c];
                    e += (d * d) as u32;
                }
                if e < error {
                    error = e;
                    fit.indices[i] = j as u32;
                }
            }
            fit.error += error;
        }
    }
}

fn refit(
    pixels: &[[i32; 4]; 16],
    mask: u32,
    first: usize,
    channels: usize,
    index_bits: u32,
    fit: &Fit,
) -> [[i32; 4]; 2] {
    let mut aa = 0_i64;
    let mut ab = 0_i64;
    let mut bb = 0_i64;
    let mut ax = [0_i64; 4];
    let mut bx = [0_i64; 4];
    for (i, pixel) in pixels.iter().enumerate() {
        if mask & (1 << i) != 0 {
            let b = i64::from(WEIGHTS[(index_bits - 2) as usize][fit.indices[i] as usize]);
            let a = 64 - b;
            aa += a * a;
            ab += a * b;
            bb += b * b;
            for c in first..first + channels {
                ax[c] += a * i64::from(pixel[c]);
                bx[c] += b * i64::from(pixel[c]);
            }
        }
    }
    let det = aa * bb - ab * ab;
    let mut endpoints = fit.ep;
    if det > 0 {
        for c in first..first + channels {
            endpoints[0][c] =
                ((64 * (ax[c] * bb - bx[c] * ab) + det / 2) / det).clamp(0, 255) as i32;
            endpoints[1][c] =
                ((64 * (bx[c] * aa - ax[c] * ab) + det / 2) / det).clamp(0, 255) as i32;
        }
    }
    endpoints
}

#[allow(clippy::too_many_arguments)]
fn fit_subset(
    pixels: &[[i32; 4]; 16],
    mask: u32,
    first: usize,
    channels: usize,
    bits: u32,
    index_bits: u32,
    pbits: u32,
    preset: u32,
) -> Fit {
    let rounds = if preset == 3 {
        2
    } else if preset >= 2 {
        4
    } else {
        3
    };
    let seeds = if preset == 0 || preset == 3 { 1 } else { 2 };
    let mut best = Fit::empty();
    for s in 0..seeds {
        let (mut endpoints, uniform) = seed(pixels, mask, first, channels, s != 0);
        for _ in 0..rounds {
            let mut iteration = Fit::empty();
            for pattern in 0..1 << pbits {
                let mut trial = Fit::empty();
                trial.p = if pbits == 1 {
                    [pattern, pattern]
                } else {
                    [pattern & 1, (pattern >> 1) & 1]
                };
                for (e, endpoint) in endpoints.iter().enumerate() {
                    for (c, &value) in endpoint.iter().enumerate().skip(first).take(channels) {
                        (trial.q[e][c], trial.ep[e][c]) =
                            quantize(value, bits, pbits != 0, trial.p[e]);
                    }
                }
                assign(
                    pixels, mask, first, channels, index_bits, uniform, &mut trial,
                );
                if trial.error < iteration.error {
                    iteration = trial;
                }
                if iteration.error == 0 {
                    return iteration;
                }
            }
            if iteration.error < best.error {
                best = iteration;
            }
            let next = refit(pixels, mask, first, channels, index_bits, &iteration);
            let converged =
                (0..2).all(|e| (first..first + channels).all(|c| next[e][c] == endpoints[e][c]));
            endpoints = next;
            if converged {
                break;
            }
        }
        // Re-quantization can improve p-bit choices even when the determinant
        // is zero. Only the second seed is redundant for a uniform plane.
        if uniform {
            return best;
        }
    }
    best
}

struct Bits {
    words: [u32; 4],
    cursor: u32,
}
impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        if count == 0 {
            return;
        }
        let word = (self.cursor / 32) as usize;
        let shift = self.cursor % 32;
        self.words[word] |= value << shift;
        if shift + count > 32 {
            self.words[word + 1] |= value >> (32 - shift);
        }
        self.cursor += count;
    }
}

fn subset(mode: usize, partition: usize, texel: usize) -> usize {
    match MODES[mode].subsets {
        2 => ((PARTITION2[partition] >> (texel * 2)) & 3) as usize,
        3 => ((PARTITION3[partition] >> (texel * 2)) & 3) as usize,
        _ => 0,
    }
}

fn anchor(mode: usize, partition: usize, s: usize) -> usize {
    if s == 0 {
        0
    } else if MODES[mode].subsets == 2 {
        ANCHOR2[partition] as usize
    } else if s == 1 {
        ANCHOR31[partition] as usize
    } else {
        ANCHOR32[partition] as usize
    }
}

fn candidate(
    pixels: &[u32; 16],
    mode: usize,
    partition: usize,
    rotation: usize,
    selection: u32,
    preset: u32,
) -> (u32, [u32; 4]) {
    let m = MODES[mode];
    let dual = mode == 4 || mode == 5;
    let color_index = m.index_bits + selection;
    let alpha_index = if mode == 4 { 3 - selection } else { 2 };
    let mut colors = [[0_i32; 4]; 16];
    for (i, &pixel) in pixels.iter().enumerate() {
        for (c, channel) in colors[i].iter_mut().enumerate() {
            *channel = ((pixel >> (c * 8)) & 255) as i32;
        }
        if rotation != 0 {
            colors[i].swap(3, rotation - 1);
        }
    }
    let mut fits = [Fit::empty(); 3];
    let mut scalar = Fit::empty();
    let mut error = 0;
    for (s, fit) in fits.iter_mut().enumerate().take(m.subsets) {
        let mut mask = 0;
        for i in 0..16 {
            if subset(mode, partition, i) == s {
                mask |= 1 << i;
            }
        }
        *fit = fit_subset(
            &colors,
            mask,
            0,
            if m.alpha_bits != 0 && !dual { 4 } else { 3 },
            m.color_bits,
            color_index,
            m.pbits,
            preset,
        );
        error += fit.error;
        if fit.indices[anchor(mode, partition, s)] & (1 << (color_index - 1)) != 0 {
            fit.q.swap(0, 1);
            fit.p.swap(0, 1);
            for i in 0..16 {
                fit.indices[i] ^= (1 << color_index) - 1;
            }
        }
    }
    if dual {
        scalar = fit_subset(&colors, 65535, 3, 1, m.alpha_bits, alpha_index, 0, preset);
        error += scalar.error;
        if scalar.indices[0] & (1 << (alpha_index - 1)) != 0 {
            scalar.q.swap(0, 1);
            for i in 0..16 {
                scalar.indices[i] ^= (1 << alpha_index) - 1;
            }
        }
        for e in 0..2 {
            fits[0].q[e][3] = scalar.q[e][3];
        }
    } else if m.alpha_bits == 0 {
        for color in colors {
            let d = 255 - color[3];
            error += (d * d) as u32;
        }
    }
    let mut bits = Bits {
        words: [0; 4],
        cursor: 0,
    };
    bits.put(1 << mode, mode as u32 + 1);
    bits.put(partition as u32, m.partition_bits);
    if dual {
        bits.put(rotation as u32, 2);
    }
    if mode == 4 {
        bits.put(selection, 1);
    }
    for c in 0..4 {
        let precision = if c == 3 { m.alpha_bits } else { m.color_bits };
        if precision != 0 {
            for fit in fits.iter().take(m.subsets) {
                for e in 0..2 {
                    bits.put(fit.q[e][c], precision);
                }
            }
        }
    }
    for fit in fits.iter().take(m.subsets) {
        if m.pbits != 0 {
            bits.put(fit.p[0], 1);
            if m.pbits == 2 {
                bits.put(fit.p[1], 1);
            }
        }
    }
    for stream in 0..if dual { 2 } else { 1 } {
        let use_scalar = dual && stream == 1 - selection;
        let precision = if use_scalar { alpha_index } else { color_index };
        for i in 0..16 {
            let s = subset(mode, partition, i);
            let index = if use_scalar {
                scalar.indices[i]
            } else {
                fits[s].indices[i]
            };
            bits.put(
                index,
                precision - u32::from(i == anchor(mode, partition, s)),
            );
        }
    }
    debug_assert_eq!(bits.cursor, 128);
    (error, bits.words)
}

fn candidate_count(preset: u32) -> u32 {
    if preset == 0 {
        1
    } else if preset == 1 {
        85
    } else {
        285
    }
}

// Mode 6 first, then all dual-plane variants, then partitioned modes in mode order.
fn candidate_parameters(mut id: u32, preset: u32) -> (usize, usize, usize, u32) {
    if id == 0 {
        return (6, 0, 0, 0);
    }
    id -= 1;
    if id < 8 {
        return (4, 0, (id / 2) as usize, id & 1);
    }
    id -= 8;
    if id < 4 {
        return (5, 0, id as usize, 0);
    }
    id -= 4;
    for mode in [0, 1, 2, 3, 7] {
        let count = if mode == 0 {
            if preset == 1 { 8 } else { 16 }
        } else if preset == 1 {
            16
        } else {
            64
        };
        if id < count {
            return (mode, id as usize, 0, 0);
        }
        id -= count;
    }
    unreachable!("candidate index is bounded by candidate_count")
}

/// Encodes sixteen packed `R | G<<8 | B<<16 | A<<24` texels as little-endian BC7.
/// Presets are 0 (Fast), 1 (Balanced), and 2 (HQ); larger values use HQ.
/// The GPU uses identical integer arithmetic, candidate numbering and tie breaks.
pub fn encode_block(pixels: &[u32; 16], preset: u32) -> [u32; 4] {
    let preset = preset.min(2);
    if preset == 0 {
        return candidate(pixels, 6, 0, 0, 0, 0).1;
    }
    let mut ranking = Vec::with_capacity(candidate_count(preset) as usize);
    for id in 0..candidate_count(preset) {
        let (mode, partition, rotation, selection) = candidate_parameters(id, preset);
        let trial = candidate(pixels, mode, partition, rotation, selection, 3);
        ranking.push((trial.0, id));
    }
    ranking.sort_unstable();
    let mut finalists: Vec<u32> = ranking.iter().take(8).map(|&(_, id)| id).collect();
    if preset == 2 {
        finalists.extend(
            ranking
                .iter()
                .filter(|&&(_, id)| {
                    let (mode, partition, _, _) = candidate_parameters(id, 2);
                    mode == 4
                        || mode == 5
                        || mode == 6
                        || partition < if mode == 0 { 8 } else { 16 }
                })
                .take(8)
                .map(|&(_, id)| id),
        );
    }
    finalists.push(0); // Mode 6 includes the full Fast search, even if its coarse fit loses.
    let mut best = (u32::MAX, u32::MAX, [0; 4]);
    for id in finalists {
        let (mode, partition, rotation, selection) = candidate_parameters(id, preset);
        let trial = candidate(pixels, mode, partition, rotation, selection, preset);
        if (trial.0, id) < (best.0, best.1) {
            best = (trial.0, id, trial.1);
        }
    }
    best.2
}

#[cfg(test)]
#[path = "bc7_tests.rs"]
mod tests;

// File: bc7enc.c - Richard Geldreich, Jr. 3/31/2020 - MIT license or public domain (see below)
// Only normative BC7 partition/anchor tables are derived from that source.
// https://github.com/richgel999/bc7enc_rdo_devel/blob/d6425dc0a5557bab67e2d845e9e462ef39bc4c7b/bc7enc.cpp
// Public-domain alternative from the original license:
// This is free and unencumbered software released into the public domain.
// Anyone is free to copy, modify, publish, use, compile, sell, or distribute this
// software, either in source code form or as a compiled binary, for any purpose,
// commercial or non-commercial, and by any means.
// In jurisdictions that recognize copyright laws, the author or authors of this
// software dedicate any and all copyright interest in the software to the public
// domain. We make this dedication for the benefit of the public at large and to
// the detriment of our heirs and successors. We intend this dedication to be an
// overt act of relinquishment in perpetuity of all present and future rights to
// this software under copyright law.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN
// ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
// WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
// For more information, please refer to <http://unlicense.org/>

const PARTITION2: [u32; 64] = [
    0x50505050, 0x40404040, 0x54545454, 0x54505040, 0x50404000, 0x55545450, 0x55545040, 0x54504000,
    0x50400000, 0x55555450, 0x55544000, 0x54400000, 0x55555440, 0x55550000, 0x55555500, 0x55000000,
    0x55150100, 0x00004054, 0x15010000, 0x00405054, 0x00004050, 0x15050100, 0x05010000, 0x40505054,
    0x00404050, 0x05010100, 0x14141414, 0x05141450, 0x01155440, 0x00555500, 0x15014054, 0x05414150,
    0x44444444, 0x55005500, 0x11441144, 0x05055050, 0x05500550, 0x11114444, 0x41144114, 0x44111144,
    0x15055054, 0x01055040, 0x05041050, 0x05455150, 0x14414114, 0x50050550, 0x41411414, 0x00141400,
    0x00041504, 0x00105410, 0x10541000, 0x04150400, 0x50410514, 0x41051450, 0x05415014, 0x14054150,
    0x41050514, 0x41505014, 0x40011554, 0x54150140, 0x50505500, 0x00555050, 0x15151010, 0x54540404,
];
const PARTITION3: [u32; 64] = [
    0xaa685050, 0x6a5a5040, 0x5a5a4200, 0x5450a0a8, 0xa5a50000, 0xa0a05050, 0x5555a0a0, 0x5a5a5050,
    0xaa550000, 0xaa555500, 0xaaaa5500, 0x90909090, 0x94949494, 0xa4a4a4a4, 0xa9a59450, 0x2a0a4250,
    0xa5945040, 0x0a425054, 0xa5a5a500, 0x55a0a0a0, 0xa8a85454, 0x6a6a4040, 0xa4a45000, 0x1a1a0500,
    0x0050a4a4, 0xaaa59090, 0x14696914, 0x69691400, 0xa08585a0, 0xaa821414, 0x50a4a450, 0x6a5a0200,
    0xa9a58000, 0x5090a0a8, 0xa8a09050, 0x24242424, 0x00aa5500, 0x24924924, 0x24499224, 0x50a50a50,
    0x500aa550, 0xaaaa4444, 0x66660000, 0xa5a0a5a0, 0x50a050a0, 0x69286928, 0x44aaaa44, 0x66666600,
    0xaa444444, 0x54a854a8, 0x95809580, 0x96969600, 0xa85454a8, 0x80959580, 0xaa141414, 0x96960000,
    0xaaaa1414, 0xa05050a0, 0xa0a5a5a0, 0x96000000, 0x40804080, 0xa9a8a9a8, 0xaaaaaa44, 0x2a4a5254,
];
const ANCHOR2: [u32; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8,
    2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2,
    2, 15, 15, 15, 15, 15, 2, 2, 15,
];
const ANCHOR31: [u32; 64] = [
    3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3, 3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5,
    15, 15, 8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15, 3, 15, 5, 5, 5, 8, 5, 10, 5,
    10, 8, 13, 15, 12, 3, 3,
];
const ANCHOR32: [u32; 64] = [
    15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8, 15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6,
    10, 15, 15, 10, 8, 15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8, 15, 3, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
];
