//! The listener's room reverb: geometric early reflections plus an FDN late tail,
//! running as an `ara_audio::AuxEffect` fed by every voice's reverb send.
//!
//! The control path converts physical [`ReverbParams`] (reverberation times, room size,
//! level, early taps) into sample-domain targets and pushes them through a bounded
//! lock-free ring. The audio thread only applies them. Room changes crossfade to the
//! second network: sound sent after the change rings in the new room while the old
//! room's tail decays naturally.

mod early;
mod fdn;

use crate::{ROOM_CROSSFADE_SECONDS, propagation::AcousticsSettings};
use ara_audio::{AudioError, AuxEffect, Frame, Result, dsp::CROSSOVER_HZ};
pub use early::MAX_TAPS;
use early::{Early, EarlyTarget};
use fdn::{Fdn, FdnTarget, LINES};
use rtrb::{Consumer, Producer, RingBuffer};

/// One early reflection relative to the listener.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EarlyTap {
    /// Arrival delay after the direct sound, in seconds.
    pub delay: f32,
    /// Left/right amplitude.
    pub gain: [f32; 2],
}

/// Physical room reverb parameters, from a zone, baked probes or a blend of both.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbParams {
    /// Low/mid/high reverberation time (RT60) in seconds; zero disables the late tail.
    pub rt60: [f32; 3],
    /// Mean free path 4V/S in meters; sets the network's delay lengths (room size).
    pub mean_free_path: f32,
    /// Reverberant-to-direct energy ratio at 1 m (16π/R for room constant R).
    pub level: f32,
    /// Return gain, normally `ReverbZone.wet`.
    pub wet: f32,
    /// Early reflections; the first `early_count` are used.
    pub early: [EarlyTap; MAX_TAPS],
    /// Number of valid early taps.
    pub early_count: usize,
    /// High-band reflection loss for early taps, 0 (none) to 1 (dark).
    pub early_damping: f32,
    /// Room identity; a different room crossfades to a fresh network.
    pub room: u64,
}

impl ReverbParams {
    /// No reverb (outdoors with no zone).
    pub fn silent() -> Self {
        Self {
            rt60: [0.0; 3],
            mean_free_path: 4.0,
            level: 0.0,
            wet: 0.0,
            early: [EarlyTap::default(); MAX_TAPS],
            early_count: 0,
            early_damping: 0.0,
            room: 0,
        }
    }

    /// Linear blend for crossing a zone boundary or interpolating probes. Size, taps and
    /// room identity come from whichever side dominates (`t < 0.5` keeps `self`).
    pub fn blend(&self, other: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        let base = if t < 0.5 { self } else { other };
        Self {
            rt60: std::array::from_fn(|b| lerp(self.rt60[b], other.rt60[b])),
            level: lerp(self.level, other.level),
            wet: lerp(self.wet, other.wet),
            ..*base
        }
    }

    fn validate(&self) -> Result<()> {
        let finite = self
            .rt60
            .iter()
            .chain([
                &self.mean_free_path,
                &self.level,
                &self.wet,
                &self.early_damping,
            ])
            .all(|v| v.is_finite() && *v >= 0.0);
        if !finite
            || self.rt60.iter().any(|t| *t > 30.0)
            || self.early_count > MAX_TAPS
            || self.early[..self.early_count].iter().any(|t| {
                !t.delay.is_finite() || t.delay < 0.0 || t.gain.iter().any(|g| !g.is_finite())
            })
        {
            return Err(AudioError::Invalid("reverb parameters"));
        }
        Ok(())
    }
}

/// Sample-domain target, computed on the control path.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Target {
    fdn: FdnTarget,
    early: EarlyTarget,
    wet: f32,
}

/// Longest network delay line, predelay and early tap, in seconds.
const MAX_DELAY: f32 = 0.1;
const MAX_EARLY: f32 = 0.25;

fn next_prime(mut n: u32) -> u32 {
    n = n.max(2);
    loop {
        if (2..)
            .take_while(|d| d * d <= n)
            .all(|d| !n.is_multiple_of(d))
        {
            return n;
        }
        n += 1;
    }
}

