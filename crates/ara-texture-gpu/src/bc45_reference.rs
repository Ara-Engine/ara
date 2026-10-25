//! Integer BC4/BC5 oracle for `shaders/bc45.slang`.
//!
//! Error is squared distance in byte units scaled by 35 (LCM of 5 and 7),
//! preserving the exact UNORM interpolation fractions in both endpoint orders.
//! The largest block error is 16 * (255 * 35)^2 = 1,274,490,000 < u32::MAX.
//! Endpoint least squares fits use signed 32-bit sums/products (below 2^29).
//! No code is ported from another encoder. Format reference:
//! https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression

#[derive(Clone, Copy)]
struct Candidate {
    error: u32,
    first: u32,
    second: u32,
}

impl Candidate {
    const EMPTY: Self = Self {
        error: u32::MAX,
        first: 255,
        second: 255,
    };

    fn better_than(self, other: Self) -> bool {
        (self.error, self.first, self.second) < (other.error, other.first, other.second)
    }
}

fn ordered(first: i32, second: i32, six: bool) -> (u32, u32) {
    let low = first.min(second).clamp(0, 255) as u32;
    let high = first.max(second).clamp(0, 255) as u32;
    if six {
        (low, high)
    } else if high == low {
        if high == 255 {
            (255, 254)
        } else {
            (high + 1, low)
        }
    } else {
        (high, low)
    }
}

fn palette(first: u32, second: u32) -> [i32; 8] {
    let a = first as i32;
    let b = second as i32;
    let mut result = [a * 35, b * 35, 0, 0, 0, 0, 0, 0];
    if a > b {
        for index in 2..8 {
            result[index as usize] = ((8 - index) * a + (index - 1) * b) * 5;
        }
    } else {
        for index in 2..6 {
            result[index as usize] = ((6 - index) * a + (index - 1) * b) * 7;
        }
        result[7] = 255 * 35;
    }
    result
}

fn nearest(value: u32, values: &[i32; 8]) -> (u32, u32) {
    let mut error = u32::MAX;
    let mut selected = 0;
    for (index, &decoded) in values.iter().enumerate() {
        let delta = value as i32 * 35 - decoded;
        let trial = (delta * delta) as u32;
        if trial < error {
            error = trial;
            selected = index as u32;
        }
    }
    (selected, error)
}

fn score(values: &[u32; 16], first: u32, second: u32) -> Candidate {
    let decoded = palette(first, second);
    Candidate {
        error: values.iter().map(|&v| nearest(v, &decoded).1).sum(),
        first,
        second,
    }
}

fn fitted(values: &[u32; 16], first: u32, second: u32) -> (Candidate, (u32, u32)) {
    let six = first <= second;
    let denominator = if six { 5 } else { 7 };
    let decoded = palette(first, second);
    let (mut aa, mut ab, mut bb, mut av, mut bv) = (0, 0, 0, 0, 0);
    let mut error = 0;
    for &value in values {
        let (index, residual) = nearest(value, &decoded);
        error += residual;
        if six && index >= 6 {
            continue;
        }
        let weight_b = match index {
            0 => 0,
            1 => denominator,
            _ => index as i32 - 1,
        };
        let weight_a = denominator - weight_b;
        aa += weight_a * weight_a;
        ab += weight_a * weight_b;
        bb += weight_b * weight_b;
        av += weight_a * denominator * value as i32;
        bv += weight_b * denominator * value as i32;
    }
    let current = Candidate {
        error,
        first,
        second,
    };
    let determinant = aa * bb - ab * ab;
    if determinant <= 0 {
        return (current, (first, second));
    }
    // Negative fitted endpoints clamp to zero; positive halves round upward.
    let a = (av * bb - bv * ab).max(0);
    let b = (bv * aa - av * ab).max(0);
    (
        current,
        ordered(
            (a + determinant / 2) / determinant,
            (b + determinant / 2) / determinant,
            six,
        ),
    )
}

