use crate::{
    CookedClip, Error, Pose, RootMotion, Skeleton, data::Interpolation, ensure,
    quantization::Packed, sampling_plan::BlockCursor,
};
use glam::Quat;

#[derive(Debug, Clone, Copy, Default)]
struct Cursor {
    key: usize,
    valid: bool,
    a: [f32; 4],
    b: [f32; 4],
}
impl Cursor {
    #[inline]
    fn sample(&mut self, track: &Packed, tick: f64, rotation: bool) -> [f32; 4] {
        let key = locate(track, tick, self.key);
        if !self.valid || self.key != key {
            let adjacent = self.valid && key == self.key + 1;
            self.key = key;
            self.valid = true;
            self.a = if adjacent { self.b } else { track.decode(key) };
            self.b = track.decode((key + 1).min(track.times.len() - 1));
            if rotation {
                if !adjacent {
                    self.a = Quat::from_array(self.a).normalize().to_array();
                }
                self.b = Quat::from_array(self.b).normalize().to_array();
            }
        }
        let fraction = fraction(track, key, tick);
        if rotation {
            crate::pose::nlerp(Quat::from_array(self.a), Quat::from_array(self.b), fraction)
                .to_array()
        } else {
            glam::Vec4::from_array(self.a)
                .lerp(glam::Vec4::from_array(self.b), fraction)
                .to_array()
        }
    }
}
#[inline]
fn locate(track: &Packed, tick: f64, cached: usize) -> usize {
    let last = track.times.len() - 1;
    if tick < f64::from(track.times[cached]) {
        return track
            .times
            .partition_point(|t| f64::from(*t) <= tick)
            .saturating_sub(1);
    }
    let mut key = cached;
    while key < last && tick >= f64::from(track.times[key + 1]) {
        key += 1;
    }
    key
}
#[inline]
fn fraction(track: &Packed, key: usize, tick: f64) -> f32 {
    if track.interpolation == Interpolation::Step || key + 1 == track.times.len() {
        return 0.0;
    }
    ((tick - f64::from(track.times[key])) / f64::from(track.times[key + 1] - track.times[key]))
        .clamp(0.0, 1.0) as f32
}
pub(crate) fn sample_uncached(track: &Packed, tick: f64, rotation: bool) -> [f32; 4] {
    let key = track
        .times
        .partition_point(|t| f64::from(*t) <= tick)
        .saturating_sub(1);
    Cursor {
        key,
        ..Default::default()
    }
    .sample(track, tick, rotation)
}

