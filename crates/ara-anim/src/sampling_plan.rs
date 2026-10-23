//! Offline sparse-channel classification and four-track interpolation packets.
use crate::{Pose, Skeleton, compression::Track, data::Interpolation, quantization::Packed};
use glam::{Quat, Vec3A, Vec4};

/// Selects a joint stream during offline sparse-channel classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Channel {
    Translation,
    Rotation,
    Scale,
}
impl Channel {
    pub fn track(self, track: &Track) -> Option<&Packed> {
        match self {
            Self::Translation => &track.translation,
            Self::Rotation => &track.rotation,
            Self::Scale => &track.scale,
        }
        .as_ref()
    }
}
/// A four-track packet; unused tail lanes duplicate the first joint.
#[derive(Debug)]
pub(crate) struct Block {
    pub joints: [usize; 4],
    pub len: usize,
    pub channel: Channel,
    pub keys: crate::packed_block::BlockKeys,
}
/// One bracket/fraction calculation shared by all matching channel timelines.
#[derive(Debug)]
pub(crate) struct Timeline {
    pub times: Vec<u32>,
    pub step: Option<u32>,
    pub interpolation: Interpolation,
    pub blocks: Vec<Block>,
}
/// Bind/constant streams plus compressed packets, built after cache validation.
#[derive(Debug)]
pub(crate) struct SamplingPlan {
    pub defaults: Pose,
    pub timelines: Vec<Timeline>,
}
impl SamplingPlan {
    /// Classifies constants and groups dynamic tracks by exact time/interpolation.
    pub fn new(tracks: &[Track], skeleton: &Skeleton) -> Self {
        let mut defaults = skeleton.bind.clone();
        let mut timelines: Vec<Timeline> = Vec::new();
        for channel in [Channel::Translation, Channel::Rotation, Channel::Scale] {
            for (joint, track) in tracks.iter().enumerate() {
                let Some(t) = channel.track(track) else {
                    continue;
                };
                if t.times.len() == 1 {
                    let v = t.decode(0);
                    match channel {
                        Channel::Translation => {
                            defaults.translations[joint] = Vec3A::from_slice(&v)
                        }
                        Channel::Rotation => {
                            defaults.rotations[joint] = Quat::from_array(v).normalize()
                        }
                        Channel::Scale => defaults.scales[joint] = Vec3A::from_slice(&v),
                    }
                    continue;
                }
                let index = timelines
                    .iter()
                    .position(|line| line.times == t.times && line.interpolation == t.interpolation)
                    .unwrap_or_else(|| {
                        let step = t.times[1] - t.times[0];
                        timelines.push(Timeline {
                            times: t.times.clone(),
                            step: t
                                .times
                                .windows(2)
                                .all(|w| w[1] - w[0] == step)
                                .then_some(step),
                            interpolation: t.interpolation,
                            blocks: Vec::new(),
                        });
                        timelines.len() - 1
                    });
                let blocks = &mut timelines[index].blocks;
                if let Some(block) = blocks
                    .last_mut()
                    .filter(|b| b.channel == channel && b.len < 4)
                {
                    block.joints[block.len] = joint;
                    block.len += 1;
                } else {
                    blocks.push(Block {
                        joints: [joint; 4],
                        len: 1,
                        channel,
                        keys: Default::default(),
                    });
                }
            }
        }
        for line in &mut timelines {
            for block in &mut line.blocks {
                block.prepare(tracks);
            }
            line.blocks.shrink_to_fit();
        }
        timelines.shrink_to_fit();
        Self {
            defaults,
            timelines,
        }
    }
    pub fn block_count(&self) -> usize {
        self.timelines.iter().map(|t| t.blocks.len()).sum()
    }
    pub fn storage_bytes(&self) -> usize {
        self.defaults.len() * 48
            + self.timelines.capacity() * std::mem::size_of::<Timeline>()
            + self
                .timelines
                .iter()
                .map(|t| {
                    t.times.capacity() * 4
                        + t.blocks.capacity() * std::mem::size_of::<Block>()
                        + t.blocks
                            .iter()
                            .map(|b| b.keys.storage_bytes())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

/// Transposes four XYZW values into four SIMD lanes, one lane per track.
pub(crate) fn transpose(v: [[f32; 4]; 4]) -> [Vec4; 4] {
    std::array::from_fn(|c| Vec4::new(v[0][c], v[1][c], v[2][c], v[3][c]))
}
/// Normalizes four quaternions with a fixed arithmetic order for cache replay.
pub(crate) fn normalize(q: [Vec4; 4]) -> [Vec4; 4] {
    let length = ((q[0] * q[0] + q[1] * q[1]) + (q[2] * q[2] + q[3] * q[3])).sqrt();
    q.map(|v| v / length)
}
/// Per-actor endpoints; adjacent keys reuse the previously decoded upper bound.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BlockCursor {
    key: Option<usize>,
    a: [Vec4; 4],
    b: [Vec4; 4],
}
impl BlockCursor {
    /// Decodes only changed brackets, then interpolates four joints in SIMD.
    pub fn sample(
        &mut self,
        block: &Block,
        key: usize,
        fraction: f32,
        last: usize,
        out: &mut Pose,
        joint_limit: usize,
    ) {
        let rotation = block.channel == Channel::Rotation;
        if self.key != Some(key) {
            let adjacent = self.key.is_some_and(|old| key == old + 1);
            let decode = |index: usize| {
                let v = block.keys.decode(index.min(last), rotation);
                if rotation { normalize(v) } else { v }
            };
            self.a = if adjacent { self.b } else { decode(key) };
            self.b = decode(key + 1);
            self.key = Some(key);
        }
        let value = if rotation {
            let dot = (self.a[0] * self.b[0] + self.a[1] * self.b[1])
                + (self.a[2] * self.b[2] + self.a[3] * self.b[3]);
            let sign = Vec4::select(dot.cmplt(Vec4::ZERO), Vec4::splat(-1.0), Vec4::ONE);
            normalize(std::array::from_fn(|c| {
                self.a[c] * (1.0 - fraction) + self.b[c] * (sign * fraction)
            }))
        } else {
            std::array::from_fn(|c| self.a[c] + (self.b[c] - self.a[c]) * fraction)
        };
        let value = transpose(value.map(|v| v.to_array()));
        for (lane, &joint) in block.joints.iter().enumerate().take(block.len) {
            if joint >= joint_limit {
                break;
            }
            match block.channel {
                Channel::Translation => out.translations[joint] = value[lane].truncate().into(),
                Channel::Rotation => {
                    out.rotations[joint] = Quat::from_array(value[lane].to_array())
                }
                Channel::Scale => out.scales[joint] = value[lane].truncate().into(),
            }
        }
    }
}
