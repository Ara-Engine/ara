//! Offline agent evidence: WAV plus EBU R128, true/sample peaks and spectrum JSON.

use crate::{
    AudioEngine, AudioError, Frame, Result,
    device::{OfflineBackend, render_to},
    importer::pcm_wav,
};
use ebur128::{EbuR128, Mode};
use serde::Serialize;
use std::path::Path;

/// One logarithmic spectrum band's RMS amplitude, from a deterministic Hann-windowed DFT.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpectrumBand {
    /// Inclusive lower frequency in hertz.
    pub low_hz: f64,
    /// Exclusive upper frequency in hertz.
    pub high_hz: f64,
    /// Mean energy across both channels, in dBFS; None means no energy.
    pub rms_dbfs: Option<f64>,
}
/// Machine-readable evidence for audio.play and clipping/silence assertions.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    /// Output rate used by the offline renderer.
    pub sample_rate: u32,
    /// Captured stereo frame count.
    pub frames: usize,
    /// Capture duration in seconds.
    pub seconds: f64,
    /// EBU R128/BS.1770 integrated loudness. None for silence or captures shorter than 400 ms.
    pub integrated_lufs: Option<f64>,
    /// Largest absolute sample after limiting.
    pub sample_peak: f64,
    /// Oversampled true peak after limiting, detecting intersample overloads.
    pub true_peak: f64,
    /// Stereo RMS in dBFS. None for exact silence.
    pub rms_dbfs: Option<f64>,
    /// Fraction of frames where both channels are below -80 dBFS.
    pub silence_fraction: f64,
    /// True when the entire capture stays below -80 dBFS.
    pub silent: bool,
    /// Output channel samples at or beyond full scale, or non-finite samples.
    pub clipped_samples: usize,
    /// Source mix samples beyond full scale before final limiting, during this capture.
    pub pre_limiter_overloads: u64,
    /// True for sample clipping, true-peak overload or pre-limiter overload.
    pub clipping: bool,
    /// Separate left and right RMS amplitudes, useful for spatial assertions.
    pub channel_rms: [f64; 2],
    /// Approximate long-term spectral energy in fixed logarithmic bands.
    pub spectrum: Vec<SpectrumBand>,
    /// BLAKE3 of the delivered PCM16 WAV, stable on the same platform and binary.
    pub wav_blake3: String,
}
/// A self-contained capture; publishing transport belongs to the protocol job layer.
pub struct AudioCapture {
    /// Standard stereo PCM16 WAV suitable for listening and golden hashing.
    pub wav: Vec<u8>,
    /// Structured verification results.
    pub analysis: Analysis,
}
impl AudioCapture {
    /// Canonical JSON through Ara's writer, never serde's generic text formatter.
    pub fn analysis_json(&self) -> Result<String> {
        let value =
            serde_json::to_value(&self.analysis).map_err(|e| AudioError::Codec(e.to_string()))?;
        Ok(ara_json::to_canonical_string(&ara_json::sort_keys(&value)))
    }
    /// Writes caller-chosen artifacts on an offline job worker. Does not edit engine documents.
    pub fn write(&self, wav: impl AsRef<Path>, json: impl AsRef<Path>) -> Result<()> {
        std::fs::write(wav, &self.wav)?;
        std::fs::write(json, self.analysis_json()?)?;
        Ok(())
    }
}

/// Captures the next seconds from an offline engine, bounded to 60 seconds.
/// Run on an isolated replay/presentation engine so agents never interrupt human playback (D20).
pub fn capture_audio(engine: &mut AudioEngine, seconds: f64) -> Result<AudioCapture> {
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 60.0 {
        return Err(AudioError::Invalid("capture seconds must be in (0,60]"));
    }
    let frames = (seconds * engine.sample_rate() as f64).round() as usize;
    if frames == 0 {
        return Err(AudioError::Invalid("capture shorter than one frame"));
    }
    let before = engine.delivered_overloads();
    let mut backend = OfflineBackend::new(engine.sample_rate(), frames)?;
    render_to(engine, &mut backend, frames)?;
    analyze(
        engine.sample_rate(),
        backend.samples(),
        engine.delivered_overloads() - before,
    )
}

