//! Display mapping and histogram-metering contracts, independent of transport.
use glam::Vec3;

/// Explicit offline readback of the photographic exposure controller.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ExposureState {
    /// Multiplier applied to physical scene radiance before tone mapping.
    pub multiplier: f32,
    /// Preceding frame multiplier, used by display-history rejection.
    pub previous_multiplier: f32,
    /// Clamped target EV100, before adaptation and automatic compensation.
    pub target_ev100: f32,
    /// Percentile-trimmed geometric mean scene luminance; zero in manual mode.
    pub metered_luminance: f32,
}

/// SDR display transform applied after photographic exposure, before grading/sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ToneMapper {
    /// Narkowicz's ACES fitted curve, retained for existing captures.
    AcesFit = 0,
    /// Khronos PBR Neutral; preserves midtone colour and compresses highlights.
    PbrNeutral = 1,
}

/// Percentile-trimmed logarithmic luminance metering, with photographic EV limits.
#[derive(Debug, Clone, Copy)]
pub struct AutoExposure {
    /// Ignore this lower fraction of the histogram, from zero through one.
    pub low_percentile: f32,
    /// Ignore samples above this histogram fraction; greater than low_percentile.
    pub high_percentile: f32,
    /// Lowest permitted camera EV100 before exposure compensation.
    pub minimum_ev100: f32,
    /// Highest permitted camera EV100 before exposure compensation.
    pub maximum_ev100: f32,
}
impl Default for AutoExposure {
    fn default() -> Self {
        Self {
            low_percentile: 0.05,
            high_percentile: 0.95,
            minimum_ev100: -6.,
            maximum_ev100: 20.,
        }
    }
}

/// Map nonnegative linear Rec.709 radiance after exposure to linear SDR Rec.709.
/// Implements the Khronos PBR Neutral specification, or the legacy ACES fit.
pub fn tone_map(colour: Vec3, mapper: ToneMapper) -> Vec3 {
    let colour = colour.max(Vec3::ZERO);
    match mapper {
        ToneMapper::AcesFit => ((colour * (colour * 2.51 + Vec3::splat(0.03)))
            / (colour * (colour * 2.43 + Vec3::splat(0.59)) + Vec3::splat(0.14)))
        .clamp(Vec3::ZERO, Vec3::ONE),
        ToneMapper::PbrNeutral => {
            let floor = colour.min_element();
            let offset = if floor < 0.08 {
                floor - 6.25 * floor * floor
            } else {
                0.04
            };
            let colour = colour - Vec3::splat(offset);
            let peak = colour.max_element();
            if peak <= 0.76 {
                return colour;
            }
            let compressed = 1. - 0.24 * 0.24 / (peak - 0.52);
            let neutral = 1. - 1. / (1. + 0.15 * (peak - compressed));
            (colour * (compressed / peak)).lerp(Vec3::splat(compressed), neutral)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_preserves_midtone_chroma_and_rolls_to_white() {
        assert_eq!(tone_map(Vec3::ZERO, ToneMapper::PbrNeutral), Vec3::ZERO);
        let c = tone_map(Vec3::new(0.3, 0.5, 0.7), ToneMapper::PbrNeutral);
        assert!((c - Vec3::new(0.26, 0.46, 0.66)).length() < 1e-6);
        let mut last = 0.;
        for i in 0..1000 {
            let c = tone_map(Vec3::splat(i as f32 / 10.), ToneMapper::PbrNeutral);
            assert!(c.is_finite() && c.min_element() >= last && c.max_element() <= 1.);
            last = c.x;
        }
    }
}
