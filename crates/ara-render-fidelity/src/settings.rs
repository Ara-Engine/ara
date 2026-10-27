//! Physical and temporal contracts shared by every reconstruction backend.
use crate::colour::{AutoExposure, Grading, ToneMapper};
use anyhow::{Result, ensure};
use ara_gpu::Image;
use ara_render::scene::CameraSpec;

/// Capability tier, independent of the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Hardware ray queries and dynamic probe updates.
    Rt,
    /// Software visibility with mandatory baked probes.
    Raster,
    /// Baked probes with expensive post effects disabled.
    Mobile,
}
/// Transport selected for a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Ray-gathered diffuse/specular transport terminated in a DDGI field.
    Hybrid,
    /// Undenoised path transport with mixed GGX/diffuse importance sampling.
    Reference {
        /// Independent samples per output pixel.
        samples: u32,
        /// Maximum scattering events, from one through sixteen.
        bounces: u32,
    },
}
/// Reconstruction algorithm; FSR1 follows the same temporal resolve as TAAU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upscaler {
    /// Engine temporal antialiasing/upscaling.
    Taau,
    /// AMD FSR1 EASU and RCAS, with engine temporal antialiasing first.
    Fsr1,
}
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
/// Per-view allocation and work budgets.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Internal transport resolution.
    pub render_size: [u32; 2],
    /// Offscreen display target resolution.
    pub display_size: [u32; 2],
    /// Hardware capability tier requested by the host.
    pub tier: Tier,
    /// Transport mode.
    pub mode: Mode,
    /// Reconstruction implementation.
    pub upscaler: Upscaler,
    /// Maximum primary probe paths per frame: 64-ray geometry updates, or
    /// 16-ray irradiance updates during a lighting response.
    /// Each path resolves up to five surfaces plus their explicit shadow rays.
    pub probe_ray_budget: u32,
    /// Fade traced specular into probe lighting over the preceding 0.1 roughness.
    /// Values above this cutoff use only the filtered directional probe field.
    pub reflection_cutoff: f32,
    /// Post processing in physical units.
    pub post: PostSettings,
}
impl Settings {
    /// Validate before allocating or publishing any replacement resources.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.render_size
                .into_iter()
                .chain(self.display_size)
                .all(|v| v > 0 && v <= 8192),
            "Invalid view extent"
        );
        ensure!(
            self.display_size[0] >= self.render_size[0]
                && self.display_size[1] >= self.render_size[1],
            "Display extent cannot be smaller than transport"
        );
        ensure!(
            self.reflection_cutoff.is_finite() && (0.0..=1.0).contains(&self.reflection_cutoff),
            "Invalid reflection cutoff"
        );
        ensure!(
            self.probe_ray_budget >= 64 && self.probe_ray_budget <= 1_048_576,
            "Invalid DDGI ray budget"
        );
        if let Mode::Reference { samples, bounces } = self.mode {
            ensure!(
                (1..=65536).contains(&samples) && (1..=16).contains(&bounces),
                "Invalid reference sample/bounce budget"
            );
            ensure!(
                self.tier == Tier::Rt,
                "Reference requires hardware ray queries"
            );
        }
        let p = self.post;
        let a = p.auto_exposure;
        ensure!(
            a.low_percentile.is_finite()
                && a.high_percentile.is_finite()
                && a.low_percentile >= 0.
                && a.high_percentile <= 1.
                && a.low_percentile < a.high_percentile
                && a.minimum_ev100.is_finite()
                && a.maximum_ev100.is_finite()
                && a.minimum_ev100 >= -24.
                && a.maximum_ev100 <= 32.
                && a.minimum_ev100 < a.maximum_ev100,
            "Invalid exposure histogram or EV100 limits"
        );
        ensure!(
            p.ev100
                .is_none_or(|v| v.is_finite() && (-24.0..=32.0).contains(&v)),
            "Invalid EV100"
        );
        ensure!(
            p.compensation.is_finite()
                && p.compensation.abs() <= 16.
                && p.adaptation.is_finite()
                && p.adaptation > 0.,
            "Invalid exposure adaptation"
        );
        ensure!(
            p.bloom.is_finite()
                && (0.0..=0.25).contains(&p.bloom)
                && p.fog_density.is_finite()
                && (0.0..=1.0).contains(&p.fog_density),
            "Invalid bloom/fog"
        );
        ensure!(p.grading.is_valid(), "Invalid colour grading");
        ensure!(
            p.focus_distance.is_finite()
                && p.focus_distance > 0.
                && p.aperture.is_finite()
                && (0.0..=32.0).contains(&p.aperture)
                && p.f_number.is_none_or(|n| (0.5..=64.0).contains(&n))
                && p.shutter.is_finite()
                && (0.0..=1.0).contains(&p.shutter),
            "Invalid lens"
        );
        Ok(())
    }
}
/// Inputs supplied by simulation/extraction at a frame boundary.
pub struct FrameInput<'a> {
    /// Camera with unjittered projection semantics.
    pub camera: &'a CameraSpec,
    /// Deterministic simulation frame index, used to seed sampling.
    pub tick: u32,
    /// Simulation delta in seconds; never derived from wall-clock time here.
    pub delta_seconds: f32,
    /// Camera cut, view reassignment, or content replacement.
    pub reset: bool,
    /// Lighting changed; retain geometric history with responsive radiance weights
    /// and faster probe coverage inside the configured ray budget.
    pub lighting_changed: bool,
    /// Disable subpixel jitter for exact deterministic captures.
    pub deterministic: bool,
}
/// Vendor-neutral temporal upscaler input contract. Motion is previous UV minus
/// current UV, excludes projection jitter, and includes object deformation.
pub struct UpscalerInput {
    /// Linear pre-exposed scene radiance.
    pub colour: Image,
    /// Positive view-space depth in metres (zero means sky).
    pub depth: Image,
    /// Current-to-previous UV motion, expected previous depth and reactive mask.
    pub motion: Image,
    /// Projection jitter in render pixels.
    pub jitter: [f32; 2],
    /// Linear pre-exposure multiplier used for colour.
    pub exposure: f32,
    /// Reject all previous samples after a discontinuity.
    pub reset: bool,
    /// Transport dimensions.
    pub render_size: [u32; 2],
    /// Presentation dimensions.
    pub display_size: [u32; 2],
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
    fn photographic_stops_double_and_invalid_contracts_fail() {
        assert!((exposure_from_ev100(4., 1.) / exposure_from_ev100(4., 0.) - 2.).abs() < 1e-6);
        let mut s = Settings {
            render_size: [320, 180],
            display_size: [640, 360],
            tier: Tier::Rt,
            mode: Mode::Hybrid,
            upscaler: Upscaler::Taau,
            probe_ray_budget: 1024,
            reflection_cutoff: 0.6,
            post: PostSettings::default(),
        };
        s.validate().unwrap();
        s.post.shutter = f32::NAN;
        assert!(s.validate().is_err());
    }
}