/// Per-actor decompressed key cache. Seeking backwards and clip changes are safe.
#[derive(Debug)]
pub struct Sampler {
    signature: u64,
    cursors: Vec<Cursor>,
    blocks: Vec<BlockCursor>,
    joints: usize,
}
impl Sampler {
    /// Allocates cursors for a clip; initialize outside the simulation loop.
    pub fn new(clip: &CookedClip) -> Self {
        Self {
            signature: clip.signature,
            cursors: vec![Cursor::default(); clip.curves.len()],
            blocks: vec![BlockCursor::default(); clip.plan.block_count()],
            joints: clip.joints,
        }
    }
    /// Invalidates derived caches after rollback; does not allocate.
    pub fn reset(&mut self) {
        self.cursors.fill(Cursor::default());
        self.blocks.fill(BlockCursor::default());
    }
    /// Samples an exact local tick, clamped at the endpoints, into caller storage.
    /// `curves` must have exactly `clip.curve_count()` elements.
    pub fn sample(
        &mut self,
        clip: &CookedClip,
        skeleton: &Skeleton,
        tick: u32,
        out: &mut Pose,
        curves: &mut [f32],
    ) -> Result<(), Error> {
        self.sample_at(
            clip,
            skeleton,
            f64::from(tick.min(clip.duration)),
            out,
            curves,
        )
    }
    pub(crate) fn sample_at(
        &mut self,
        clip: &CookedClip,
        skeleton: &Skeleton,
        tick: f64,
        out: &mut Pose,
        curves: &mut [f32],
    ) -> Result<(), Error> {
        self.sample_prefix_at(clip, skeleton, tick, skeleton.len(), out, curves)
    }
    /// Presentation-only bone LOD: sample a parent-closed joint prefix and leave
    /// remaining joints at bind pose. Scalar curves retain their full rate.
    /// The caller chooses the prefix from screen size; zero is valid.
    #[allow(clippy::too_many_arguments)]
    pub fn sample_prefix(
        &mut self,
        clip: &CookedClip,
        skeleton: &Skeleton,
        tick: u32,
        joints: usize,
        out: &mut Pose,
        curves: &mut [f32],
    ) -> Result<(), Error> {
        self.sample_prefix_at(
            clip,
            skeleton,
            f64::from(tick.min(clip.duration)),
            joints,
            out,
            curves,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn sample_prefix_at(
        &mut self,
        clip: &CookedClip,
        skeleton: &Skeleton,
        tick: f64,
        joints: usize,
        out: &mut Pose,
        curves: &mut [f32],
    ) -> Result<(), Error> {
        ensure(
            skeleton.signature() == clip.skeleton
                && out.len() == skeleton.len()
                && curves.len() == clip.curves.len()
                && self.cursors.len() == clip.curves.len()
                && self.joints == clip.joints
                && joints <= skeleton.len(),
            "sampling skeleton or buffer mismatch",
        )?;
        if self.signature != clip.signature {
            self.reset();
            self.signature = clip.signature;
        }
        if joints == skeleton.len() {
            out.copy_from(&clip.plan.defaults)?;
        } else {
            out.copy_from(&skeleton.bind)?;
            out.translations[..joints].copy_from_slice(&clip.plan.defaults.translations[..joints]);
            out.rotations[..joints].copy_from_slice(&clip.plan.defaults.rotations[..joints]);
            out.scales[..joints].copy_from_slice(&clip.plan.defaults.scales[..joints]);
        }
        let mut block_index = 0;
        for line in &clip.plan.timelines {
            let key = if let Some(step) = line.step {
                ((tick - f64::from(line.times[0])).max(0.0) / f64::from(step)) as usize
            } else {
                line.times
                    .partition_point(|t| f64::from(*t) <= tick)
                    .saturating_sub(1)
            }
            .min(line.times.len() - 1);
            let fraction =
                if line.interpolation == Interpolation::Step || key + 1 == line.times.len() {
                    0.0
                } else {
                    ((tick - f64::from(line.times[key]))
                        / f64::from(line.times[key + 1] - line.times[key]))
                    .clamp(0.0, 1.0) as f32
                };
            for block in &line.blocks {
                if block.joints[0] < joints {
                    // More fragmented compatible clips can exceed the original
                    // cache capacity. A stack cursor keeps rebinding allocation-free.
                    let mut uncached = BlockCursor::default();
                    self.blocks
                        .get_mut(block_index)
                        .unwrap_or(&mut uncached)
                        .sample(block, key, fraction, line.times.len() - 1, out, joints);
                }
                block_index += 1;
            }
        }
        for (i, curve) in clip.curves.iter().enumerate() {
            curves[i] = self.cursors[i].sample(&curve.track, tick, false)[0];
        }
        Ok(())
    }
}

impl CookedClip {
    /// Extracts rigid displacement across any number of forward clip loops.
    /// The caller owns the unwrapped integer clock, including after rollback.
    pub fn root_motion(
        &self,
        skeleton: &Skeleton,
        previous: u64,
        current: u64,
        looping: bool,
    ) -> Result<RootMotion, Error> {
        ensure(
            skeleton.signature() == self.skeleton && current >= previous,
            "invalid root motion skeleton or time interval",
        )?;
        let d = u64::from(self.duration);
        let previous = if looping { previous } else { previous.min(d) };
        let current = if looping { current } else { current.min(d) };
        if previous == current {
            return Ok(RootMotion::default());
        }
        let (start, cycle) = self.root_cache;
        let a = RootMotion::between(start, self.root_at(skeleton, (previous % d) as f64));
        let b = RootMotion::between(start, self.root_at(skeleton, (current % d) as f64));
        Ok(a.inverse()
            .compose(cycle.pow(current / d - previous / d))
            .compose(b))
    }
    pub(crate) fn root_at(&self, skeleton: &Skeleton, tick: f64) -> RootMotion {
        let i = skeleton.data.motion_root as usize;
        let track = &self.root_track;
        let translation =
            track
                .translation
                .as_ref()
                .map_or(skeleton.bind.translations[i].into(), |t| {
                    let v = sample_uncached(t, tick, false);
                    glam::Vec3::new(v[0], v[1], v[2])
                });
        let rotation = track
            .rotation
            .as_ref()
            .map_or(skeleton.bind.rotations[i], |t| {
                Quat::from_array(sample_uncached(t, tick, true))
            });
        RootMotion {
            translation,
            rotation,
        }
    }
}
