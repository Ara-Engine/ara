//! Eight-line feedback delay network with three-band decay per line.
//!
//! Lines recirculate through a normalized Hadamard matrix (lossless and dense). Each
//! line's three bands decay by `10^(-3·len / (RT60·fs))` per pass, so low, mid and high
//! reverberation times are independent. All buffers are allocated at construction.

use ara_audio::dsp::BandSplit;

pub(crate) const LINES: usize = 8;

/// Precomputed configuration for one room, produced on the control path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FdnTarget {
    /// Delay lengths in samples (distinct primes).
    pub lengths: [u32; LINES],
    /// Per-line low/mid/high gain per recirculation.
    pub decay: [[f32; 3]; LINES],
    /// Predelay in samples.
    pub predelay: u32,
    /// Output scale for the requested reverberant energy, including the wet gain.
    pub output: f32,
}

struct Line {
    buffer: Box<[f32]>,
    cursor: usize,
    length: usize,
}

impl Line {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity.max(1)].into_boxed_slice(),
            cursor: 0,
            length: 1,
        }
    }
    #[inline]
    fn read(&self) -> f32 {
        self.buffer[self.cursor]
    }
    #[inline]
    fn write(&mut self, value: f32) {
        self.buffer[self.cursor] = value;
        self.cursor += 1;
        if self.cursor >= self.length {
            self.cursor = 0;
        }
    }
    fn set_length(&mut self, length: usize) {
        self.length = length.clamp(1, self.buffer.len());
        self.cursor %= self.length;
    }
    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.cursor = 0;
    }
}

pub(crate) struct Fdn {
    lines: [Line; LINES],
    filters: [BandSplit; LINES],
    decay: [[f32; 3]; LINES],
    predelay: Line,
    /// Current and target output gain (room level × wet), smoothed per sample.
    output: f32,
    output_target: f32,
    smoothing: f32,
    level: f32,
    sample_rate: u32,
}

/// In-place fast Walsh–Hadamard transform of 8 values, normalized to stay lossless.
#[inline]
fn hadamard(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        for i in (0..LINES).step_by(h * 2) {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
        }
        h *= 2;
    }
    let norm = 1.0 / (LINES as f32).sqrt();
    for x in v.iter_mut() {
        *x *= norm;
    }
}

/// Output sign patterns decorrelate left and right.
const LEFT: [f32; LINES] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];
const RIGHT: [f32; LINES] = [1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
const INPUT: [f32; LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
/// 1/sqrt(8): input is spread over and output summed from eight lines at unit total energy.
const NORM: f32 = 0.353_553_38;

/// Output gain smoothing for in-place retargets (level or wet changes).
const OUTPUT_SECONDS: f32 = 0.05;

impl Fdn {
    /// Allocates lines for up to `max_seconds` of delay and predelay.
    pub(crate) fn new(sample_rate: u32, max_seconds: f32) -> Self {
        let capacity = (max_seconds * sample_rate as f32).ceil() as usize + 1;
        let mut fdn = Self {
            lines: std::array::from_fn(|_| Line::new(capacity)),
            filters: [BandSplit::new(sample_rate); LINES],
            decay: [[0.0; 3]; LINES],
            predelay: Line::new(capacity),
            output: 0.0,
            output_target: 0.0,
            smoothing: 0.0,
            level: 0.0,
            sample_rate,
        };
        fdn.smooth_over(OUTPUT_SECONDS);
        fdn
    }

    fn smooth_over(&mut self, seconds: f32) {
        self.smoothing = 1.0 - (-1.0 / (seconds * self.sample_rate as f32)).exp();
    }

    pub(crate) fn lengths(&self) -> [u32; LINES] {
        std::array::from_fn(|i| self.lines[i].length as u32)
    }

    /// Peak absolute output since the last `begin_block`.
    pub(crate) fn level(&self) -> f32 {
        self.level
    }

    /// Starts a block's level measurement.
    pub(crate) fn begin_block(&mut self) {
        self.level = 0.0;
    }

    /// Fades the output to silence over `seconds` (before reusing a ringing network).
    pub(crate) fn fade_out(&mut self, seconds: f32) {
        self.output_target = 0.0;
        self.smooth_over(seconds);
    }

    /// Applies a target. New lengths clear memory and take the output gain at once (the
    /// network is silent); same lengths retarget decay in place and glide the output.
    pub(crate) fn configure(&mut self, target: &FdnTarget) {
        if self.lengths() != target.lengths {
            for (line, &length) in self.lines.iter_mut().zip(&target.lengths) {
                line.clear();
                line.set_length(length as usize);
            }
            for filter in &mut self.filters {
                *filter = BandSplit::new(self.sample_rate);
            }
            self.predelay.clear();
            self.output = target.output;
        }
        self.predelay.set_length(target.predelay.max(1) as usize);
        self.decay = target.decay;
        self.output_target = target.output;
        self.smooth_over(OUTPUT_SECONDS);
    }

    /// Processes one sample of mono input into a stereo pair (added to `out`).
    #[inline]
    pub(crate) fn tick(&mut self, input: f32, out: &mut [f32; 2]) {
        let delayed = self.predelay.read() * NORM;
        self.predelay.write(input);
        let mut taps = [0.0; LINES];
        let (mut left, mut right) = (0.0, 0.0);
        for i in 0..LINES {
            let y = self.lines[i].read();
            left += y * LEFT[i];
            right += y * RIGHT[i];
            let [lo, mid, hi] = self.filters[i].split(y);
            let g = self.decay[i];
            taps[i] = lo * g[0] + mid * g[1] + hi * g[2];
        }
        hadamard(&mut taps);
        for i in 0..LINES {
            self.lines[i].write(taps[i] + delayed * INPUT[i]);
        }
        self.output += (self.output_target - self.output) * self.smoothing;
        let (l, r) = (left * self.output * NORM, right * self.output * NORM);
        self.level = self.level.max(l.abs()).max(r.abs());
        out[0] += l;
        out[1] += r;
    }
}