fn seed(values: &[u32; 16], seed: u32, six: bool) -> (u32, u32) {
    let mut low = 255;
    let mut high = 0;
    let mut inner_low = 255;
    let mut inner_high = 0;
    for &v in values {
        low = low.min(v);
        high = high.max(v);
        if v > 0 && v < 255 {
            inner_low = inner_low.min(v);
            inner_high = inner_high.max(v);
        }
    }
    if seed == 1 && inner_low <= inner_high {
        return ordered(inner_low as i32, inner_high as i32, six);
    }
    let divisor = if six { 10 } else { 14 };
    let step = ((high - low + divisor / 2) / divisor).max(1) as i32;
    let (x, y) = match seed {
        0 | 1 => (0, 0),
        2 => (1, 0),
        3 => (0, -1),
        4 => (1, -1),
        5 => (-1, 1),
        6 => (-1, 0),
        7 => (0, 1),
        _ => {
            let mut cell = seed - 8;
            if cell >= 12 {
                cell += 1;
            }
            (cell as i32 % 5 - 2, cell as i32 / 5 - 2)
        }
    };
    ordered(low as i32 + x * step, high as i32 + y * step, six)
}

fn search(values: &[u32; 16], count: u32, iterations: u32, six: bool) -> Candidate {
    let mut best = Candidate::EMPTY;
    for index in 0..count {
        let (mut a, mut b) = seed(values, index, six);
        for _ in 0..iterations {
            let (candidate, next) = fitted(values, a, b);
            if candidate.better_than(best) {
                best = candidate;
            }
            (a, b) = next;
        }
        let candidate = score(values, a, b);
        if candidate.better_than(best) {
            best = candidate;
        }
    }
    best
}

fn neighborhood(values: &[u32; 16], best: Candidate, radius: i32, six: bool) -> Candidate {
    let mut result = best;
    for x in -radius..=radius {
        for y in -radius..=radius {
            let a = best.first as i32 + x;
            let b = best.second as i32 + y;
            if !(0..=255).contains(&a) || !(0..=255).contains(&b) || (a <= b) != six {
                continue;
            }
            let candidate = score(values, a as u32, b as u32);
            if candidate.better_than(result) {
                result = candidate;
            }
        }
    }
    result
}

fn encode_channel(values: &[u32; 16], preset: u32) -> [u32; 2] {
    let mut best = Candidate::EMPTY;
    for six in [false, true] {
        // Nested stages retain every cheaper preset's candidate and neighborhood.
        let mut mode = search(values, 2, 1, six);
        if preset >= 1 {
            let balanced = search(values, 8, 3, six);
            if balanced.better_than(mode) {
                mode = balanced;
            }
            mode = neighborhood(values, mode, 1, six);
        }
        if preset >= 2 {
            let thorough = search(values, 32, 6, six);
            if thorough.better_than(mode) {
                mode = thorough;
            }
            for _ in 0..2 {
                mode = neighborhood(values, mode, 3, six);
            }
        }
        if mode.better_than(best) {
            best = mode;
        }
    }
    let decoded = palette(best.first, best.second);
    let mut packed = u64::from(best.first | (best.second << 8));
    for (index, &value) in values.iter().enumerate() {
        packed |= u64::from(nearest(value, &decoded).0) << (16 + index * 3);
    }
    [packed as u32, (packed >> 32) as u32]
}

/// Encodes R from sixteen packed RGBA8 words as little-endian BC4 UNORM.
/// Presets are 0=Fast, 1=Balanced, and 2=HQ; larger values select HQ.
pub fn encode_bc4(pixels: &[u32; 16], preset: u32) -> [u32; 2] {
    encode_channel(&pixels.map(|word| word & 255), preset)
}

/// Encodes R then G as two independent BC4 blocks (BC5 UNORM).
/// Uses exactly the shader's integer algorithm, including endpoint/index ties.
pub fn encode_bc5(pixels: &[u32; 16], preset: u32) -> [u32; 4] {
    let red = encode_bc4(pixels, preset);
    let green = encode_channel(&pixels.map(|word| (word >> 8) & 255), preset);
    [red[0], red[1], green[0], green[1]]
}
