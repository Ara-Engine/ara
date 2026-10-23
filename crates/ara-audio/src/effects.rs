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

/// Lock-free peak meter. Copy the handle only while configuring a graph.
#[derive(Clone, Default)]
pub struct Meter(Arc<MeterState>);
#[derive(Default)]
struct MeterState {
    peak: AtomicU32,
    overloads: AtomicU64,
    block_peak: AtomicU32,
    clipped: [AtomicU64; 4],
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
    pub(crate) fn effect(&self) -> Box<dyn Effect> {
        Box::new(MeterEffect(self.clone()))
    }
    pub(crate) fn clipped_mask(&self) -> [u64; 4] {
        std::array::from_fn(|i| self.0.clipped[i].load(Ordering::Relaxed))
    }
}

struct MeterEffect(Meter);
impl Effect for MeterEffect {
    fn process(&mut self, input: &mut [Frame], _: f64, _: &Info) {
        let mut peak = 0.0_f32;
        let mut overloads = 0;
        let mut clipped = [0_u64; 4];
        for (i, f) in input.iter().enumerate() {
            peak = peak.max(f.left.abs()).max(f.right.abs());
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
        for (dst, value) in self.0.0.clipped.iter().zip(clipped) {
            dst.store(value, Ordering::Relaxed);
        }
        self.0.0.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        self.0.0.block_peak.store(peak.to_bits(), Ordering::Relaxed);
        self.0.0.overloads.fetch_add(overloads, Ordering::Relaxed);
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
