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

/// Three-band splitter with exact reconstruction: `low + mid + high == input`.
///
/// Critically damped (Q = 0.5) low-passes at `low_hz` and `high_hz`, so bands never
/// overshoot. Band centers follow Steam Audio's 400 Hz / 2.5 kHz / 15 kHz convention.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BandSplit {
    low: Svf,
    high: Svf,
}

impl BandSplit {
    /// Ara's default 800 Hz / 8 kHz crossover.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_crossovers(800.0, 8000.0, sample_rate)
    }
    /// Custom crossover frequencies in Hz.
    pub fn with_crossovers(low_hz: f32, high_hz: f32, sample_rate: u32) -> Self {
        Self {
            low: Svf::new(SvfCoefficients::new(low_hz, 0.5, sample_rate)),
            high: Svf::new(SvfCoefficients::new(high_hz, 0.5, sample_rate)),
        }
    }
    /// Splits one sample into `[low, mid, high]`.
    #[inline]
    pub fn split(&mut self, input: f32) -> [f32; 3] {
        let low = self.low.low(input);
        let rest = input - low;
        let mid = self.high.low(rest);
        [low, mid, rest - mid]
    }
}
