//! Glint's sounds, synthesised here (no third-party audio): the roll loop, the jump, the
//! landing, the orb chime, the checkpoint, the cannon's wind-up and shot, the fall, the finish
//! and two looping music stems. Written as 48 kHz mono 16-bit WAV into `content/audio/`. The
//! only noise source is a fixed-seed LCG, so a machine always writes the same files.
use anyhow::Result;
use std::{f32::consts::TAU, path::Path};

const RATE: f32 = 48_000.0;

/// Clip file stems and their asset IDs, in the order `write` makes them.
pub const CLIPS: [(&str, &str); 9] = [
    ("roll", "as_glintroll"),
    ("jump", "as_glintjump"),
    ("land", "as_glintland"),
    ("orb", "as_glintorb"),
    ("gate", "as_glintgate"),
    ("load", "as_glintload"),
    ("fire", "as_glintfire"),
    ("fall", "as_glintfall"),
    ("finish", "as_glintfinish"),
];
/// The music stems: file stems and asset IDs.
pub const STEMS: [(&str, &str); 2] = [("pad", "as_glintpad"), ("pulse", "as_glintpulse")];
/// The music loop: 6 bars of 4/4 at 90 bpm.
pub const LOOP: (u32, u32) = (6, 90);

/// White noise in -1..1 from a fixed seed.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// `seconds` of samples from `f(t)`.
fn render(seconds: f32, mut f: impl FnMut(f32) -> f32) -> Vec<f32> {
    (0..(seconds * RATE) as usize)
        .map(|i| f(i as f32 / RATE))
        .collect()
}

/// A note with a few harmonic partials and an exponential decay, starting at `at`.
fn pluck(t: f32, at: f32, hz: f32, decay: f32, partials: &[(f32, f32)]) -> f32 {
    let t = t - at;
    if t < 0.0 {
        return 0.0;
    }
    let attack = (t / 0.004).min(1.0);
    partials
        .iter()
        .map(|(ratio, gain)| (TAU * hz * ratio * t).sin() * gain)
        .sum::<f32>()
        * attack
        * (-t * decay).exp()
}

/// A short fade at both ends so one-shots never click.
fn faded(mut samples: Vec<f32>) -> Vec<f32> {
    let n = samples.len();
    let edge = (0.004 * RATE) as usize;
    for i in 0..edge.min(n / 2) {
        let k = i as f32 / edge as f32;
        samples[i] *= k;
        samples[n - 1 - i] *= k;
    }
    samples
}

/// Writes `samples` to `dir/name.wav`, normalised to `peak`.
fn wav(dir: &Path, name: &str, samples: &[f32], peak: f32) -> Result<()> {
    let top = samples.iter().fold(1e-6f32, |m, s| m.max(s.abs()));
    let bytes = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s / top * peak * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(dir.join(format!("{name}.wav")), out)?;
    Ok(())
}

/// The roll: a 2 s rumble that loops exactly (every partial has a whole number of cycles in it).
fn roll() -> Vec<f32> {
    let mut noise = Noise(7);
    let partials: Vec<(f32, f32, f32)> = (0..90)
        .map(|_| {
            // Mostly 30 to 400 Hz; one partial in five reaches up to 1.6 kHz for a little grit.
            let reach = if noise.next() > 0.6 { 3200.0 } else { 760.0 };
            let cycles = (60.0 + (noise.next() * 0.5 + 0.5) * reach).round();
            (
                cycles / 2.0,
                noise.next() * TAU,
                1.0 / (cycles / 2.0).sqrt(),
            )
        })
        .collect();
    render(2.0, |t| {
        let grain = 1.0 + 0.22 * (TAU * 5.0 * t).sin() + 0.12 * (TAU * 13.0 * t).sin();
        partials
            .iter()
            .map(|(hz, phase, gain)| (TAU * hz * t + phase).sin() * gain)
            .sum::<f32>()
            * grain
    })
}

/// A rising chirp with a puff of air.
fn jump() -> Vec<f32> {
    let mut noise = Noise(11);
    let (mut phase, mut air) = (0.0f32, 0.0f32);
    faded(render(0.24, |t| {
        let k = t / 0.24;
        phase += TAU * (260.0 + 560.0 * k.powf(0.6)) / RATE;
        air += (noise.next() - air) * 0.2;
        (phase.sin() + 0.3 * (phase * 2.0).sin()) * (1.0 - k).powi(2)
            + air * 0.5 * (1.0 - k).powi(3)
    }))
}

/// A soft thud.
fn land() -> Vec<f32> {
    let mut noise = Noise(13);
    let (mut phase, mut low) = (0.0f32, 0.0f32);
    faded(render(0.3, |t| {
        phase += TAU * (48.0 + 90.0 * (-t * 22.0).exp()) / RATE;
        low += (noise.next() - low) * 0.08;
        phase.sin() * (-t * 16.0).exp() + low * 1.6 * (-t * 45.0).exp()
    }))
}

const BELL: [(f32, f32); 3] = [(1.0, 1.0), (2.0, 0.32), (3.01, 0.14)];
const SOFT: [(f32, f32); 3] = [(1.0, 1.0), (2.0, 0.22), (3.0, 0.08)];

/// Two bright notes, a fifth apart.
fn orb() -> Vec<f32> {
    faded(render(0.5, |t| {
        pluck(t, 0.0, 1318.5, 9.0, &BELL) + pluck(t, 0.07, 1975.5, 8.0, &BELL)
    }))
}

/// A rising four-note figure.
fn gate() -> Vec<f32> {
    faded(render(1.1, |t| {
        [
            (0.0, 523.25),
            (0.09, 659.25),
            (0.18, 783.99),
            (0.27, 1046.5),
        ]
        .iter()
        .map(|(at, hz)| pluck(t, *at, *hz, 4.5, &BELL))
        .sum()
    }))
}