/// Peak magnitude over frequency of a line's absorption filter
/// `g0·L1 + g1·(L2 − L1) + g2·(1 − L2)` (`dsp::BandSplit` with band gains `g`).
fn peak_gain(g: [f32; 3], sample_rate: u32) -> f32 {
    type C = (f64, f64);
    let mul = |a: C, b: C| -> C { (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0) };
    let div = |a: C, b: C| -> C {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    };
    let fs = f64::from(sample_rate);
    let gains = CROSSOVER_HZ.map(|hz| {
        let g = (std::f64::consts::PI * f64::from(hz).min(fs * 0.49) / fs).tan();
        g / (1.0 + g)
    });
    let [g0, g1, g2] = g.map(f64::from);
    let mut peak = 0.0_f64;
    for k in 0..256 {
        let hz = 10.0 * (0.499 * fs / 10.0).powf(f64::from(k) / 255.0);
        let w = std::f64::consts::TAU * hz / fs;
        let z = (w.cos(), -w.sin());
        // Bilinear one-pole G(1 + z⁻¹) / (1 − (1 − 2G) z⁻¹), squared for the cascade.
        let lp = |big: f64| {
            let h = div(
                (big * (1.0 + z.0), big * z.1),
                (1.0 - (1.0 - 2.0 * big) * z.0, -(1.0 - 2.0 * big) * z.1),
            );
            mul(h, h)
        };
        let (l1, l2) = (lp(gains[0]), lp(gains[1]));
        let re = g0 * l1.0 + g1 * (l2.0 - l1.0) + g2 * (1.0 - l2.0);
        let im = g0 * l1.1 + g1 * (l2.1 - l1.1) - g2 * l2.1;
        peak = peak.max((re * re + im * im).sqrt());
    }
    peak as f32
}

/// Scales a line's band gains so its filter never exceeds the largest band gain at any
/// frequency. Complementary bands of a critically damped crossover overshoot by up to
/// ~15%, which would otherwise push the loop gain above one (unbounded growth).
fn stable(g: [f32; 3], sample_rate: u32) -> [f32; 3] {
    let max = g.iter().fold(0.0_f32, |m, v| m.max(*v));
    let peak = peak_gain(g, sample_rate) * 1.001;
    if peak > max && peak > 0.0 {
        g.map(|v| v * max / peak)
    } else {
        g
    }
}

fn target(params: &ReverbParams, sample_rate: u32, speed_of_sound: f32) -> Target {
    let fs = sample_rate as f32;
    // Quantize room size to ~15% steps: interpolated probes then retarget decay and level
    // in place instead of constantly crossfading to freshly sized networks.
    let size = ((params.mean_free_path.max(0.1).ln() / 0.15).round() * 0.15).exp();
    let base = (size / speed_of_sound).clamp(0.004, MAX_DELAY);
    const SPREAD: [f32; LINES] = [0.62, 0.71, 0.80, 0.89, 1.0, 1.12, 1.26, 1.41];
    let mut lengths = [0_u32; LINES];
    let mut previous = 0;
    for (length, spread) in lengths.iter_mut().zip(SPREAD) {
        let seconds = (base * spread).clamp(0.007, MAX_DELAY * 0.95);
        *length = next_prime(((seconds * fs) as u32).max(previous + 1));
        previous = *length;
    }
    let decay: [[f32; 3]; LINES] = std::array::from_fn(|i| {
        std::array::from_fn(|b| {
            let t = params.rt60[b];
            if t <= 0.0 {
                0.0
            } else {
                10f32.powf(-3.0 * lengths[i] as f32 / (t * fs)).min(0.9999)
            }
        })
    });
    let decay = decay.map(|g| stable(g, sample_rate));
    // Stationary FDN energy gain is ~1/(1-g²); scale it to the requested room level.
    let mean = decay.iter().map(|g| g[1] * g[1]).sum::<f32>() / LINES as f32;
    let output = (params.level * (1.0 - mean)).max(0.0).sqrt() * params.wet;
    let predelay =
        ((1.5 * params.mean_free_path / speed_of_sound).clamp(0.005, MAX_DELAY * 0.8) * fs) as u32;
    let mut early = EarlyTarget::silent();
    early.count = params.early_count.min(MAX_TAPS);
    for (slot, tap) in early.taps.iter_mut().zip(&params.early[..early.count]) {
        let delay = (tap.delay.min(MAX_EARLY * 0.98) * fs) as u32;
        *slot = (delay.max(1), tap.gain);
    }
    // One-pole cutoff falls from 16 kHz (no damping) to 2 kHz (full damping).
    let cutoff =
        16000.0 * (1.0 - params.early_damping.clamp(0.0, 1.0)) + 2000.0 * params.early_damping;
    early.damping = 1.0 - (-std::f32::consts::TAU * cutoff.min(fs * 0.45) / fs).exp();
    Target {
        fdn: FdnTarget {
            lengths,
            decay,
            predelay,
            output,
        },
        early,
        wet: params.wet,
    }
}

/// Room reverb aux effect. Create with [`room_reverb`] and add with `AudioEngine::add_aux`.
pub struct RoomReverb {
    networks: [Fdn; 2],
    active: usize,
    fade: f32,
    fade_step: f32,
    early: Early,
    targets: Consumer<Target>,
    /// Early-reflection gain; each network carries its own output gain.
    wet: f32,
    wet_target: f32,
    /// A new room waiting for the idle network to fall quiet.
    pending: Option<FdnTarget>,
}

