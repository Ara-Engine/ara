//! Small allocation-free DSP primitives shared by spatializers, acoustics and audio graphs.

/// Topology-preserving-transform state variable filter (Zavalishin/Cytomic form).
///
/// Stable under per-sample coefficient changes, so cutoff/Q can be modulated. One `tick`
/// returns low/band/high outputs; `low + k·band + high` reconstructs the input exactly.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    coefficients: SvfCoefficients,
}

/// Precomputed SVF coefficients for one cutoff, resonance and sample rate.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SvfCoefficients {
    a1: f32,
    a2: f32,
    a3: f32,
    /// Damping, 1/Q.
    pub k: f32,
}

/// Simultaneous SVF outputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfOut {
    /// Low-pass output.
    pub low: f32,
    /// Band-pass output (unity peak gain is `band × k`).
    pub band: f32,
    /// High-pass output.
    pub high: f32,
}

impl SvfCoefficients {
    /// Coefficients for `cutoff` Hz (clamped below Nyquist) and quality `q` (> 0).
    pub fn new(cutoff: f32, q: f32, sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let cutoff = cutoff.clamp(1.0, rate * 0.49);
        let g = (std::f32::consts::PI * cutoff / rate).tan();
        let k = 1.0 / q.max(0.05);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            k,
        }
    }
}

impl Svf {
    /// A filter at rest with the given coefficients.
    pub fn new(coefficients: SvfCoefficients) -> Self {
        Self {
            coefficients,
            ..Self::default()
        }
    }
    /// Replaces coefficients without resetting state (smooth modulation).
    pub fn set(&mut self, coefficients: SvfCoefficients) {
        self.coefficients = coefficients;
    }
    /// Clears filter memory.
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
    /// Processes one sample.
    #[inline]
    pub fn tick(&mut self, input: f32) -> SvfOut {
        let c = self.coefficients;
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut {
            low: v2,
            band: v1,
            high: input - c.k * v1 - v2,
        }
    }
    /// Low-pass only.
    #[inline]
    pub fn low(&mut self, input: f32) -> f32 {
        self.tick(input).low
    }
}

/// Bilinear (topology-preserving) one-pole low-pass. The state recursion is one
/// multiply and one add per sample, so several run in parallel at full throughput.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OnePole {
    state: f32,
    /// G = g / (1 + g), g = tan(π·cutoff / rate).
    gain: f32,
}

impl OnePole {
    /// A filter at rest for `cutoff` Hz (clamped below Nyquist).
    pub fn new(cutoff: f32, sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let g = (std::f32::consts::PI * cutoff.clamp(1.0, rate * 0.49) / rate).tan();
        Self {
            state: 0.0,
            gain: g / (1.0 + g),
        }
    }
    /// Processes one sample.
    #[inline]
    pub fn tick(&mut self, input: f32) -> f32 {
        let g = self.gain;
        let y = self.state * (1.0 - g) + input * g;
        self.state = self.state * (1.0 - 2.0 * g) + input * (2.0 * g);
        y
    }
}

/// A critically damped (Q = 0.5) second-order low-pass: two cascaded bilinear one-poles,
/// the same response as a Q = 0.5 TPT state-variable filter at a third of its latency.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Lowpass2([OnePole; 2]);

impl Lowpass2 {
    /// A filter at rest for `cutoff` Hz.
    pub fn new(cutoff: f32, sample_rate: u32) -> Self {
        Self([OnePole::new(cutoff, sample_rate); 2])
    }
    /// Processes one sample.
    #[inline]
    pub fn tick(&mut self, input: f32) -> f32 {
        let first = self.0[0].tick(input);
        self.0[1].tick(first)
    }
}

/// Default low/mid and mid/high crossover frequencies in Hz.
pub const CROSSOVER_HZ: [f32; 2] = [800.0, 8000.0];

/// Three-band splitter with exact reconstruction: `low + mid + high == input`.
///
/// Two critically damped low-passes at `low_hz` and `high_hz` run in parallel on the
/// input: `low = LP(low_hz)`, `mid = LP(high_hz) − low`, `high = input − LP(high_hz)`,
/// so bands never overshoot. Band centers follow Steam Audio's 400 Hz / 2.5 kHz / 15 kHz.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BandSplit {
    low: Lowpass2,
    high: Lowpass2,
}

impl BandSplit {
    /// Ara's default crossover (`CROSSOVER_HZ`).
    pub fn new(sample_rate: u32) -> Self {
        Self::with_crossovers(CROSSOVER_HZ[0], CROSSOVER_HZ[1], sample_rate)
    }
    /// Custom crossover frequencies in Hz.
    pub fn with_crossovers(low_hz: f32, high_hz: f32, sample_rate: u32) -> Self {
        Self {
            low: Lowpass2::new(low_hz, sample_rate),
            high: Lowpass2::new(high_hz, sample_rate),
        }
    }
    /// The two crossover low-passes `[LP(low_hz), LP(high_hz)]` of one sample. With band
    /// gains `g`, the output is `(g0−g1)·lp0 + (g1−g2)·lp1 + g2·input`.
    #[inline]
    pub fn lows(&mut self, input: f32) -> [f32; 2] {
        [self.low.tick(input), self.high.tick(input)]
    }
    /// Splits one sample into `[low, mid, high]`.
    #[inline]
    pub fn split(&mut self, input: f32) -> [f32; 3] {
        let [low, upper] = self.lows(input);
        [low, upper - low, input - upper]
    }
}
