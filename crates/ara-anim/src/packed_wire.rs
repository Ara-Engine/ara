//! Compact cache metadata; runtime keys keep padded windows and prepared scales.
use crate::{
    data::Interpolation,
    prediction::Payload,
    quantization::{Packed, Segment},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

#[derive(Serialize, Deserialize)]
enum Times {
    Single(u32),
    Uniform { first: u32, step: u32, count: u32 },
    Explicit(Vec<u32>),
}
#[derive(Serialize, Deserialize)]
enum Component {
    Zero,
    One,
    Constant(f32),
    Quantized { min: f32, span: f32, bits: u8 },
    Raw,
}
#[derive(Serialize, Deserialize)]
struct WireSegment {
    components: Vec<Component>,
    bytes: Payload,
}
#[derive(Serialize, Deserialize)]
struct Wire {
    times: Times,
    dimensions: u8,
    interpolation: Interpolation,
    omitted: u8,
    segment_len: usize,
    segments: Vec<WireSegment>,
}
impl Serialize for Packed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let times = if self.times.len() == 1 {
            Times::Single(self.times[0])
        } else {
            let step = self.times[1] - self.times[0];
            if self.times.windows(2).all(|w| w[1] - w[0] == step) {
                Times::Uniform {
                    first: self.times[0],
                    step,
                    count: self.times.len() as u32,
                }
            } else {
                Times::Explicit(self.times.clone())
            }
        };
        let segments = self
            .segments
            .iter()
            .enumerate()
            .map(|(i, s)| WireSegment {
                components: (0..usize::from(self.dimensions))
                    .map(|c| match s.bits[c] {
                        0 if s.min[c].to_bits() == 0 => Component::Zero,
                        0 if s.min[c] == 1.0 => Component::One,
                        0 => Component::Constant(s.min[c]),
                        32 => Component::Raw,
                        bits => Component::Quantized {
                            min: s.min[c],
                            span: s.span[c],
                            bits,
                        },
                    })
                    .collect(),
                bytes: Payload::encode(
                    s,
                    self.segment_len
                        .min(self.times.len() - i * self.segment_len),
                    self.dimensions,
                ),
            })
            .collect();
        Wire {
            times,
            dimensions: self.dimensions,
            interpolation: self.interpolation,
            omitted: self.omitted,
            segment_len: self.segment_len,
            segments,
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Packed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        if !(1..=4).contains(&wire.dimensions) {
            return Err(D::Error::custom("invalid dimensions"));
        }
        let times = match wire.times {
            Times::Single(t) => vec![t],
            Times::Explicit(t) => t,
            Times::Uniform { first, step, count } => {
                if count == 0
                    || count > 4_000_000
                    || step == 0
                    || step
                        .checked_mul(count - 1)
                        .and_then(|t| t.checked_add(first))
                        .is_none()
                {
                    return Err(D::Error::custom("invalid uniform times"));
                }
                (0..count).map(|i| first + i * step).collect()
            }
        };
        let mut segments = Vec::with_capacity(wire.segments.len());
        if times.is_empty()
            || wire.segment_len == 0
            || wire.segments.len() != times.len().div_ceil(wire.segment_len)
        {
            return Err(D::Error::custom("invalid segment count"));
        }
        for (i, s) in wire.segments.into_iter().enumerate() {
            if s.components.len() != usize::from(wire.dimensions) {
                return Err(D::Error::custom("invalid components"));
            }
            let mut segment = Segment::from_parts(Vec::new());
            for (c, component) in s.components.into_iter().enumerate() {
                match component {
                    Component::Zero => {}
                    Component::One => segment.min[c] = 1.0,
                    Component::Constant(v) => segment.min[c] = v,
                    Component::Raw => segment.bits[c] = 32,
                    Component::Quantized { min, span, bits } => {
                        if !(1..=23).contains(&bits) {
                            return Err(D::Error::custom("invalid bit rate"));
                        }
                        segment.min[c] = min;
                        segment.span[c] = span;
                        segment.bits[c] = bits;
                    }
                }
            }
            segment.bytes = s
                .bytes
                .decode(
                    segment.bits,
                    wire.segment_len.min(times.len() - i * wire.segment_len),
                    wire.dimensions,
                )
                .map_err(D::Error::custom)?;
            segment.prepare();
            segments.push(segment);
        }
        Ok(Packed {
            times,
            dimensions: wire.dimensions,
            interpolation: wire.interpolation,
            omitted: wire.omitted,
            segment_len: wire.segment_len,
            segments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_metadata_is_rejected_before_sampling() {
        let valid = || Wire {
            times: Times::Uniform {
                first: 0,
                step: 1,
                count: 2,
            },
            dimensions: 3,
            interpolation: Interpolation::Linear,
            omitted: 4,
            segment_len: 2,
            segments: vec![WireSegment {
                components: vec![Component::Zero, Component::Zero, Component::Zero],
                bytes: Payload::Raw(vec![]),
            }],
        };
        for case in 0..9 {
            let mut wire = valid();
            match case {
                0 => {
                    wire.times = Times::Uniform {
                        first: u32::MAX,
                        step: 2,
                        count: 2,
                    }
                }
                1 => {
                    wire.times = Times::Uniform {
                        first: 0,
                        step: 0,
                        count: 2,
                    }
                }
                2 => {
                    wire.times = Times::Uniform {
                        first: 0,
                        step: 1,
                        count: u32::MAX,
                    }
                }
                3 => wire.segment_len = 0,
                4 => wire.dimensions = 5,
                5 => {
                    wire.segments[0].components[0] = Component::Quantized {
                        min: 0.0,
                        span: 1.0,
                        bits: 24,
                    }
                }
                6 => wire.segments[0].bytes = Payload::Raw(vec![0]),
                7 => wire.segments[0].components[0] = Component::Constant(f32::NAN),
                _ => wire.omitted = 0,
            }
            let bytes = postcard::to_allocvec(&wire).expect("test fixture serializes");
            let decoded: Result<Packed, _> = postcard::from_bytes(&bytes);
            assert!(
                decoded.is_err() || decoded.is_ok_and(|p| p.validate(100, 3).is_err()),
                "case {case}"
            );
        }
    }
}
