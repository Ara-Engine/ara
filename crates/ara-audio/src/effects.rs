//! Kira DSP plus bounded metering, peak limiting and sidechain ducking.

use crate::{AudioError, Result};
use kira::{Frame, effect::Effect, info::Info};
use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

/// Reusable effects for bus builders; all buffers initialize before rendering.
pub use kira::effect::{
    compressor::CompressorBuilder,
    eq_filter::{EqFilterBuilder, EqFilterKind},
    filter::{FilterBuilder, FilterMode},
    reverb::ReverbBuilder,
};
/// Kira bus builder for ordered effect chains and send routes.
pub use kira::track::TrackBuilder;
/// Builder and route identity for shared effect sends, such as a wet-only room reverb.
pub use kira::track::{SendTrackBuilder, SendTrackId};
mod delay;
pub use delay::DelayBuilder;

/// Lock-free peak/energy meter. Copy the handle only while configuring a graph.
#[derive(Clone, Default)]
pub struct Meter(Arc<MeterState>);
#[derive(Default)]
struct MeterState {
    peak: AtomicU32,
    overloads: AtomicU64,
    block_peak: AtomicU32,
    clipped: [AtomicU64; 4],
    energy: AtomicU64,
    frames: AtomicU64,
    window_peak: AtomicU32,
}

/// Cumulative meter totals. Subtract two readings to measure a capture window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MeterReading {
    /// Sum of squared samples over both channels since the graph started.
    pub energy: f64,
    /// Frames observed since the graph started.
    pub frames: u64,
    /// Channel samples beyond full scale since the graph started.
    pub overloads: u64,
    /// Highest absolute sample since the graph started.
    pub peak: f32,
}

impl MeterReading {
    /// Stereo RMS in dBFS between `earlier` and this reading; None for silence.
    pub fn rms_dbfs_since(&self, earlier: &MeterReading) -> Option<f64> {
        let frames = self.frames.saturating_sub(earlier.frames);
        let energy = (self.energy - earlier.energy).max(0.0);
        (frames > 0 && energy > 0.0).then(|| 10.0 * (energy / (frames * 2) as f64).log10())
    }
}

impl Meter {
    /// Highest absolute sample observed before the final limiter.
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.0.peak.load(Ordering::Relaxed))
    }
    /// Count of channel samples exceeding digital full scale, before limiting.
    pub fn overloads(&self) -> u64 {
        self.0.overloads.load(Ordering::Relaxed)
    }
    /// Highest absolute sample in the most recent quantum.
    pub fn block_peak(&self) -> f32 {
        f32::from_bits(self.0.block_peak.load(Ordering::Relaxed))
    }
    /// Cumulative totals for windowed RMS (see `MeterReading::rms_dbfs_since`).
    pub fn reading(&self) -> MeterReading {
        MeterReading {
            energy: f64::from_bits(self.0.energy.load(Ordering::Relaxed)),
            frames: self.0.frames.load(Ordering::Relaxed),
            overloads: self.overloads(),
            peak: self.peak(),
        }
    }
    /// Highest absolute sample since the previous call; resets the window (control path).
    pub fn take_window_peak(&self) -> f32 {
        f32::from_bits(self.0.window_peak.swap(0, Ordering::Relaxed))
    }
    pub(crate) fn effect(&self) -> Box<dyn Effect> {
        Box::new(MeterEffect(self.clone()))
    }
    pub(crate) fn clipped_mask(&self) -> [u64; 4] {
        std::array::from_fn(|i| self.0.clipped[i].load(Ordering::Relaxed))
    }
    /// Records one block. Called only by the renderer thread that owns this meter's track.
    pub(crate) fn observe(&self, input: &[Frame]) {
        let mut peak = 0.0_f32;
        let mut overloads = 0;
        let mut clipped = [0_u64; 4];
        let mut energy = 0.0_f32;
        for (i, f) in input.iter().enumerate() {
            peak = peak.max(f.left.abs()).max(f.right.abs());
            energy += f.left * f.left + f.right * f.right;
            overloads += u64::from(f.left.abs() > 1.0) + u64::from(f.right.abs() > 1.0);
            for (c, s) in [f.left, f.right].into_iter().enumerate() {
                if s.abs() > 1.0 || !s.is_finite() {
                    let index = i * 2 + c;
                    if index < 256 {
                        clipped[index / 64] |= 1 << (index % 64);
                    }
                }
            }
        }
        let state = &self.0;
        for (dst, value) in state.clipped.iter().zip(clipped) {
            dst.store(value, Ordering::Relaxed);
        }
        state.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        state
            .window_peak
            .fetch_max(peak.to_bits(), Ordering::Relaxed);
        state.block_peak.store(peak.to_bits(), Ordering::Relaxed);
        state.overloads.fetch_add(overloads, Ordering::Relaxed);
        // Single writer: load/add/store keeps the f64 total exact without a CAS loop.
        if energy.is_finite() {
            let total = f64::from_bits(state.energy.load(Ordering::Relaxed)) + f64::from(energy);
            state.energy.store(total.to_bits(), Ordering::Relaxed);
        }
        state
            .frames
            .fetch_add(input.len() as u64, Ordering::Relaxed);
    }
}