/// Analyzes interleaved stereo samples, also useful for independently recorded device output.
pub fn analyze(
    sample_rate: u32,
    samples: &[f32],
    pre_limiter_overloads: u64,
) -> Result<AudioCapture> {
    if !(8000..=192000).contains(&sample_rate)
        || samples.is_empty()
        || !samples.len().is_multiple_of(2)
        || samples.iter().any(|s| !s.is_finite())
    {
        return Err(AudioError::Invalid("finite stereo analysis input"));
    }
    let error = |e: ebur128::Error| AudioError::Codec(e.to_string());
    let mut loudness = EbuR128::new(2, sample_rate, Mode::I | Mode::TRUE_PEAK).map_err(error)?;
    loudness.add_frames_f32(samples).map_err(error)?;
    let integrated = loudness.loudness_global().map_err(error)?;
    // Read loudness before flushing the true-peak interpolator's remaining history.
    loudness.add_frames_f32(&[0.0; 256]).map_err(error)?;
    let mut true_peak = 0.0_f64;
    for channel in 0..2 {
        true_peak = true_peak.max(loudness.true_peak(channel).map_err(error)?);
    }
    let frames = samples.len() / 2;
    let mut square = [0.0; 2];
    let mut peak = 0.0_f64;
    let mut silent = 0;
    let mut clipped = 0;
    let mut pcm = Vec::with_capacity(frames);
    for frame in samples.as_chunks::<2>().0 {
        if frame[0].abs() < 0.0001 && frame[1].abs() < 0.0001 {
            silent += 1;
        }
        for c in 0..2 {
            let s = frame[c] as f64;
            square[c] += s * s;
            peak = peak.max(s.abs());
            clipped += usize::from(s.abs() >= 1.0);
        }
        pcm.push(Frame::new(frame[0], frame[1]));
    }
    let channel_rms = square.map(|s| (s / frames as f64).sqrt());
    let rms = ((square[0] + square[1]) / samples.len() as f64).sqrt();
    let wav = pcm_wav(sample_rate, &pcm)?;
    let hash = blake3::hash(&wav).to_hex().to_string();
    let analysis = Analysis {
        sample_rate,
        frames,
        seconds: frames as f64 / sample_rate as f64,
        integrated_lufs: if frames >= sample_rate as usize * 4 / 10 && integrated.is_finite() {
            Some(integrated)
        } else {
            None
        },
        sample_peak: peak,
        true_peak,
        rms_dbfs: db(rms),
        silence_fraction: silent as f64 / frames as f64,
        silent: silent == frames,
        clipped_samples: clipped,
        pre_limiter_overloads,
        clipping: clipped > 0 || true_peak > 1.0 || pre_limiter_overloads > 0,
        channel_rms,
        spectrum: spectrum(sample_rate, samples),
        wav_blake3: hash,
    };
    Ok(AudioCapture { wav, analysis })
}
fn db(amplitude: f64) -> Option<f64> {
    if amplitude > 0.0 {
        Some(20.0 * amplitude.log10())
    } else {
        None
    }
}
fn spectrum(rate: u32, samples: &[f32]) -> Vec<SpectrumBand> {
    let nyquist = rate as f64 / 2.0;
    let edges = [0.0, 125.0, 500.0, 2000.0, 8000.0_f64.min(nyquist), nyquist];
    let mut energy = [0.0; 5];
    let mut count = 0;
    let n = 1024_usize.min(samples.len() / 2);
    let size = n.next_power_of_two();
    let mut planner = rustfft::FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(size);
    let mut data = vec![rustfft::num_complex::Complex::default(); size];
    // Sample up to 32 evenly distributed windows. FFT analysis never runs in a callback.
    let windows = 32_usize.min((samples.len() / 2).div_ceil(n));
    for window in 0..windows {
        let start = if windows == 1 {
            0
        } else {
            (samples.len() / 2 - n) * window / (windows - 1)
        };
        for channel in 0..2 {
            data.fill(rustfft::num_complex::Complex::default());
            for (i, value) in data.iter_mut().take(n).enumerate() {
                value.re = samples[(start + i) * 2 + channel] as f64
                    * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos());
            }
            fft.process(&mut data);
            for (bin, value) in data.iter().enumerate().take(size / 2 + 1).skip(1) {
                let hz = bin as f64 * rate as f64 / size as f64;
                if let Some(b) = edges
                    .windows(2)
                    .position(|edge| edge[0] < edge[1] && hz >= edge[0] && hz <= edge[1])
                {
                    energy[b] += 2.0 * value.norm_sqr() / (size * n) as f64 / (0.375);
                }
            }
            count += 1;
        }
    }
    edges
        .windows(2)
        .enumerate()
        .filter(|(_, e)| e[0] < e[1] && e[0] < rate as f64 / 2.0)
        .map(|(i, e)| SpectrumBand {
            low_hz: e[0],
            high_hz: e[1].min(rate as f64 / 2.0),
            rms_dbfs: db((energy[i] / count.max(1) as f64).sqrt()),
        })
        .collect()
}
