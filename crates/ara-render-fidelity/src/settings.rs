//! Exposure and display processing in physical units.
use crate::colour::{AutoExposure, Grading, ToneMapper};

/// Optional cinematic effects are bounded independently of transport quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// No DOF or motion blur.
    Gameplay,
    /// Depth-aware DOF and shutter-based motion blur.
    Cinematic,
}
/// Exposure and display processing. Values are linear until the SDR transfer.
#[derive(Debug, Clone, Copy)]
pub struct PostSettings {
    /// Manual EV100; None enables percentile-trimmed histogram metering.
    pub ev100: Option<f32>,
    /// Exposure compensation in stops.
    pub compensation: f32,
    /// Adaptation rate in inverse seconds.
    pub adaptation: f32,
    /// Histogram percentile trims and permitted EV100 range for automatic metering.
    pub auto_exposure: AutoExposure,
    /// Selectable SDR curve; exposure is applied before this transform.
    pub tone_mapper: ToneMapper,
    /// Energy fraction redistributed to the bloom kernel, from zero to 0.25.
    pub bloom: f32,
    /// Scene-linear white balance, contrast, saturation and vignette.
    pub grading: Grading,
    /// Sea-level homogeneous extinction in inverse metres.
    pub fog_density: f32,
    /// Lens focus distance in metres.
    pub focus_distance: f32,
    /// Blur diameter at one metre defocus, in render pixels.
    pub aperture: f32,
    /// A physical lens f-number (post-process volumes): when set, the blur follows
    /// the thin lens for the camera's field of view on a full-frame sensor instead
    /// of `aperture`.
    pub f_number: Option<f32>,
    /// Fraction of the frame interval exposed by motion blur.
    pub shutter: f32,
    /// Quality gate for DOF and motion blur.
    pub quality: Quality,
}
impl Default for PostSettings {
    fn default() -> Self {
        Self {
            ev100: Some(5.),
            compensation: 0.,
            adaptation: 3.,
            auto_exposure: AutoExposure::default(),
            tone_mapper: ToneMapper::AcesFit,
            bloom: 0.04,
            grading: Grading::default(),
            fog_density: 0.002,
            focus_distance: 5.,
            aperture: 2.,
            f_number: None,
            shutter: 0.5,
            quality: Quality::Gameplay,
        }
    }
}
/// Convert photographic EV100 to the scene-radiance exposure multiplier.
/// Uses the saturation-based calibration Lmax = 1.2 * 2^EV100.
pub fn exposure_from_ev100(ev100: f32, compensation: f32) -> f32 {
    2.0_f32.powf(compensation - ev100) / 1.2
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn photographic_stops_double() {
        assert!((exposure_from_ev100(4., 1.) / exposure_from_ev100(4., 0.) - 2.).abs() < 1e-6);
    }
}
