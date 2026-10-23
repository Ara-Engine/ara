//! Cooked skin (FORMAT.md skeleton/clip, v1) → ara-anim source data.
//!
//! - Joints are reordered parent-first; `joint_map` maps the mesh's skin-order joint
//!   indices to skeleton order.
//! - Non-joint nodes between a joint and its joint parent (or above a root joint) are
//!   flattened into the joint's bind and into its animated keys (rigid prefixes).
//! - Clip seconds become integer ticks at [`TICKS_PER_SECOND`]; cubic splines keep
//!   their key values (linear between keys); morph channels are not animated yet.
use crate::{Result, RuntimeError};
use ara_anim::data::{
    AnimMatrix, AnimTransform, ClipData, Interpolation, Joint as AnimJoint, JointTrack,
    RotationKey, SkeletonData, VectorKey,
};
use ara_asset_format::{Channel, Clip, Joint, Skeleton as CookedSkeleton};
use glam::{Mat4, Quat, Vec3};
use std::collections::BTreeMap;

/// Clip timebase shared by every runtime clip; one 60 Hz tick is 800 clip ticks.
pub const TICKS_PER_SECOND: u32 = 48_000;

/// A cooked skeleton plus its source clips, shared by every animator of a model.
#[derive(Debug)]
pub struct Skin {
    /// Validated parent-first skeleton with the skin's inverse binds.
    pub skeleton: ara_anim::Skeleton,
    /// Skin-order joint index → skeleton joint index.
    pub joint_map: Vec<usize>,
    /// Source clips by animation name, one track per skeleton joint.
    pub clips: BTreeMap<String, ClipData>,
}

