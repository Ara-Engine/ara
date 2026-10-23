use crate::{Result, SimError};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// An injectable monotonic host clock; simulation systems never read this clock.
pub trait Clock {
    /// Host time relative to an arbitrary origin.
    fn now(&self) -> Duration;
}

/// Fixed simulation rate and maximum work per presentation frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickConfig {
    /// Ticks per second; must be positive and at most 1,000,000.
    pub hz: u32,
    /// Maximum catch-up ticks in one frame; excess whole ticks are discarded.
    pub max_catch_up: u32,
}

impl Default for TickConfig {
    fn default() -> Self {
        Self {
            hz: 60,
            max_catch_up: 8,
        }
    }
}

/// Presentation scheduling result; simulation always receives the same fixed dt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameAdvance {
    /// Number of fixed ticks scheduled/executed.
    pub ticks: u32,
    /// Fraction toward the next tick, in [0, 1), for presentation interpolation.
    pub alpha: f64,
    /// Whole catch-up ticks discarded to bound work.
    pub dropped_ticks: u64,
}

/// Restorable accumulator with an explicit host-time boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixedTick {
    config: TickConfig,
    phase: f64,
    timescale: f64,
    last_clock: Option<Duration>,
}

impl FixedTick {
    /// Creates a bounded fixed loop. No process clock is accessed.
    pub fn new(config: TickConfig) -> Result<Self> {
        if config.hz == 0 || config.hz > 1_000_000 || config.max_catch_up == 0 {
            return Err(SimError("invalid fixed tick configuration".into()));
        }
        Ok(Self {
            config,
            phase: 0.0,
            timescale: 1.0,
            last_clock: None,
        })
    }

    /// Current fixed-rate configuration.
    pub fn config(&self) -> TickConfig {
        self.config
    }

    /// Presentation interpolation phase.
    pub fn alpha(&self) -> f64 {
        self.phase
    }

    /// Changes host-time speed in [0, 1000]; fixed dt is unchanged.
    pub fn set_timescale(&mut self, timescale: f64) -> Result<()> {
        if !timescale.is_finite() || !(0.0..=1000.0).contains(&timescale) {
            return Err(SimError("timescale must be finite and in [0, 1000]".into()));
        }
        self.timescale = timescale;
        Ok(())
    }

    /// Accumulates host elapsed time, preserving only the fractional excess phase.
    pub fn advance(&mut self, elapsed: Duration) -> FrameAdvance {
        let total = self.phase + elapsed.as_secs_f64() * self.timescale * f64::from(self.config.hz);
        let whole = total.floor() as u64;
        self.phase = total.fract();
        let ticks = whole.min(u64::from(self.config.max_catch_up)) as u32;
        FrameAdvance {
            ticks,
            alpha: self.phase,
            dropped_ticks: whole - u64::from(ticks),
        }
    }

    /// Samples an injected monotonic clock. A backward jump is rejected without mutation.
    pub fn poll(&mut self, clock: &impl Clock) -> Result<FrameAdvance> {
        let now = clock.now();
        let elapsed = match self.last_clock {
            Some(last) => now
                .checked_sub(last)
                .ok_or_else(|| SimError("clock moved backwards".into()))?,
            None => Duration::ZERO,
        };
        self.last_clock = Some(now);
        Ok(self.advance(elapsed))
    }

    /// Rebases a paused/restored loop so host time spent paused is never caught up.
    pub fn rebase_clock(&mut self) {
        self.last_clock = None;
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let mut checked = Self::new(self.config)?;
        checked.set_timescale(self.timescale)?;
        if !self.phase.is_finite() || !(0.0..1.0).contains(&self.phase) {
            return Err(SimError("invalid fixed tick phase".into()));
        }
        Ok(())
    }
}
