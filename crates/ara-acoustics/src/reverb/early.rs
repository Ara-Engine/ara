//! Geometric early reflections as a fixed multi-tap delay, filtered by wall absorption.

/// Maximum early reflection taps per room (6 first-order + 12 second-order images).
pub const MAX_TAPS: usize = 18;

/// Taps in samples, produced on the control path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EarlyTarget {
    pub taps: [(u32, [f32; 2]); MAX_TAPS],
    pub count: usize,
    /// One-pole low-pass coefficient modelling high-band wall absorption.
    pub damping: f32,
}

impl EarlyTarget {
    pub(crate) fn silent() -> Self {
        Self {
            taps: [(1, [0.0; 2]); MAX_TAPS],
            count: 0,
            damping: 1.0,
        }
    }
}

pub(crate) struct Early {
    history: Box<[f32]>,
    cursor: usize,
    target: EarlyTarget,
    low: [f32; 2],
}

impl Early {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            history: vec![0.0; capacity.max(2)].into_boxed_slice(),
            cursor: 0,
            target: EarlyTarget::silent(),
            low: [0.0; 2],
        }
    }

    pub(crate) fn configure(&mut self, target: &EarlyTarget) {
        self.target = *target;
        let max = self.history.len() as u32 - 1;
        for tap in &mut self.target.taps[..target.count.min(MAX_TAPS)] {
            tap.0 = tap.0.clamp(1, max);
        }
    }

    #[inline]
    pub(crate) fn tick(&mut self, input: f32, out: &mut [f32; 2]) {
        let len = self.history.len();
        self.history[self.cursor] = input;
        let mut sum = [0.0; 2];
        for &(delay, gain) in &self.target.taps[..self.target.count] {
            let mut index = self.cursor + len - delay as usize;
            if index >= len {
                index -= len;
            }
            let s = self.history[index];
            sum[0] += s * gain[0];
            sum[1] += s * gain[1];
        }
        self.cursor += 1;
        if self.cursor == len {
            self.cursor = 0;
        }
        for c in 0..2 {
            self.low[c] += (sum[c] - self.low[c]) * self.target.damping;
            out[c] += self.low[c];
        }
    }
}