/// The cannon winding up: a rising, quickening hum (80 ticks long).
fn load() -> Vec<f32> {
    let seconds = 80.0 / 60.0;
    let (mut phase, mut beat) = (0.0f32, 0.0f32);
    faded(render(seconds, |t| {
        let k = t / seconds;
        phase += TAU * 70.0 * 2f32.powf(2.3 * k) / RATE;
        beat += TAU * (8.0 + 22.0 * k) / RATE;
        (phase.sin() + 0.5 * (phase * 2.0).sin() + 0.25 * (phase * 3.0).sin())
            * (0.25 + 0.75 * k)
            * (0.75 + 0.25 * beat.sin())
    }))
}

/// The shot: a low boom, a burst and a tail of rushing air.
fn fire() -> Vec<f32> {
    let mut noise = Noise(17);
    let (mut phase, mut burst, mut air) = (0.0f32, 0.0f32, 0.0f32);
    faded(render(1.6, |t| {
        phase += TAU * (32.0 + 70.0 * (-t * 9.0).exp()) / RATE;
        let n = noise.next();
        burst += (n - burst) * (0.02 + 0.3 * (-t * 14.0).exp());
        air += (n - air) * 0.35;
        phase.sin() * (-t * 3.4).exp() * 1.2
            + burst * 1.4 * (-t * 7.0).exp()
            + air * 0.28 * (t * 9.0).min(1.0) * (-t * 2.6).exp()
    }))
}

/// A falling whistle.
fn fall() -> Vec<f32> {
    let mut noise = Noise(19);
    let (mut phase, mut air) = (0.0f32, 0.0f32);
    faded(render(0.8, |t| {
        let k = t / 0.8;
        phase += TAU * 980.0 * (-k * 1.9).exp() / RATE;
        air += (noise.next() - air) * 0.25;
        (phase.sin() * 0.8 + air * 0.5) * (std::f32::consts::PI * k).sin()
    }))
}

/// An arpeggio up into a held chord.
fn finish() -> Vec<f32> {
    faded(render(2.4, |t| {
        let run: f32 = [
            (0.0, 523.25),
            (0.1, 659.25),
            (0.2, 783.99),
            (0.3, 1046.5),
            (0.4, 1318.5),
        ]
        .iter()
        .map(|(at, hz)| pluck(t, *at, *hz, 5.0, &BELL))
        .sum();
        let chord: f32 = [523.25, 659.25, 783.99, 1046.5]
            .iter()
            .map(|hz| pluck(t, 0.55, *hz, 1.7, &SOFT))
            .sum();
        run + chord * 0.7
    }))
}

/// The chords of the loop, one per 4 s: A minor 9, F major 7, C major 7, G 6 (Hz).
const CHORDS: [[f32; 4]; 4] = [
    [110.0, 164.81, 196.0, 246.94],
    [87.31, 130.81, 164.81, 220.0],
    [130.81, 164.81, 196.0, 246.94],
    [98.0, 146.83, 196.0, 220.0],
];
const SECONDS: f32 = 16.0;

/// Adds `voice(t)` over `from..from + length` into the loop, wrapping past its end.
fn layer(out: &mut [f32], from: f32, length: f32, mut voice: impl FnMut(f32) -> f32) {
    let n = out.len();
    let start = (from * RATE) as usize;
    for i in 0..(length * RATE) as usize {
        out[(start + i) % n] += voice(i as f32 / RATE);
    }
}

/// The pad: each chord swells in and out over 5.2 s, overlapping the next (and the loop's seam).
fn pad() -> Vec<f32> {
    let mut out = vec![0.0; (SECONDS * RATE) as usize];
    for (index, chord) in CHORDS.iter().enumerate() {
        for (voice, hz) in chord.iter().enumerate() {
            let drift = 1.0 + 0.0016 * (voice as f32 - 1.5);
            layer(&mut out, index as f32 * 4.0 - 0.6 + SECONDS, 5.2, |t| {
                let swell = (std::f32::consts::PI * t / 5.2).sin().powi(2);
                ((TAU * hz * t).sin()
                    + 0.6 * (TAU * hz * drift * 2.0 * t).sin()
                    + 0.18 * (TAU * hz * 3.0 * t).sin())
                    * swell
            });
        }
    }
    out
}

/// The pulse: eighth notes walking each chord an octave up; tails ring across the seam.
fn pulse() -> Vec<f32> {
    let mut out = vec![0.0; (SECONDS * RATE) as usize];
    let eighth = 60.0 / LOOP.1 as f32 / 2.0;
    for step in 0..(SECONDS / eighth) as usize {
        let at = step as f32 * eighth;
        let chord = CHORDS[(at / 4.0) as usize % 4];
        let hz = chord[[0, 2, 1, 3, 2, 1][step % 6]] * 4.0;
        let accent = if step % 4 == 0 { 1.0 } else { 0.6 };
        layer(&mut out, at, 1.2, |t| {
            pluck(t, 0.0, hz, 5.5, &SOFT) * accent
        });
    }
    out
}

/// Writes every clip and stem into `dir`.
pub fn write(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let clips = [
        roll(),
        jump(),
        land(),
        orb(),
        gate(),
        load(),
        fire(),
        fall(),
        finish(),
    ];
    for ((name, _), samples) in CLIPS.iter().zip(&clips) {
        wav(dir, name, samples, 0.85)?;
    }
    for ((name, _), samples) in STEMS.iter().zip([pad(), pulse()]) {
        wav(dir, name, &samples, 0.7)?;
    }
    Ok(())
}