impl Skin {
    /// Builds the skeleton and converts every clip. `locals`/`parents` are the source
    /// scene's node matrices and hierarchy (from the import manifest).
    pub fn cook(
        skeleton: &[u8],
        clips: &[(String, &[u8])],
        locals: &[Mat4],
        parents: &BTreeMap<usize, usize>,
    ) -> Result<Self> {
        let cooked = CookedSkeleton::read(skeleton)?;
        let joints: Vec<Joint> = cooked
            .joints
            .as_chunks::<112>()
            .0
            .iter()
            .map(|raw| bytemuck::pod_read_unaligned(raw))
            .collect();
        let names: Vec<&str> = cooked.joint_names().collect();
        let count = joints.len();
        let mut order = Vec::with_capacity(count);
        let mut placed = vec![false; count];
        while order.len() < count {
            let before = order.len();
            for i in 0..count {
                let parent = joints[i].parent;
                if !placed[i] && (parent < 0 || placed[parent as usize]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            if order.len() == before {
                return Err(RuntimeError("skeleton hierarchy has a cycle".into()));
            }
        }
        let mut joint_map = vec![0; count];
        for (anim, &skin) in order.iter().enumerate() {
            joint_map[skin] = anim;
        }
        let node_of: Vec<usize> = joints.iter().map(|j| j.node as usize).collect();
        let joint_nodes: BTreeMap<usize, usize> =
            node_of.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        // Rigid transform of the non-joint nodes between a joint and its joint parent.
        let prefixes: Vec<Mat4> = node_of
            .iter()
            .map(|&node| {
                let mut prefix = Mat4::IDENTITY;
                let mut cursor = parents.get(&node).copied();
                while let Some(p) = cursor {
                    if joint_nodes.contains_key(&p) {
                        break;
                    }
                    prefix = locals.get(p).copied().unwrap_or(Mat4::IDENTITY) * prefix;
                    cursor = parents.get(&p).copied();
                }
                prefix
            })
            .collect();
        let rest = |j: &Joint| {
            Mat4::from_scale_rotation_translation(
                Vec3::new(j.rest[7], j.rest[8], j.rest[9]),
                Quat::from_xyzw(j.rest[3], j.rest[4], j.rest[5], j.rest[6]).normalize(),
                Vec3::new(j.rest[0], j.rest[1], j.rest[2]),
            )
        };
        let data = SkeletonData {
            joints: order
                .iter()
                .map(|&i| {
                    let (s, r, t) =
                        (prefixes[i] * rest(&joints[i])).to_scale_rotation_translation();
                    AnimJoint {
                        id: format!("node{}", node_of[i]),
                        name: names.get(i).copied().unwrap_or_default().to_owned(),
                        parent: (joints[i].parent >= 0)
                            .then(|| joint_map[joints[i].parent as usize] as u32),
                        bind: AnimTransform {
                            translation: t.to_array(),
                            rotation: r.normalize().to_array(),
                            scale: s.to_array(),
                        },
                        retarget: Default::default(),
                    }
                })
                .collect(),
            motion_root: 0,
            inverse_bind: order
                .iter()
                .map(|&i| {
                    let m = joints[i].inverse_bind;
                    AnimMatrix {
                        x: [m[0], m[1], m[2], m[3]],
                        y: [m[4], m[5], m[6], m[7]],
                        z: [m[8], m[9], m[10], m[11]],
                        w: [m[12], m[13], m[14], m[15]],
                    }
                })
                .collect(),
        };
        let skeleton = ara_anim::Skeleton::cook(data).map_err(|e| RuntimeError(e.0))?;
        let mut converted = BTreeMap::new();
        for (name, bytes) in clips {
            let clip = convert_clip(bytes, count, &joint_map, &joint_nodes, &prefixes)?;
            converted.insert(name.clone(), clip);
        }
        Ok(Self {
            skeleton,
            joint_map,
            clips: converted,
        })
    }
}

fn convert_clip(
    bytes: &[u8],
    joints: usize,
    joint_map: &[usize],
    joint_nodes: &BTreeMap<usize, usize>,
    prefixes: &[Mat4],
) -> Result<ClipData> {
    let clip = Clip::read(bytes)?;
    let float = |stream: &[u8], i: usize| -> f32 {
        f32::from_le_bytes([
            stream[i * 4],
            stream[i * 4 + 1],
            stream[i * 4 + 2],
            stream[i * 4 + 3],
        ])
    };
    let mut tracks = vec![JointTrack::default(); joints];
    let mut end = 0.0f32;
    for raw in clip.channels.as_chunks::<32>().0 {
        let ch: Channel = bytemuck::pod_read_unaligned(raw);
        let Some(&skin) = joint_nodes.get(&(ch.node as usize)) else {
            continue;
        };
        let (prefix_scale, prefix_rotation, prefix_translation) =
            prefixes[skin].to_scale_rotation_translation();
        let rigid = prefix_scale.abs_diff_eq(Vec3::ONE, 1e-5);
        let track = &mut tracks[joint_map[skin]];
        let stride = if ch.interpolation == 2 { 3 } else { 1 };
        let mode = if ch.interpolation == 0 {
            Interpolation::Step
        } else {
            Interpolation::Linear
        };
        let width = ch.components as usize;
        for k in 0..ch.key_count as usize {
            let seconds = float(clip.times, ch.time_offset as usize + k);
            end = end.max(seconds);
            let tick = (f64::from(seconds) * f64::from(TICKS_PER_SECOND)).round() as u32;
            let base = ch.value_offset as usize + (k * stride + stride / 2) * width;
            let value = |c: usize| float(clip.values, base + c);
            match ch.path {
                0 => {
                    let mut v = Vec3::new(value(0), value(1), value(2));
                    if rigid {
                        v = prefix_translation + prefix_rotation * v;
                    }
                    push(
                        &mut track.translations,
                        tick,
                        |k| k.tick,
                        VectorKey {
                            tick,
                            value: v.to_array(),
                        },
                    );
                    track.translation_interpolation = mode;
                }
                1 => {
                    let mut q = Quat::from_xyzw(value(0), value(1), value(2), value(3)).normalize();
                    if rigid {
                        q = (prefix_rotation * q).normalize();
                    }
                    push(
                        &mut track.rotations,
                        tick,
                        |k| k.tick,
                        RotationKey {
                            tick,
                            value: q.to_array(),
                        },
                    );
                    track.rotation_interpolation = mode;
                }
                2 => {
                    let v = [value(0), value(1), value(2)];
                    push(
                        &mut track.scales,
                        tick,
                        |k| k.tick,
                        VectorKey { tick, value: v },
                    );
                    track.scale_interpolation = mode;
                }
                _ => {}
            }
        }
    }
    let duration = ((f64::from(end) * f64::from(TICKS_PER_SECOND)).round() as u32).max(1);
    Ok(ClipData {
        duration,
        ticks_per_second: TICKS_PER_SECOND,
        tracks,
        curves: Vec::new(),
        notifies: Vec::new(),
    })
}

/// Appends a key, replacing the previous one when rounding produced the same tick.
fn push<K>(keys: &mut Vec<K>, tick: u32, tick_of: impl Fn(&K) -> u32, key: K) {
    match keys.last_mut() {
        Some(last) if tick_of(last) >= tick => *last = key,
        _ => keys.push(key),
    }
}
