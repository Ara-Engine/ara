use crate::{Error, data::Interpolation, ensure};
use serde::{Deserialize, Serialize};

/// Packed per-component ranges. Zero bits means constant; 32 means raw float.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Packed {
    pub times: Vec<u32>,
    pub min: [f32; 4],
    pub span: [f32; 4],
    pub bits: [u8; 4],
    pub dimensions: u8,
    pub interpolation: Interpolation,
    pub bytes: Vec<u8>,
}
impl Packed {
    pub fn cook(
        mut keys: Vec<(u32, [f32; 4])>,
        dimensions: u8,
        interpolation: Interpolation,
        budget: f32,
    ) -> Self {
        let mut min = keys[0].1;
        let mut max = min;
        for (_, key) in &keys {
            for c in 0..usize::from(dimensions) {
                min[c] = min[c].min(key[c]);
                max[c] = max[c].max(key[c]);
            }
        }
        let mut span = [0.0; 4];
        let mut bits = [0; 4];
        for c in 0..usize::from(dimensions) {
            span[c] = max[c] - min[c];
            if span[c] == 0.0 {
                continue;
            }
            bits[c] = (8..=23)
                .find(|b| {
                    let e = span[c] / (2.0 * ((1_u32 << b) - 1) as f32)
                        + (min[c].abs() + span[c].abs()) * f32::EPSILON * 4.0;
                    e * (f32::from(dimensions)).sqrt() <= budget
                })
                .unwrap_or(32);
            if bits[c] == 32 {
                span[c] = 0.0;
                min[c] = 0.0;
            }
        }
        if bits.iter().all(|b| *b == 0) {
            keys.truncate(1);
        }
        let stride = bits.iter().map(|b| usize::from(*b)).sum::<usize>();
        let mut bytes = vec![0; (stride * keys.len()).div_ceil(8)];
        for (i, (_, value)) in keys.iter().enumerate() {
            let mut offset = i * stride;
            for c in 0..usize::from(dimensions) {
                let count = bits[c];
                let quantized = if count == 32 {
                    value[c].to_bits()
                } else if count == 0 {
                    0
                } else {
                    (((value[c] - min[c]) / span[c]).clamp(0.0, 1.0)
                        * ((1_u32 << count) - 1) as f32)
                        .round() as u32
                };
                for bit in 0..usize::from(count) {
                    bytes[(offset + bit) / 8] |=
                        (((quantized >> bit) & 1) as u8) << ((offset + bit) % 8);
                }
                offset += usize::from(count);
            }
        }
        Self {
            times: keys.into_iter().map(|k| k.0).collect(),
            min,
            span,
            bits,
            dimensions,
            interpolation,
            bytes,
        }
    }
    #[inline]
    pub fn decode(&self, index: usize) -> [f32; 4] {
        let stride = self.bits.iter().map(|b| usize::from(*b)).sum::<usize>();
        let mut offset = index * stride;
        let mut out = self.min;
        for (c, value) in out
            .iter_mut()
            .enumerate()
            .take(usize::from(self.dimensions))
        {
            let count = self.bits[c];
            if count == 0 {
                continue;
            }
            let start = offset / 8;
            let length =
                ((offset % 8 + usize::from(count)).div_ceil(8)).min(self.bytes.len() - start);
            let mut window = [0; 8];
            window[..length].copy_from_slice(&self.bytes[start..start + length]);
            let raw =
                ((u64::from_le_bytes(window) >> (offset % 8)) & ((1_u64 << count) - 1)) as u32;
            *value = if count == 32 {
                f32::from_bits(raw)
            } else {
                self.min[c] + (raw as f32 / ((1_u32 << count) - 1) as f32) * self.span[c]
            };
            offset += usize::from(count);
        }
        out
    }
    pub fn bound(&self) -> f32 {
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
    pub fn validate(&self, duration: u32, dimensions: u8) -> Result<(), Error> {
        ensure(
            self.dimensions == dimensions && !self.times.is_empty(),
            "invalid packed track dimensions",
        )?;
        ensure(
            self.times.iter().all(|t| *t <= duration) && self.times.windows(2).all(|w| w[0] < w[1]),
            "invalid packed key times",
        )?;
        ensure(
            self.min.iter().all(|f| f.is_finite())
                && self.span.iter().all(|f| f.is_finite() && *f >= 0.0),
            "invalid packed ranges",
        )?;
        ensure(
            self.bits.iter().all(|b| *b <= 23 || *b == 32)
                && self.bits[usize::from(dimensions)..].iter().all(|b| *b == 0),
            "invalid packed bit rates",
        )?;
        let stride = self.bits.iter().map(|b| usize::from(*b)).sum::<usize>();
        ensure(
            stride
                .checked_mul(self.times.len())
                .is_some_and(|bits| bits.div_ceil(8) == self.bytes.len()),
            "invalid packed track byte count",
        )?;
        for i in 0..self.times.len() {
            ensure(
                self.decode(i).iter().all(|v| v.is_finite()),
                "non-finite packed key",
            )?;
        }
        Ok(())
    }
}
