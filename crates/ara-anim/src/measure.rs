//! Verify the actual blocked runtime, in addition to the scalar codec checks.
use crate::{
    CompressionReport, CookedClip, Error, Pose, Sampler, Skeleton,
    compression::{reference, rotation_distance},
    data::{ClipData, Interpolation},
    ensure,
};
use glam::{Quat, Vec3};
use std::collections::BTreeSet;

type Keys = Vec<(u32, [f32; 4])>;
struct Channel {
    keys: Keys,
    interpolation: Interpolation,
    default: [f32; 4],
    rotation: bool,
}
impl Channel {
    fn sample(&self, tick: f64) -> [f32; 4] {
        if self.keys.is_empty() {
            return self.default;
        }
        let i = self
            .keys
            .partition_point(|k| f64::from(k.0) <= tick)
            .saturating_sub(1);
        let j = (i + 1).min(self.keys.len() - 1);
        let t = if i == j || self.interpolation == Interpolation::Step {
            0.0
        } else {
            ((tick - f64::from(self.keys[i].0)) / f64::from(self.keys[j].0 - self.keys[i].0))
                .clamp(0.0, 1.0) as f32
        };
        reference(&self.keys[i].1, &self.keys[j].1, t, self.rotation)
    }
}
/// Measure the production SIMD path at every source key and 16 subdivisions.
pub(crate) fn runtime(
    data: &ClipData,
    clip: &CookedClip,
    skeleton: &Skeleton,
    report: &mut CompressionReport,
) -> Result<(), Error> {
    let mut channels = Vec::new();
    for (i, t) in data.tracks.iter().enumerate() {
        let bind = skeleton.bind.joint(i).expect("validated skeleton track");
        channels.push([
            Channel {
                keys: t
                    .translations
                    .iter()
                    .map(|k| (k.tick, [k.value[0], k.value[1], k.value[2], 0.0]))
                    .collect(),
                interpolation: t.translation_interpolation,
                default: [
                    bind.translation.x,
                    bind.translation.y,
                    bind.translation.z,
                    0.0,
                ],
                rotation: false,
            },
            Channel {
                keys: t.rotations.iter().map(|k| (k.tick, k.value)).collect(),
                interpolation: t.rotation_interpolation,
                default: bind.rotation.to_array(),
                rotation: true,
            },
            Channel {
                keys: t
                    .scales
                    .iter()
                    .map(|k| (k.tick, [k.value[0], k.value[1], k.value[2], 0.0]))
                    .collect(),
                interpolation: t.scale_interpolation,
                default: [bind.scale.x, bind.scale.y, bind.scale.z, 0.0],
                rotation: false,
            },
        ]);
    }
    let curves: Vec<_> = data
        .curves
        .iter()
        .map(|c| Channel {
            keys: c
                .keys
                .iter()
                .map(|k| (k.tick, [k.value, 0.0, 0.0, 0.0]))
                .collect(),
            interpolation: c.interpolation,
            default: [0.0; 4],
            rotation: false,
        })
        .collect();
    let mut times = BTreeSet::from([0_f64.to_bits(), f64::from(data.duration).to_bits()]);
    for c in channels.iter().flatten().chain(&curves) {
        for (i, a) in c.keys.iter().enumerate() {
            let b = &c.keys[(i + 1).min(c.keys.len() - 1)];
            for sub in 0..=16 {
                times.insert(
                    (f64::from(a.0) + f64::from(b.0 - a.0) * f64::from(sub) / 16.0).to_bits(),
                );
            }
        }
    }
    let mut sampler = Sampler::new(clip);
    let mut pose = Pose::new(skeleton.len());
    let mut scalar = vec![0.0; curves.len()];
    for time in times {
        let tick = f64::from_bits(time);
        sampler.sample_at(clip, skeleton, tick, &mut pose, &mut scalar)?;
        for (i, c) in channels.iter().enumerate() {
            let t = Vec3::from_slice(&c[0].sample(tick)).distance(pose.translations[i].into());
            let r = rotation_distance(Quat::from_array(c[1].sample(tick)), pose.rotations[i]);
            let s = Vec3::from_slice(&c[2].sample(tick)).distance(pose.scales[i].into());
            ensure(
                [t, r, s].iter().all(|e| e.is_finite()),
                "non-finite runtime compression error",
            )?;
            report.translation_error = report.translation_error.max(t);
            report.rotation_error = report.rotation_error.max(r);
            report.scale_error = report.scale_error.max(s);
        }
        for (i, c) in curves.iter().enumerate() {
            let e = (c.sample(tick)[0] - scalar[i]).abs();
            ensure(e.is_finite(), "non-finite runtime scalar error")?;
            report.curve_error = report.curve_error.max(e);
        }
    }
    Ok(())
}
