//! Lossless integer prediction in cache blobs; runtime packets keep direct keys.
use crate::{Error, ensure, quantization::Segment};
use serde::{Deserialize, Serialize};

/// Delta rates are independent of the lossy component quantization rates.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Payload {
    Raw(Vec<u8>),
    Predicted { widths: Vec<u8>, bytes: Vec<u8> },
}

fn write(bytes: &mut [u8], offset: &mut usize, value: u32, bits: u8) {
    for bit in 0..usize::from(bits) {
        bytes[(*offset + bit) / 8] |= (((value >> bit) & 1) as u8) << ((*offset + bit) % 8);
    }
    *offset += usize::from(bits);
}
fn read(bytes: &[u8], offset: &mut usize, bits: u8) -> u32 {
    if bits == 0 {
        return 0;
    }
    let start = *offset / 8;
    let word = u64::from_le_bytes(
        bytes[start..start + 8]
            .try_into()
            .expect("checked padded prediction window"),
    );
    let value = ((word >> (*offset % 8)) & ((1_u64 << bits) - 1)) as u32;
    *offset += usize::from(bits);
    value
}
fn zigzag(previous: u32, current: u32) -> u32 {
    let delta = i64::from(current) - i64::from(previous);
    ((delta << 1) ^ (delta >> 63)) as u32
}
impl Payload {
    /// Selects raw or predicted integers by complete serialized size, offline.
    pub fn encode(segment: &Segment, count: usize, dimensions: u8) -> Self {
        let raw = Self::Raw(segment.bytes[..segment.bytes.len() - 8].to_vec());
        if count < 2 || segment.bits.contains(&32) {
            return raw;
        }
        let widths: Vec<_> = (0..usize::from(dimensions))
            .map(|c| {
                let max = (1..count)
                    .map(|i| zigzag(segment.raw(i - 1, c), segment.raw(i, c)))
                    .max()
                    .unwrap_or(0);
                (32 - max.leading_zeros()) as u8
            })
            .collect();
        let stride = segment.bits.iter().map(|b| usize::from(*b)).sum::<usize>();
        let delta_stride = widths.iter().map(|b| usize::from(*b)).sum::<usize>();
        let mut bytes = vec![0; (stride + delta_stride * (count - 1)).div_ceil(8)];
        let mut offset = 0;
        for c in 0..usize::from(dimensions) {
            write(&mut bytes, &mut offset, segment.raw(0, c), segment.bits[c]);
        }
        for key in 1..count {
            for (c, width) in widths.iter().enumerate() {
                write(
                    &mut bytes,
                    &mut offset,
                    zigzag(segment.raw(key - 1, c), segment.raw(key, c)),
                    *width,
                );
            }
        }
        let predicted = Self::Predicted { widths, bytes };
        let size = |p: &Self| {
            postcard::to_allocvec(p)
                .expect("integer payload serializes")
                .len()
        };
        if size(&predicted) < size(&raw) {
            predicted
        } else {
            raw
        }
    }
    /// Checks bit lengths and integer overflow before expanding direct keys.
    pub fn decode(self, rates: [u8; 4], count: usize, dimensions: u8) -> Result<Vec<u8>, Error> {
        ensure(count > 0, "empty predicted channel")?;
        let stride = rates.iter().map(|b| usize::from(*b)).sum::<usize>();
        let length = stride
            .checked_mul(count)
            .ok_or_else(|| Error("prediction size overflow".into()))?
            .div_ceil(8);
        match self {
            Self::Raw(bytes) => {
                ensure(bytes.len() == length, "invalid raw payload length")?;
                Ok(bytes)
            }
            Self::Predicted { widths, mut bytes } => {
                ensure(
                    !rates.contains(&32)
                        && widths.len() == usize::from(dimensions)
                        && widths
                            .iter()
                            .enumerate()
                            .all(|(c, b)| *b <= rates[c] + 1 && (rates[c] != 0 || *b == 0)),
                    "invalid prediction rates",
                )?;
                let delta_stride = widths.iter().map(|b| usize::from(*b)).sum::<usize>();
                let bits = delta_stride
                    .checked_mul(count - 1)
                    .and_then(|n| n.checked_add(stride))
                    .ok_or_else(|| Error("prediction size overflow".into()))?;
                ensure(
                    bytes.len() == bits.div_ceil(8),
                    "invalid prediction payload length",
                )?;
                bytes.resize(bytes.len() + 8, 0);
                let mut previous = [0_u32; 4];
                let mut input = 0;
                let mut output = 0;
                let mut direct = vec![0; length];
                for c in 0..usize::from(dimensions) {
                    previous[c] = read(&bytes, &mut input, rates[c]);
                    write(&mut direct, &mut output, previous[c], rates[c]);
                }
                for _ in 1..count {
                    for (c, width) in widths.iter().enumerate() {
                        let raw = read(&bytes, &mut input, *width);
                        let delta = i64::from(raw >> 1) ^ -i64::from(raw & 1);
                        let value = i64::from(previous[c]) + delta;
                        ensure(
                            (0..=(1_i64 << rates[c]) - 1).contains(&value),
                            "predicted integer outside component range",
                        )?;
                        previous[c] = value as u32;
                        write(&mut direct, &mut output, previous[c], rates[c]);
                    }
                }
                Ok(direct)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn predicted_payload_recovers_the_exact_quantized_bitstream() {
        let keys = (0..256)
            .map(|i| (i, [(i as f32 * 0.025).sin() * 0.01, 0.2, 0.0, 0.0]))
            .collect();
        let track = crate::quantization::Packed::cook(
            keys,
            3,
            crate::data::Interpolation::Linear,
            0.0001,
            false,
        );
        let segment = &track.segments[0];
        let count = track.segment_len.min(track.times.len());
        let payload = Payload::encode(segment, count, 3);
        assert!(matches!(payload, Payload::Predicted { .. }));
        assert_eq!(
            payload
                .decode(segment.bits, count, 3)
                .expect("valid predicted stream"),
            segment.bytes[..segment.bytes.len() - 8]
        );
    }
    #[test]
    fn corrupt_predictions_cannot_overflow_or_read_past_the_payload() {
        for payload in [
            Payload::Predicted {
                widths: vec![3],
                bytes: vec![0],
            },
            Payload::Predicted {
                widths: vec![1],
                bytes: vec![0xff],
            },
            Payload::Predicted {
                widths: vec![1],
                bytes: vec![],
            },
            Payload::Predicted {
                widths: vec![1, 1],
                bytes: vec![0],
            },
        ] {
            assert!(payload.decode([1, 0, 0, 0], 3, 1).is_err());
        }
    }
}
