use crate::{Error, data::Interpolation, ensure};

/// Independently range-reduced keys with off-thread prepared decode metadata.
#[derive(Debug)]
pub(crate) struct Segment {
    pub min: [f32; 4],
    pub span: [f32; 4],
    pub bits: [u8; 4],
    pub bytes: Vec<u8>,
    stride: usize,
    offsets: [usize; 4],
    scales: [f32; 4],
}
impl Segment {
    pub(crate) fn from_parts(bytes: Vec<u8>) -> Self {
        Self {
            min: [0.0; 4],
            span: [0.0; 4],
            bits: [0; 4],
            bytes,
            stride: 0,
            offsets: [0; 4],
            scales: [0.0; 4],
        }
    }
    /// Chooses component rates from local ranges and writes the packed integers.
    fn cook(keys: &[(u32, [f32; 4])], dimensions: u8, budget: f32, omitted: u8) -> Self {
        let mut result = Self::from_parts(Vec::new());
        result.min = keys[0].1;
        let mut max = result.min;
        for (_, key) in keys {
            for c in 0..usize::from(dimensions) {
                result.min[c] = result.min[c].min(key[c]);
                max[c] = max[c].max(key[c]);
            }
        }
        let radius = (0..usize::from(dimensions))
            .filter(|c| *c != usize::from(omitted))
            .map(|c| result.min[c].abs().max(max[c].abs()).powi(2))
            .sum::<f32>();
        let budget = if omitted < 4 {
            (budget - 8.0 * f32::EPSILON) * (1.0 - radius - 0.01).sqrt()
        } else {
            budget
        };
        let varying = (0..usize::from(dimensions))
            .filter(|c| *c != usize::from(omitted) && max[*c] != result.min[*c])
            .count()
            .max(1) as f32;
        for (c, max) in max.iter().enumerate().take(usize::from(dimensions)) {
            if c == usize::from(omitted) {
                result.min[c] = 0.0;
                continue;
            }
            result.span[c] = max - result.min[c];
            if result.span[c] == 0.0 {
                continue;
            }
            result.bits[c] = (1..=23)
                .find(|b| {
                    let e = result.span[c] / (2.0 * ((1_u32 << b) - 1) as f32)
                        + (result.min[c].abs() + result.span[c].abs()) * f32::EPSILON * 4.0;
                    e * varying.sqrt() <= budget
                })
                .unwrap_or(32);
            if result.bits[c] == 32 {
                result.span[c] = 0.0;
                result.min[c] = 0.0;
            }
        }
        let stride = result.bits.iter().map(|b| usize::from(*b)).sum::<usize>();
        result.bytes.resize((stride * keys.len()).div_ceil(8), 0);
        for (i, (_, value)) in keys.iter().enumerate() {
            let mut offset = i * stride;
            for (c, value) in value.iter().enumerate().take(usize::from(dimensions)) {
                let count = result.bits[c];
                let quantized = match count {
                    32 => value.to_bits(),
                    0 => 0,
                    _ => (((value - result.min[c]) / result.span[c]).clamp(0.0, 1.0)
                        * ((1_u32 << count) - 1) as f32)
                        .round() as u32,
                };
                for bit in 0..usize::from(count) {
                    result.bytes[(offset + bit) / 8] |=
                        (((quantized >> bit) & 1) as u8) << ((offset + bit) % 8);
                }
                offset += usize::from(count);
            }
        }
        result.prepare();
        result
    }
    /// Prepares fixed offsets/scales and an eight-byte safe read window.
    pub(crate) fn prepare(&mut self) {
        self.stride = 0;
        for c in 0..4 {
            self.offsets[c] = self.stride;
            self.stride += usize::from(self.bits[c]);
            self.scales[c] = if (1..=23).contains(&self.bits[c]) {
                self.span[c] / ((1_u32 << self.bits[c]) - 1) as f32
            } else {
                0.0
            };
        }
        // Padded windows avoid a memcpy/length branch per decoded component.
        self.bytes.resize(self.bytes.len() + 8, 0);
    }
    #[inline]
    /// Reads an exact integer for packet repacking or scalar root/curve sampling.
    pub(crate) fn raw(&self, index: usize, c: usize) -> u32 {
        let count = self.bits[c];
        if count == 0 {
            return 0;
        }
        let offset = index * self.stride + self.offsets[c];
        let start = offset / 8;
        let window = u64::from_le_bytes(
            self.bytes[start..start + 8]
                .try_into()
                .expect("validated padded key window"),
        );
        ((window >> (offset % 8)) & ((1_u64 << count) - 1)) as u32
    }
    #[inline]
    fn decode(&self, index: usize) -> [f32; 4] {
        std::array::from_fn(|c| {
            let count = self.bits[c];
            if count == 0 {
                return self.min[c];
            }
            let raw = self.raw(index, c);
            if count == 32 {
                f32::from_bits(raw)
            } else {
                self.min[c] + raw as f32 * self.scales[c]
            }
        })
    }
    fn bound(&self) -> f32 {
        self.bits
            .iter()
            .enumerate()
            .map(|(c, b)| {
                if *b == 0 || *b == 32 {
                    0.0
                } else {
                    (self.span[c] / (2.0 * ((1_u32 << b) - 1) as f32)
                        + (self.min[c].abs() + self.span[c].abs()) * f32::EPSILON * 4.0)
                        .powi(2)
                }
            })
            .sum::<f32>()
            .sqrt()
    }
}

