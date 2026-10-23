//! Per-node DSP state and block processing. Nothing here allocates after `State::new`.
//!
//! Buffers are audio-rate. Triggers are pulse buffers (1.0 on an event sample). Clip
//! values are clip-table slot indices carried as numbers.

use crate::{compile::Instr, library::Op};
use ara_audio::{
    Clip, QUANTUM,
    dsp::{Svf, SvfCoefficients},
};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

pub(crate) type Buf = [f32; QUANTUM];

/// SplitMix64: deterministic per-node randomness.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in [-1, 1).
    fn bipolar(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Grain {
    pos: f64,
    step: f64,
    age: u32,
    len: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Cached biquad (RBJ) coefficients and direct-form-I history.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Biquad {
    key: [f32; 3],
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn design(&mut self, op: Op, freq: f32, q: f32, gain: f32, rate: f32) {
        let key = [freq, q, gain];
        if key == self.key && self.b != [0.0; 3] {
            return;
        }
        self.key = key;
        let w = TAU * freq.clamp(10.0, rate * 0.49) / rate;
        let (sin, cos) = w.sin_cos();
        let amp = 10f32.powf(gain / 40.0);
        let (b, a) = match op {
            Op::Peak => {
                let alpha = sin / (2.0 * q.max(0.05));
                (
                    [1.0 + alpha * amp, -2.0 * cos, 1.0 - alpha * amp],
                    [1.0 + alpha / amp, -2.0 * cos, 1.0 - alpha / amp],
                )
            }
            _ => {
                // Shelves with slope 1.
                let alpha = sin / 2.0 * 2f32.sqrt();
                let s = 2.0 * amp.sqrt() * alpha;
                let sign = if op == Op::LowShelf { 1.0 } else { -1.0 };
                let (p, m) = (amp + 1.0, amp - 1.0);
                (
                    [
                        amp * (p - sign * m * cos + s),
                        sign * 2.0 * amp * (m - sign * p * cos),
                        amp * (p - sign * m * cos - s),
                    ],
                    [
                        p + sign * m * cos + s,
                        -sign * 2.0 * (m + sign * p * cos),
                        p + sign * m * cos - s,
                    ],
                )
            }
        };
        let a0 = a[0];
        self.b = b.map(|v| v / a0);
        self.a = [a[1] / a0, a[2] / a0];
    }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

pub(crate) enum State {
    None,
    Phase(f32),
    Noise {
        rng: Rng,
        b: [f32; 3],
    },
    Smooth {
        rng: Rng,
        from: f32,
        to: f32,
        t: f32,
    },
    Envelope {
        level: f32,
        stage: Stage,
    },
    Filter {
        svf: Svf,
        key: [f32; 2],
    },
    Biquad(Biquad),
    OnePole(f32),
    Sample {
        pos: f64,
        playing: bool,
        clip: usize,
        auto: bool,
    },
    Granular {
        rng: Rng,
        grains: Box<[Grain; 32]>,
        countdown: f64,
    },
    Random {
        rng: Rng,
        value: f32,
        started: bool,
    },
    Pick {
        rng: Rng,
        index: usize,
        started: bool,
    },
    Counter {
        remaining: f64,
    },
    Fired(bool),
    Pending {
        at: [f64; 8],
        count: usize,
    },
    Count(u32),
    Step {
        step: usize,
        started: bool,
    },
    Previous(f32),
    Echo {
        buffer: Box<[f32]>,
        cursor: usize,
    },
}

impl State {
    /// Allocates the node's state; `seed` mixes the voice seed with the node's literal seed.
    pub(crate) fn new(instr: &Instr, seed: u64, sample_rate: u32) -> Self {
        let rng = Rng::new(seed ^ (instr.seed as i64 as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
        match instr.op {
            Op::Sine | Op::Saw | Op::Square | Op::Triangle | Op::Phasor => State::Phase(0.0),
            Op::White | Op::Pink | Op::Brown => State::Noise { rng, b: [0.0; 3] },
            Op::Smooth => {
                let mut rng = rng;
                let from = rng.bipolar();
                let to = rng.bipolar();
                State::Smooth {
                    rng,
                    from,
                    to,
                    t: 0.0,
                }
            }
            Op::Adsr | Op::Ad => State::Envelope {
                level: 0.0,
                stage: Stage::Idle,
            },
            Op::Lowpass | Op::Highpass | Op::Bandpass | Op::Notch => State::Filter {
                svf: Svf::default(),
                key: [f32::NAN; 2],
            },
            Op::Peak | Op::LowShelf | Op::HighShelf => State::Biquad(Biquad::default()),
            Op::OnePole | Op::Slew => State::OnePole(0.0),
            Op::Sample => State::Sample {
                pos: 0.0,
                playing: false,
                clip: 0,
                auto: instr.auto_start,
            },
            Op::Granular => State::Granular {
                rng,
                grains: Box::new([Grain::default(); 32]),
                countdown: 0.0,
            },
            Op::RandomRange | Op::Chance => State::Random {
                rng,
                value: 0.0,
                started: false,
            },
            Op::RandomPick => State::Pick {
                rng,
                index: 0,
                started: false,
            },
            Op::Metro => State::Counter { remaining: 0.0 },
            Op::Start => State::Fired(false),
            Op::Delay => State::Pending {
                at: [0.0; 8],
                count: 0,
            },
            Op::Every => State::Count(0),
            Op::Seq => State::Step {
                step: 0,
                started: false,
            },
            Op::Threshold => State::Previous(f32::INFINITY),
            Op::Echo => State::Echo {
                buffer: vec![0.0; (instr.capacity * sample_rate as f32).ceil() as usize + 2]
                    .into_boxed_slice(),
                cursor: 0,
            },
            _ => State::None,
        }
    }
}

/// Everything an op reads besides its own state.
pub(crate) struct Ctx<'a> {
    pub rate: f32,
    pub clips: &'a [Clip],
    pub n: usize,
}

/// The value if every sample of the block is identical (lets ops hoist math).
#[inline]
fn constant(buf: &Buf, n: usize) -> Option<f32> {
    let first = buf[0];
    buf[..n].iter().all(|v| *v == first).then_some(first)
}

#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Rational tanh approximation (deterministic, |error| < 0.03 on [-3, 3], saturating beyond).
#[inline]
fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

fn frame(clip: &Clip, pos: f64) -> (f32, f32) {
    let frames = clip.frames();
    let i = pos.floor();
    let t = (pos - i) as f32;
    let i = i as usize;
    let a = frames.get(i).copied().unwrap_or_default();
    let b = frames.get(i + 1).copied().unwrap_or(a);
    (
        a.left + (b.left - a.left) * t,
        a.right + (b.right - a.right) * t,
    )
}

/// Runs one instruction over `ctx.n` frames. `inputs` holds every buffer below the
/// instruction's first output; `outputs` are its own output buffers.
pub(crate) fn run(
    instr: &Instr,
    state: &mut State,
    inputs: &[Buf],
    outputs: &mut [Buf],
    ctx: &Ctx<'_>,
) {
    let n = ctx.n;
    let input = |k: usize| -> &Buf { &inputs[usize::from(instr.inputs[k])] };
    let fs = ctx.rate;
    match (instr.op, state) {
        (Op::Sine | Op::Saw | Op::Square | Op::Triangle | Op::Phasor, State::Phase(phase)) => {
            let freq = input(0);
            let width = if instr.op == Op::Square {
                Some(input(1))
            } else {
                None
            };
            let phase_offset = if instr.op == Op::Sine {
                Some(input(1))
            } else {
                None
            };
            let out = &mut outputs[0];
            for i in 0..n {
                let dt = (freq[i] / fs).clamp(-0.5, 0.5);
                let p = *phase;
                out[i] = match instr.op {
                    Op::Sine => ((p + phase_offset.map_or(0.0, |o| o[i])) * TAU).sin(),
                    Op::Saw => 2.0 * p - 1.0 - poly_blep(p, dt.abs().max(1e-6)),
                    Op::Square => {
                        let w = width.map_or(0.5, |w| w[i]).clamp(0.01, 0.99);
                        let dt = dt.abs().max(1e-6);
                        let mut v = if p < w { 1.0 } else { -1.0 };
                        v += poly_blep(p, dt);
                        v - poly_blep((p - w).rem_euclid(1.0), dt)
                    }
                    Op::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
                    _ => p,
                };
                *phase = (p + dt).rem_euclid(1.0);
            }
        }
        (Op::White | Op::Pink | Op::Brown, State::Noise { rng, b }) => {
            let out = &mut outputs[0];
            for v in &mut out[..n] {
                let white = rng.bipolar();
                *v = match instr.op {
                    Op::White => white,
                    Op::Pink => {
                        // Paul Kellet's economy pink filter.
                        b[0] = 0.99765 * b[0] + white * 0.099_046;
                        b[1] = 0.963 * b[1] + white * 0.296_516_4;
                        b[2] = 0.57 * b[2] + white * 1.052_691_3;
                        (b[0] + b[1] + b[2] + white * 0.1848) * 0.2
                    }
                    _ => {
                        b[0] = (b[0] * 0.998 + white * 0.02).clamp(-1.0, 1.0);
                        b[0] * 3.5
                    }
                };
            }
        }
        (Op::Smooth, State::Smooth { rng, from, to, t }) => {
            let rate = input(0);
            for i in 0..n {
                let s = 0.5 - 0.5 * (*t * PI).cos();
                outputs[0][i] = *from + (*to - *from) * s;
                // At most one new target per sample: bounded work for any (even infinite) rate.
                let step = rate[i] / fs;
                *t += if step > 0.0 { step.min(1.0) } else { 0.0 };
                if *t >= 1.0 {
                    *t -= 1.0;
                    *from = *to;
                    *to = rng.bipolar();
                }
            }
        }
        (Op::Adsr, State::Envelope { level, stage }) => {
            let (gate, attack, decay, sustain, release) =
                (input(0), input(1), input(2), input(3), input(4));
            let (out, done) = outputs.split_at_mut(1);
            for i in 0..n {
                done[0][i] = 0.0;
                let on = gate[i] > 0.5;
                if on && matches!(stage, Stage::Idle | Stage::Release) {
                    *stage = Stage::Attack;
                } else if !on && matches!(stage, Stage::Attack | Stage::Decay | Stage::Sustain) {
                    *stage = Stage::Release;
                }
                let s = sustain[i].clamp(0.0, 1.0);
                match stage {
                    Stage::Attack => {
                        *level += 1.0 / (attack[i].max(1e-4) * fs);
                        if *level >= 1.0 {
                            *level = 1.0;
                            *stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => {
                        *level -= (1.0 - s) / (decay[i].max(1e-4) * fs);
                        if *level <= s {
                            *level = s;
                            *stage = Stage::Sustain;
                        }
                    }
                    Stage::Sustain => *level = s,
                    Stage::Release => {
                        *level -= 1.0 / (release[i].max(1e-4) * fs);
                        if *level <= 0.0 {
                            *level = 0.0;
                            *stage = Stage::Idle;
                            done[0][i] = 1.0;
                        }
                    }
                    Stage::Idle => {}
                }
                out[0][i] = *level;
            }
        }
        (Op::Ad, State::Envelope { level, stage }) => {
            let (trigger, attack, decay) = (input(0), input(1), input(2));
            let (out, done) = outputs.split_at_mut(1);
            for i in 0..n {
                done[0][i] = 0.0;
                if trigger[i] > 0.5 {
                    *stage = Stage::Attack;
                }
                match stage {
                    Stage::Attack => {
                        *level += 1.0 / (attack[i].max(1e-4) * fs);
                        if *level >= 1.0 {
                            *level = 1.0;
                            *stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => {
                        *level -= 1.0 / (decay[i].max(1e-4) * fs);
                        if *level <= 0.0 {
                            *level = 0.0;
                            *stage = Stage::Idle;
                            done[0][i] = 1.0;
                        }
                    }
                    _ => {}
                }
                out[0][i] = *level;
            }
        }
        (Op::Lowpass | Op::Highpass | Op::Bandpass | Op::Notch, State::Filter { svf, key }) => {
            let (x, cutoff, q) = (input(0), input(1), input(2));
            let fixed = constant(cutoff, n).zip(constant(q, n));
            for i in 0..n {
                // Modulated cutoffs recompute coefficients every 8 samples.
                let k = fixed.map_or([cutoff[i & !7], q[i & !7]], |(c, q)| [c, q]);
                if k != *key {
                    *key = k;
                    svf.set(SvfCoefficients::new(k[0], k[1], fs as u32));
                }
                let o = svf.tick(x[i]);
                outputs[0][i] = match instr.op {
                    Op::Lowpass => o.low,
                    Op::Highpass => o.high,
                    Op::Bandpass => o.band * (1.0 / k[1].max(0.05)),
                    _ => o.low + o.high,
                };
            }
        }
        (Op::Peak | Op::LowShelf | Op::HighShelf, State::Biquad(bq)) => {
            let x = input(0);
            let (freq, q, gain) = if instr.op == Op::Peak {
                (input(1), Some(input(2)), input(3))
            } else {
                (input(1), None, input(2))
            };
            for i in 0..n {
                if i & 7 == 0 {
                    bq.design(instr.op, freq[i], q.map_or(0.707, |q| q[i]), gain[i], fs);
                }
                outputs[0][i] = bq.tick(x[i]);
            }
        }
        (Op::OnePole | Op::Slew, State::OnePole(z)) => {
            let (x, param) = (input(0), input(1));
            let coefficient = |p: f32| {
                if instr.op == Op::OnePole {
                    1.0 - (-TAU * p.clamp(1.0, fs * 0.49) / fs).exp()
                } else {
                    1.0 - (-1.0 / (p.max(1e-4) * fs)).exp()
                }
            };
            let fixed = constant(param, n).map(coefficient);
            for i in 0..n {
                let c = fixed.unwrap_or_else(|| coefficient(param[i]));
                *z += (x[i] - *z) * c;
                outputs[0][i] = *z;
            }
        }
        (
            Op::Sample,
            State::Sample {
                pos,
                playing,
                clip,
                auto,
            },
        ) => {
            let (clip_in, trigger, rate, start, loop_start, loop_end, looping) = (
                input(0),
                input(1),
                input(2),
                input(3),
                input(4),
                input(5),
                input(6),
            );
            for i in 0..n {
                let fire = trigger[i] > 0.5 || std::mem::take(auto);
                for out in outputs.iter_mut() {
                    out[i] = 0.0;
                }
                if fire {
                    *clip = (clip_in[i].max(0.0).round() as usize)
                        .min(ctx.clips.len().saturating_sub(1));
                    *playing = !ctx.clips.is_empty();
                    if let Some(c) = ctx.clips.get(*clip) {
                        *pos = f64::from(start[i].max(0.0) * c.sample_rate() as f32);
                    }
                }
                let Some(c) = ctx.clips.get(*clip).filter(|_| *playing) else {
                    continue;
                };
                let len = c.len() as f64;
                let rate_scale = c.sample_rate() as f32 / fs;
                let le = f64::from(loop_end[i].max(0.0) * c.sample_rate() as f32);
                let le = if le <= 0.0 { len } else { le.min(len) };
                let ls = f64::from(loop_start[i].max(0.0) * c.sample_rate() as f32)
                    .min(le - 1.0)
                    .max(0.0);
                if looping[i] > 0.5 && *pos >= le {
                    *pos = ls + (*pos - ls) % (le - ls).max(1.0);
                }
                if *pos >= len {
                    *playing = false;
                    outputs[3][i] = 1.0;
                    continue;
                }
                let (l, r) = frame(c, *pos);
                outputs[0][i] = (l + r) * 0.5;
                outputs[1][i] = l;
                outputs[2][i] = r;
                *pos += f64::from((rate[i] * rate_scale).max(0.0));
            }
        }
        (
            Op::Granular,
            State::Granular {
                rng,
                grains,
                countdown,
            },
        ) => {
            let (clip_in, position, size, density, pitch, spread) =
                (input(0), input(1), input(2), input(3), input(4), input(5));
            for i in 0..n {
                let mut sum = 0.0;
                let Some(c) = ctx.clips.get(clip_in[i].max(0.0).round() as usize) else {
                    outputs[0][i] = 0.0;
                    continue;
                };
                *countdown -= 1.0;
                if *countdown <= 0.0 {
                    *countdown += f64::from(fs / density[i].clamp(0.1, 1000.0));
                    if let Some(g) = grains.iter_mut().find(|g| g.age >= g.len) {
                        let jitter = rng.bipolar() * spread[i].clamp(0.0, 1.0);
                        let at = (position[i] + jitter).clamp(0.0, 1.0);
                        let len = (size[i].clamp(0.005, 1.0) * fs) as u32;
                        *g = Grain {
                            pos: f64::from(at) * (c.len().saturating_sub(1)) as f64,
                            step: f64::from(
                                pitch[i].clamp(0.05, 8.0) * c.sample_rate() as f32 / fs,
                            ),
                            age: 0,
                            len: len.max(2),
                        };
                    }
                }
                for g in grains.iter_mut().filter(|g| g.age < g.len) {
                    let window = 0.5 - 0.5 * (TAU * g.age as f32 / g.len as f32).cos();
                    let (l, r) = frame(c, g.pos % c.len() as f64);
                    sum += (l + r) * 0.5 * window;
                    g.pos += g.step;
                    g.age += 1;
                }
                outputs[0][i] = sum * 0.5;
            }
        }
        (Op::Mix, _) => {
            let out = &mut outputs[0];
            out[..n].copy_from_slice(&input(0)[..n]);
            for k in 1..instr.inputs.len() {
                for (o, v) in out[..n].iter_mut().zip(&input(k)[..n]) {
                    *o += v;
                }
            }
        }
        (op @ (Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Min | Op::Max), _) => {
            let (a, b) = (input(0), input(1));
            for ((o, a), b) in outputs[0][..n].iter_mut().zip(&a[..n]).zip(&b[..n]) {
                *o = match op {
                    Op::Add => a + b,
                    Op::Sub => a - b,
                    Op::Mul => a * b,
                    Op::Div => {
                        if *b == 0.0 {
                            0.0
                        } else {
                            a / b
                        }
                    }
                    Op::Min => a.min(*b),
                    _ => a.max(*b),
                };
            }
        }
        (Op::Abs, _) => {
            for (o, x) in outputs[0][..n].iter_mut().zip(&input(0)[..n]) {
                *o = x.abs();
            }
        }
        (Op::Clamp, _) => {
            let (x, lo, hi) = (input(0), input(1), input(2));
            for i in 0..n {
                outputs[0][i] = x[i].max(lo[i]).min(hi[i]);
            }
        }
        (Op::Lerp, _) => {
            let (a, b, t) = (input(0), input(1), input(2));
            for i in 0..n {
                outputs[0][i] = a[i] + (b[i] - a[i]) * t[i];
            }
        }
        (Op::Map, _) => {
            let (x, il, ih, ol, oh) = (input(0), input(1), input(2), input(3), input(4));
            for i in 0..n {
                let span = ih[i] - il[i];
                let t = if span == 0.0 {
                    0.0
                } else {
                    ((x[i] - il[i]) / span).clamp(0.0, 1.0)
                };
                outputs[0][i] = ol[i] + (oh[i] - ol[i]) * t;
            }
        }
        (Op::Pow, _) => {
            let (x, e) = (input(0), input(1));
            for i in 0..n {
                outputs[0][i] = x[i].max(0.0).powf(e[i]);
            }
        }
        (Op::Gain, _) => {
            let (x, db) = (input(0), input(1));
            let fixed = constant(db, n).map(|d| 10f32.powf(d / 20.0));
            for i in 0..n {
                outputs[0][i] = x[i] * fixed.unwrap_or_else(|| 10f32.powf(db[i] / 20.0));
            }
        }
        (Op::Crossfade, _) => {
            let (a, b, t) = (input(0), input(1), input(2));
            let fixed = constant(t, n).map(|t| (t.clamp(0.0, 1.0) * FRAC_PI_2).sin_cos());
            for i in 0..n {
                let (sin, cos) =
                    fixed.unwrap_or_else(|| (t[i].clamp(0.0, 1.0) * FRAC_PI_2).sin_cos());
                outputs[0][i] = a[i] * cos + b[i] * sin;
            }
        }
        (Op::Saturate, _) => {
            let (x, drive) = (input(0), input(1));
            for i in 0..n {
                outputs[0][i] = soft_clip(x[i] * drive[i]);
            }
        }
        (Op::Pan, _) => {
            let (x, pos) = (input(0), input(1));
            let gains = |p: f32| {
                let p = (p.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
                (p.cos(), p.sin())
            };
            let fixed = constant(pos, n).map(gains);
            let (left, right) = outputs.split_at_mut(1);
            for i in 0..n {
                let (l, r) = fixed.unwrap_or_else(|| gains(pos[i]));
                left[0][i] = x[i] * l;
                right[0][i] = x[i] * r;
            }
        }
        (Op::Switch, _) => {
            let index = input(0);
            let items = instr.inputs.len() - 1;
            for i in 0..n {
                let k = (index[i].round().max(0.0) as usize).min(items - 1);
                outputs[0][i] = input(1 + k)[i];
            }
        }
        (
            Op::RandomRange,
            State::Random {
                rng,
                value,
                started,
            },
        ) => {
            let (trigger, lo, hi) = (input(0), input(2), input(3));
            for i in 0..n {
                if trigger[i] > 0.5 || !*started {
                    *started = true;
                    *value = lo[i] + (hi[i] - lo[i]) * rng.unit();
                }
                outputs[0][i] = *value;
            }
        }
        (Op::Chance, State::Random { rng, .. }) => {
            let (trigger, p) = (input(0), input(2));
            for i in 0..n {
                outputs[0][i] = if trigger[i] > 0.5 && rng.unit() < p[i] {
                    1.0
                } else {
                    0.0
                };
            }
        }
        (
            Op::RandomPick,
            State::Pick {
                rng,
                index,
                started,
            },
        ) => {
            let trigger = input(0);
            let items = instr.inputs.len() - 2;
            for i in 0..n {
                if trigger[i] > 0.5 || !*started {
                    // The first pick is uniform; later picks never repeat the previous item.
                    *index = if *started {
                        let offset = 1 + (rng.unit() * (items - 1) as f32) as usize;
                        (*index + offset.min(items - 1)) % items
                    } else {
                        ((rng.unit() * items as f32) as usize).min(items - 1)
                    };
                    *started = true;
                }
                outputs[0][i] = input(2 + *index)[i];
            }
        }
        (Op::Metro, State::Counter { remaining }) => {
            let rate = input(0);
            for i in 0..n {
                outputs[0][i] = 0.0;
                *remaining -= 1.0;
                if *remaining <= 0.0 {
                    outputs[0][i] = 1.0;
                    *remaining += f64::from(fs / rate[i].clamp(0.001, fs));
                }
            }
        }
        (Op::Start, State::Fired(fired)) => {
            outputs[0][..n].fill(0.0);
            if !std::mem::replace(fired, true) {
                outputs[0][0] = 1.0;
            }
        }
        (Op::Delay, State::Pending { at, count }) => {
            let (trigger, time) = (input(0), input(1));
            for i in 0..n {
                outputs[0][i] = 0.0;
                let mut k = 0;
                while k < *count {
                    at[k] -= 1.0;
                    if at[k] <= 0.0 {
                        outputs[0][i] = 1.0;
                        at[k] = at[*count - 1];
                        *count -= 1;
                    } else {
                        k += 1;
                    }
                }
                if trigger[i] > 0.5 && *count < at.len() {
                    at[*count] = f64::from(time[i].max(0.0) * fs).max(1.0);
                    *count += 1;
                }
            }
        }
        (Op::Every, State::Count(count)) => {
            let (trigger, every) = (input(0), input(1));
            for i in 0..n {
                outputs[0][i] = 0.0;
                if trigger[i] > 0.5 {
                    *count += 1;
                    if *count >= every[i].round().max(1.0) as u32 {
                        *count = 0;
                        outputs[0][i] = 1.0;
                    }
                }
            }
        }
        (Op::Seq, State::Step { step, started }) => {
            let trigger = input(0);
            let steps = instr.inputs.len() - 1;
            for i in 0..n {
                if trigger[i] > 0.5 {
                    *step = if *started { (*step + 1) % steps } else { 0 };
                    *started = true;
                }
                outputs[0][i] = input(1 + *step)[i];
            }
        }
        (Op::Threshold, State::Previous(previous)) => {
            let (x, level) = (input(0), input(1));
            for i in 0..n {
                outputs[0][i] = if *previous < level[i] && x[i] >= level[i] {
                    1.0
                } else {
                    0.0
                };
                *previous = x[i];
            }
        }
        (Op::Echo, State::Echo { buffer, cursor }) => {
            let (x, time, feedback) = (input(0), input(1), input(2));
            let len = buffer.len();
            for i in 0..n {
                let delay = (time[i].max(0.0) * fs).clamp(1.0, (len - 2) as f32);
                let read = (*cursor as f32 - delay).rem_euclid(len as f32);
                let j = read.floor() as usize % len;
                let t = read - read.floor();
                let delayed = buffer[j] + (buffer[(j + 1) % len] - buffer[j]) * t;
                buffer[*cursor] = x[i] + delayed * feedback[i].clamp(-0.98, 0.98);
                *cursor = (*cursor + 1) % len;
                outputs[0][i] = delayed;
            }
        }
        _ => {
            for out in outputs.iter_mut() {
                out[..n].fill(0.0);
            }
        }
    }
}