/// Control-path handle that retargets a [`RoomReverb`].
pub struct RoomReverbHandle {
    producer: Producer<Target>,
    sample_rate: u32,
    speed_of_sound: f32,
    last: Option<ReverbParams>,
}

/// Builds a reverb and its handle for an engine running at `sample_rate`.
pub fn room_reverb(
    sample_rate: u32,
    settings: &AcousticsSettings,
) -> (RoomReverb, RoomReverbHandle) {
    let (producer, targets) = RingBuffer::new(16);
    let reverb = RoomReverb {
        networks: [
            Fdn::new(sample_rate, MAX_DELAY),
            Fdn::new(sample_rate, MAX_DELAY),
        ],
        active: 0,
        fade: 1.0,
        fade_step: 1.0 / (ROOM_CROSSFADE_SECONDS * sample_rate as f32),
        early: Early::new((MAX_EARLY * sample_rate as f32) as usize + 1),
        targets,
        wet: 0.0,
        wet_target: 0.0,
        pending: None,
    };
    let handle = RoomReverbHandle {
        producer,
        sample_rate,
        speed_of_sound: settings.speed_of_sound,
        last: None,
    };
    (reverb, handle)
}

impl RoomReverbHandle {
    /// Retargets the reverb; unchanged parameters push nothing. Takes effect at the
    /// next quantum, so offline renders are deterministic when set between renders.
    pub fn set(&mut self, params: &ReverbParams) -> Result<()> {
        params.validate()?;
        if self.last.as_ref() == Some(params) {
            return Ok(());
        }
        let target = target(params, self.sample_rate, self.speed_of_sound);
        self.producer
            .push(target)
            .map_err(|_| AudioError::Capacity("reverb parameter queue"))?;
        self.last = Some(*params);
        Ok(())
    }
    /// The most recently applied parameters.
    pub fn current(&self) -> Option<&ReverbParams> {
        self.last.as_ref()
    }
}

/// Peak level below which a network counts as silent (-80 dBFS).
const QUIET: f32 = 1e-4;
/// Fade applied to a still-ringing network before it is reused for a new room.
const REUSE_FADE_SECONDS: f32 = 0.01;

impl RoomReverb {
    fn apply(&mut self, target: &Target) {
        self.early.configure(&target.early);
        self.wet_target = target.wet;
        let active = &mut self.networks[self.active];
        if active.lengths() == target.fdn.lengths || active.level() < QUIET {
            // Same room size (retarget in place) or nothing ringing yet.
            active.configure(&target.fdn);
            self.pending = None;
        } else {
            // A new room: the current network keeps its tail; switch once the idle one is quiet.
            self.pending = Some(target.fdn);
        }
    }

    fn switch(&mut self) {
        let Some(pending) = self.pending else { return };
        let other = 1 - self.active;
        if self.networks[other].level() < QUIET {
            self.networks[other].configure(&pending);
            self.active = other;
            self.fade = 0.0;
            self.pending = None;
        } else {
            self.networks[other].fade_out(REUSE_FADE_SECONDS);
        }
    }
}

impl AuxEffect for RoomReverb {
    fn process(&mut self, input: &[f32], out: &mut [Frame], _: u32) {
        while let Ok(target) = self.targets.pop() {
            self.apply(&target);
        }
        self.switch();
        for network in &mut self.networks {
            network.begin_block();
        }
        let step = (self.wet_target - self.wet) / input.len().max(1) as f32;
        let other = 1 - self.active;
        for (x, frame) in input.iter().zip(out.iter_mut()) {
            self.wet += step;
            let mut acc = [0.0_f32; 2];
            let fade = self.fade;
            self.networks[self.active].tick(x * fade, &mut acc);
            self.networks[other].tick(x * (1.0 - fade), &mut acc);
            let mut early = [0.0_f32; 2];
            self.early.tick(*x, &mut early);
            self.fade = (self.fade + self.fade_step).min(1.0);
            *frame = Frame::new(acc[0] + early[0] * self.wet, acc[1] + early[1] * self.wet);
        }
        self.wet = self.wet_target;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn lengths_are_distinct_primes_within_capacity() {
        let mut params = ReverbParams::silent();
        params.rt60 = [1.2, 1.0, 0.6];
        params.mean_free_path = 6.0;
        params.level = 0.3;
        let t = target(&params, 48000, 343.0);
        let max = (MAX_DELAY * 48000.0) as u32;
        for w in t.fdn.lengths.windows(2) {
            assert!(w[0] < w[1]);
        }
        for l in t.fdn.lengths {
            assert!(l <= max && (2..l).take_while(|d| d * d <= l).all(|d| l % d != 0));
        }
        assert!(t.fdn.decay.iter().all(|g| g[0] > g[2]));
    }
}
