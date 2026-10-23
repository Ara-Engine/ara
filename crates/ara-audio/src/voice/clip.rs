//! Immutable resident PCM with optional loudness metadata.

use crate::{AudioError, Frame, Result, loudness};
use std::sync::Arc;

/// Immutable resident audio. Cloning a clip shares its samples on the control path.
#[derive(Clone)]
pub struct Clip {
    pub(crate) frames: Arc<[Frame]>,
    sample_rate: u32,
    peak: f32,
    loudness: Option<f32>,
}

impl Clip {
    /// Takes ownership of finite mono/stereo frames, enforcing the resident byte limit.
    pub fn new(sample_rate: u32, frames: Vec<Frame>, max_bytes: usize) -> Result<Self> {
        if !(8000..=192000).contains(&sample_rate) || frames.is_empty() {
            return Err(AudioError::Invalid("clip sample rate or empty clip"));
        }
        if frames.len().saturating_mul(size_of::<Frame>()) > max_bytes {
            return Err(AudioError::Capacity("resident clip budget"));
        }
        let mut peak = 0.0_f32;
        for f in &frames {
            if !f.left.is_finite() || !f.right.is_finite() {
                return Err(AudioError::Invalid("non-finite PCM"));
            }
            peak = peak.max(f.left.abs()).max(f.right.abs());
        }
        Ok(Self {
            frames: frames.into(),
            sample_rate,
            peak,
            loudness: None,
        })
    }
    /// Source frames per second, before resampling.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// Number of stereo frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    /// Clips are nonempty by construction.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    /// Borrow decoded stereo samples for cooking or inspection.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
    /// Largest absolute sample.
    pub fn peak(&self) -> f32 {
        self.peak
    }
    /// Integrated loudness (LUFS), when measured or supplied from cooked metadata.
    pub fn loudness(&self) -> Option<f32> {
        self.loudness
    }
    /// Attaches cooked loudness metadata (`AudioImporter` reports it per artifact).
    pub fn with_loudness(mut self, lufs: Option<f32>) -> Self {
        self.loudness = lufs.filter(|l| l.is_finite());
        self
    }
    /// Measures EBU R128 integrated loudness on this (loader) thread. Silent or
    /// sub-400 ms clips have no integrated loudness and are never normalized.
    pub fn measure_loudness(self) -> Result<Self> {
        let lufs = loudness::integrated(self.sample_rate, &self.frames)?;
        Ok(self.with_loudness(lufs))
    }
}

impl std::fmt::Debug for Clip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clip")
            .field("sample_rate", &self.sample_rate)
            .field("frames", &self.frames.len())
            .field("peak", &self.peak)
            .field("loudness", &self.loudness)
            .finish()
    }
}
