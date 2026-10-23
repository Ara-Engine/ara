//! Derived time-major compressed packets. No expanded float key cache is retained.
use crate::{
    compression::Track,
    sampling_plan::{Block, Channel},
};
use glam::Vec4;

/// An active component's bit address within one time-major four-track key.
#[derive(Debug)]
struct Component {
    component: usize,
    lane: usize,
    offset: usize,
    bits: u8,
}
/// Shared range metadata until any constituent track starts a new segment.
#[derive(Debug)]
struct Packet {
    first: usize,
    end: usize,
    stride: usize,
    components: Vec<Component>,
    min: [Vec4; 4],
    scale: [Vec4; 4],
    omitted: [glam::BVec4A; 4],
    bytes: Vec<u8>,
}
/// Four tracks' compressed keys are adjacent at each shared key time.
#[derive(Debug, Default)]
pub(crate) struct BlockKeys {
    packets: Vec<Packet>,
}
impl BlockKeys {
    /// Repackages validated integers without introducing another quantization.
    pub fn new(joints: [usize; 4], channel: Channel, tracks: &[Track]) -> Self {
        let channels = joints.map(|joint| {
            channel
                .track(&tracks[joint])
                .expect("compiled dynamic channel")
        });
        let count = channels[0].times.len();
        let mut packets = Vec::new();
        let mut first = 0;
        while first < count {
            let end = channels
                .iter()
                .map(|t| ((first / t.segment_len + 1) * t.segment_len).min(count))
                .min()
                .expect("four channels");
            let segments = channels.map(|t| &t.segments[first / t.segment_len]);
            let min = std::array::from_fn(|c| Vec4::from_array(segments.map(|s| s.min[c])));
            let scale = std::array::from_fn(|c| {
                Vec4::from_array(segments.map(|s| match s.bits[c] {
                    0 => 0.0,
                    32 => 1.0,
                    b => s.span[c] / ((1_u32 << b) - 1) as f32,
                }))
            });
            let omitted = std::array::from_fn(|c| {
                glam::BVec4A::from_array(channels.map(|t| t.omitted as usize == c))
            });
            let mut components = Vec::new();
            let mut stride = 0;
            for c in 0..4 {
                for (lane, s) in segments.iter().enumerate() {
                    if s.bits[c] > 0 {
                        components.push(Component {
                            component: c,
                            lane,
                            offset: stride,
                            bits: s.bits[c],
                        });
                        stride += usize::from(s.bits[c]);
                    }
                }
            }
            components.shrink_to_fit();
            let mut bytes = vec![0; (stride * (end - first)).div_ceil(8) + 8];
            for key in first..end {
                for c in &components {
                    let raw = segments[c.lane].raw(key % channels[c.lane].segment_len, c.component);
                    let offset = (key - first) * stride + c.offset;
                    for bit in 0..usize::from(c.bits) {
                        bytes[(offset + bit) / 8] |=
                            (((raw >> bit) & 1) as u8) << ((offset + bit) % 8);
                    }
                }
            }
            packets.push(Packet {
                first,
                end,
                stride,
                components,
                min,
                scale,
                omitted,
                bytes,
            });
            first = end;
        }
        packets.shrink_to_fit();
        Self { packets }
    }
    #[inline]
    /// Decodes four tracks, applying range reduction in SIMD across tracks.
    pub fn decode(&self, key: usize, rotation: bool) -> [Vec4; 4] {
        let index = if key < self.packets[0].end {
            0
        } else {
            self.packets.partition_point(|p| p.end <= key)
        };
        let packet = &self.packets[index];
        let offset = (key - packet.first) * packet.stride;
        let mut raw = [[0.0; 4]; 4];
        for c in &packet.components {
            let bit = offset + c.offset;
            let start = bit / 8;
            let window = u64::from_le_bytes(
                packet.bytes[start..start + 8]
                    .try_into()
                    .expect("padded packet window"),
            );
            let value = ((window >> (bit % 8)) & ((1_u64 << c.bits) - 1)) as u32;
            raw[c.component][c.lane] = if c.bits == 32 {
                f32::from_bits(value)
            } else {
                value as f32
            };
        }
        let mut value: [Vec4; 4] =
            std::array::from_fn(|c| packet.min[c] + Vec4::from_array(raw[c]) * packet.scale[c]);
        if rotation {
            let sum = (value[0] * value[0] + value[1] * value[1])
                + (value[2] * value[2] + value[3] * value[3]);
            let dominant = (Vec4::ONE - sum).max(Vec4::ZERO).sqrt();
            for (c, v) in value.iter_mut().enumerate() {
                *v = Vec4::select(packet.omitted[c], dominant, *v);
            }
        }
        value
    }
    pub fn storage_bytes(&self) -> usize {
        self.packets.capacity() * std::mem::size_of::<Packet>()
            + self
                .packets
                .iter()
                .map(|p| {
                    p.bytes.capacity() + p.components.capacity() * std::mem::size_of::<Component>()
                })
                .sum::<usize>()
    }
}

impl Block {
    pub fn prepare(&mut self, tracks: &[Track]) {
        self.keys = BlockKeys::new(self.joints, self.channel, tracks);
    }
}