struct MeterEffect(Meter);
impl Effect for MeterEffect {
    fn process(&mut self, input: &mut [Frame], _: f64, _: &Info) {
        self.0.observe(input);
    }
}

/// Stereo-linked sample peak limiter with instantaneous attack and exponential release.
/// This adds no lookahead latency; capture also reports intersample true peaks separately.
pub struct PeakLimiter {
    ceiling: f32,
    release: f64,
    gain: f32,
}
impl PeakLimiter {
    /// Builds a limiter. The ceiling must be in (0,1], release in (0,10] seconds.
    pub fn new(ceiling: f32, release_seconds: f64) -> Result<Self> {
        if !ceiling.is_finite()
            || ceiling <= 0.0
            || ceiling > 1.0
            || !release_seconds.is_finite()
            || release_seconds <= 0.0
            || release_seconds > 10.0
        {
            return Err(AudioError::Invalid("limiter"));
        }
        Ok(Self {
            ceiling,
            release: release_seconds,
            gain: 1.0,
        })
    }
}
impl Effect for PeakLimiter {
    fn process(&mut self, input: &mut [Frame], dt: f64, _: &Info) {
        let release = (-dt / self.release).exp() as f32;
        for frame in input {
            if !frame.left.is_finite() || !frame.right.is_finite() {
                *frame = Frame::ZERO;
                continue;
            }
            let peak = frame.left.abs().max(frame.right.abs());
            let required = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            self.gain = required.min(1.0 - (1.0 - self.gain) * release);
            *frame *= self.gain;
        }
    }
}

/// Sidechain envelope settings. The source meter is read from the previous/current block.
pub struct Ducking {
    /// Meter of the bus that triggers this duck.
    pub source: Meter,
    /// Trigger level in linear amplitude.
    pub threshold: f32,
    /// Target amplitude while the source exceeds the threshold.
    pub gain: f32,
    /// Attack in seconds, strictly positive.
    pub attack: f64,
    /// Release in seconds, strictly positive.
    pub release: f64,
}
impl Ducking {
    /// Prepares a no-allocation bus effect; allow one quantum of sidechain latency.
    pub fn build(self) -> Result<Box<dyn Effect>> {
        if !self.threshold.is_finite()
            || self.threshold < 0.0
            || !self.gain.is_finite()
            || !(0.0..=1.0).contains(&self.gain)
            || !self.attack.is_finite()
            || self.attack <= 0.0
            || !self.release.is_finite()
            || self.release <= 0.0
        {
            return Err(AudioError::Invalid("ducking"));
        }
        Ok(Box::new(DuckEffect {
            settings: self,
            envelope: 1.0,
        }))
    }
}
struct DuckEffect {
    settings: Ducking,
    envelope: f32,
}
impl Effect for DuckEffect {
    fn process(&mut self, input: &mut [Frame], dt: f64, _: &Info) {
        let peak = f32::from_bits(self.settings.source.0.block_peak.load(Ordering::Relaxed));
        let target = if peak > self.settings.threshold {
            self.settings.gain
        } else {
            1.0
        };
        let duration = if target < self.envelope {
            self.settings.attack
        } else {
            self.settings.release
        };
        let coefficient = (-dt / duration).exp() as f32;
        for frame in input {
            self.envelope = target + (self.envelope - target) * coefficient;
            *frame *= self.envelope;
        }
    }
}
