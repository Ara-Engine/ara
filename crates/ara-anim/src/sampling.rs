use crate::{
    CookedClip, Error, Pose, RootMotion, Skeleton, Transform, data::Interpolation, ensure,
    quantization::Packed,
};
use glam::{Quat, Vec3A};

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
        let key = locate(track, tick, if self.valid { self.key } else { 0 });
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
    Cursor::default().sample(track, tick, rotation)
}

/// Per-actor decompressed key cache. Seeking backwards and clip changes are safe.
#[derive(Debug)]
pub struct Sampler {
    signature: u64,
    cursors: Vec<Cursor>,
}
impl Sampler {
    /// Allocates cursors for a clip; initialize outside the simulation loop.
    pub fn new(clip: &CookedClip) -> Self {
        Self {
            signature: clip.signature,
            cursors: vec![Cursor::default(); clip.tracks.len() * 3 + clip.curves.len()],
        }
    }
    /// Invalidates derived caches after rollback; does not allocate.
    pub fn reset(&mut self) {
        self.cursors.fill(Cursor::default());
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
        ensure(
            skeleton.signature() == clip.skeleton
                && out.len() == skeleton.len()
                && curves.len() == clip.curves.len()
                && self.cursors.len() == clip.tracks.len() * 3 + clip.curves.len(),
            "sampling skeleton or buffer mismatch",
        )?;
        if self.signature != clip.signature {
            self.reset();
            self.signature = clip.signature;
        }
        for (i, track) in clip.tracks.iter().enumerate() {
            let mut t = Transform {
                translation: skeleton.bind.translations[i],
                rotation: skeleton.bind.rotations[i],
                scale: skeleton.bind.scales[i],
            };
            if let Some(track) = &track.translation {
                let v = self.cursors[i * 3].sample(track, tick, false);
                t.translation = Vec3A::new(v[0], v[1], v[2]);
            }
            if let Some(track) = &track.rotation {
                t.rotation = Quat::from_array(self.cursors[i * 3 + 1].sample(track, tick, true));
            }
            if let Some(track) = &track.scale {
                let v = self.cursors[i * 3 + 2].sample(track, tick, false);
                t.scale = Vec3A::new(v[0], v[1], v[2]);
            }
            out.set(i, t);
        }
        for (i, curve) in clip.curves.iter().enumerate() {
            curves[i] =
                self.cursors[clip.tracks.len() * 3 + i].sample(&curve.track, tick, false)[0];
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
        let start = self.root_at(skeleton, 0.0);
        let cycle = RootMotion::between(start, self.root_at(skeleton, f64::from(self.duration)));
        let a = RootMotion::between(start, self.root_at(skeleton, (previous % d) as f64));
        let b = RootMotion::between(start, self.root_at(skeleton, (current % d) as f64));
        Ok(a.inverse()
            .compose(cycle.pow(current / d - previous / d))
            .compose(b))
    }
    pub(crate) fn root_at(&self, skeleton: &Skeleton, tick: f64) -> RootMotion {
        let i = skeleton.data.motion_root as usize;
        let track = &self.tracks[i];
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