/// Track-local rates and optional segment ranges; 4 means no omitted component.
#[derive(Debug)]
pub(crate) struct Packed {
    pub times: Vec<u32>,
    pub dimensions: u8,
    pub interpolation: Interpolation,
    pub omitted: u8,
    pub segment_len: usize,
    pub segments: Vec<Segment>,
}
impl Packed {
    /// Chooses the smallest whole-track or 8/16/32-key segmented representation.
    pub fn cook(
        mut keys: Vec<(u32, [f32; 4])>,
        dimensions: u8,
        interpolation: Interpolation,
        budget: f32,
        rotation: bool,
    ) -> Self {
        let omitted = if rotation && budget > 8.0 * f32::EPSILON {
            (0..4).find(|c| {
                keys.iter().all(|k| k.1[*c].abs() >= 0.5)
                    && (0..4)
                        .filter(|axis| axis != c)
                        .map(|axis| {
                            keys.iter()
                                .map(|k| k.1[axis].abs())
                                .fold(0.0, f32::max)
                                .powi(2)
                        })
                        .sum::<f32>()
                        <= 0.75
            })
        } else {
            None
        };
        // Fixed dominant components preserve hemisphere coherence across keys.
        if let Some(c) = omitted {
            for (_, key) in &mut keys {
                if key[c] < 0.0 {
                    for v in key {
                        *v = -*v;
                    }
                }
            }
        }
        let omitted = omitted.map_or(4, |c| c as u8);
        if keys.iter().all(|k| k.1 == keys[0].1) {
            keys.truncate(1);
        }
        let mut best = Self {
            times: keys.iter().map(|k| k.0).collect(),
            dimensions,
            interpolation,
            omitted,
            segment_len: keys.len(),
            segments: vec![Segment::cook(&keys, dimensions, budget, omitted)],
        };
        let size = |track: &Self| {
            postcard::to_allocvec(track)
                .expect("packed values serialize")
                .len()
        };
        let mut best_size = size(&best);
        for length in [32, 16, 8] {
            if keys.len() <= length {
                continue;
            }
            let candidate = Self {
                times: keys.iter().map(|k| k.0).collect(),
                dimensions,
                interpolation,
                omitted,
                segment_len: length,
                segments: keys
                    .chunks(length)
                    .map(|k| Segment::cook(k, dimensions, budget, omitted))
                    .collect(),
            };
            let candidate_size = size(&candidate);
            if candidate_size < best_size {
                best = candidate;
                best_size = candidate_size;
            }
        }
        if best.omitted < 4 && best.segments.iter().any(|s| s.bits.contains(&32)) {
            return Self::cook(keys, dimensions, interpolation, budget, false);
        }
        best
    }
    #[inline]
    /// Decodes one local key, reconstructing an omitted dominant component.
    pub fn decode(&self, index: usize) -> [f32; 4] {
        let mut out = self.segments[index / self.segment_len].decode(index % self.segment_len);
        if self.omitted < 4 {
            out[usize::from(self.omitted)] = (1.0 - out.iter().map(|v| v * v).sum::<f32>())
                .max(0.0)
                .sqrt();
        }
        out
    }
    /// Bounds endpoint Euclidean error, including dominant-component recovery.
    pub fn bound(&self) -> f32 {
        self.segments
            .iter()
            .map(|s| {
                let e = s.bound();
                // Gradient norm of dominant reconstruction is 1/sqrt(1-r^2).
                // The extra radius margin covers dequantization rounding.
                let radius = (0..4)
                    .filter(|c| *c != usize::from(self.omitted))
                    .map(|c| s.min[c].abs().max((s.min[c] + s.span[c]).abs()).powi(2))
                    .sum::<f32>();
                if self.omitted < 4 {
                    e / (1.0 - radius - 0.01).sqrt() + 8.0 * f32::EPSILON
                } else {
                    e
                }
            })
            .fold(0.0, f32::max)
    }
    /// Checks ranges, timestamps, rates and padded lengths before packet creation.
    pub fn validate(&self, duration: u32, dimensions: u8) -> Result<(), Error> {
        ensure(
            self.dimensions == dimensions
                && !self.times.is_empty()
                && self.segment_len > 0
                && self.segments.len() == self.times.len().div_ceil(self.segment_len)
                && (self.omitted == 4 || (dimensions == 4 && self.omitted < 4)),
            "invalid packed track dimensions",
        )?;
        ensure(
            self.times.iter().all(|t| *t <= duration) && self.times.windows(2).all(|w| w[0] < w[1]),
            "invalid packed key times",
        )?;
        for (i, s) in self.segments.iter().enumerate() {
            ensure(
                s.min.iter().all(|f| f.is_finite())
                    && s.span.iter().all(|f| f.is_finite() && *f >= 0.0),
                "invalid packed ranges",
            )?;
            ensure(
                s.bits.iter().all(|b| *b <= 23 || *b == 32)
                    && s.bits[usize::from(dimensions)..].iter().all(|b| *b == 0)
                    && (self.omitted == 4
                        || (s.bits[usize::from(self.omitted)] == 0
                            && s.min[usize::from(self.omitted)] == 0.0)),
                "invalid packed bit rates",
            )?;
            let count = self
                .segment_len
                .min(self.times.len() - i * self.segment_len);
            ensure(
                s.stride
                    .checked_mul(count)
                    .is_some_and(|bits| bits.div_ceil(8) + 8 == s.bytes.len()),
                "invalid packed track byte count",
            )?;
            if self.omitted < 4 {
                let radius = (0..4)
                    .filter(|c| *c != usize::from(self.omitted))
                    .map(|c| s.min[c].abs().max((s.min[c] + s.span[c]).abs()).powi(2))
                    .sum::<f32>();
                ensure(
                    radius <= 0.76 && s.bits.iter().all(|b| *b != 32),
                    "invalid dominant quaternion range",
                )?;
            }
        }
        for i in 0..self.times.len() {
            ensure(
                self.decode(i).iter().all(|v| v.is_finite()),
                "non-finite packed key",
            )?;
        }
        Ok(())
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.times.capacity() * std::mem::size_of::<u32>()
            + self.segments.capacity() * std::mem::size_of::<Segment>()
            + self
                .segments
                .iter()
                .map(|s| s.bytes.capacity())
                .sum::<usize>()
    }
}
