//! EBU R128 loudness measurement and peak-safe normalization gains.
//!
//! Music and dialogue buses normalize sources to an integrated target (Ara's default
//! is -16 LUFS). Measurement happens at import or on a loader; voices only apply
//! a constant trim computed at installation.

use crate::{AudioError, Frame, Result};
use ebur128::{EbuR128, Mode};

/// Default integrated loudness target for music and dialogue buses.
pub const DEFAULT_TARGET_LUFS: f32 = -16.0;
/// Largest normalization boost, +12 dB. Quieter sources stay quieter rather than exposing noise.
pub const MAX_BOOST: f32 = 4.0;

/// Integrated loudness of stereo frames, or None for silence and clips shorter than 400 ms.
pub fn integrated(sample_rate: u32, frames: &[Frame]) -> Result<Option<f32>> {
    if frames.len() < sample_rate as usize * 4 / 10 {
        return Ok(None);
    }
    let error = |e: ebur128::Error| AudioError::Codec(e.to_string());
    let mut meter = EbuR128::new(2, sample_rate, Mode::I).map_err(error)?;
    let mut block = [0.0_f32; 2048];
    for chunk in frames.chunks(1024) {
        for (pair, frame) in block.as_chunks_mut::<2>().0.iter_mut().zip(chunk) {
            *pair = [frame.left, frame.right];
        }
        meter
            .add_frames_f32(&block[..chunk.len() * 2])
            .map_err(error)?;
    }
    let lufs = meter.loudness_global().map_err(error)?;
    Ok(lufs.is_finite().then_some(lufs as f32))
}

/// Linear trim that moves `lufs` to `target`, capped at `MAX_BOOST` and so the
/// trimmed sample peak stays at or below full scale. Unknown loudness is untouched.
pub fn trim(target: f32, lufs: Option<f32>, peak: f32) -> f32 {
    let Some(lufs) = lufs.filter(|l| l.is_finite()) else {
        return 1.0;
    };
    let mut gain = 10f32.powf((target - lufs) / 20.0).min(MAX_BOOST);
    if peak.is_finite() && peak > 0.0 {
        gain = gain.min(1.0 / peak);
    }
    if gain.is_finite() { gain.max(0.0) } else { 1.0 }
}
