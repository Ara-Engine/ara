//! The raster look: authored exposure, display, sky and fog, applied by the passes
//! after the G-buffer (physical sky LUTs and their image-based lighting, froxel fog,
//! exposure metering, bloom and display grading).
//!
//! Hosts map the scene's authored look onto [`Look`] and call
//! [`crate::Renderer::set_look`] at a frame boundary. The defaults reproduce the
//! M1 baseline: manual exposure, the ACES fit, no bloom, no sky and no fog.
pub(crate) mod frame;
pub mod gi;
pub(crate) mod gpu;
pub(crate) mod passes;

pub use crate::renderer::post::ToneCurve;

/// Scene look applied by the raster renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Display curve applied after exposure and grading.
    pub curve: ToneCurve,
    /// Histogram metering; `None` uses the manual exposure passed to `render`.
    /// Captures always use their request's manual exposure.
    pub auto_exposure: Option<AutoExposure>,
    /// Energy fraction redistributed to the bloom kernel, from zero to 0.25.
    pub bloom: f32,
    /// Scene-linear grading after exposure.
    pub grading: Grading,
    /// Physical sky replacing the environment map for lighting and display.
    pub sky: Option<Sky>,
    /// Exponential height fog.
    pub fog: Option<Fog>,
}
impl Default for Look {
    fn default() -> Self {
        Self {
            curve: ToneCurve::AcesFit,
            auto_exposure: None,
            bloom: 0.,
            grading: Grading::default(),
            sky: None,
            fog: None,
        }
    }
}

/// Percentile-trimmed histogram metering towards 18% grey.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoExposure {
    /// Lowest permitted EV100.
    pub minimum_ev100: f32,
    /// Highest permitted EV100.
    pub maximum_ev100: f32,
    /// Fraction of darkest histogram samples ignored, from zero to one.
    pub low_percentile: f32,
    /// Fraction of samples below the brightest ignored, from zero to one.
    pub high_percentile: f32,
    /// Exposure compensation in stops.
    pub compensation: f32,
    /// Adaptation rate in inverse seconds (frames are taken as 1/60 s).
    pub adaptation: f32,
}
impl Default for AutoExposure {
    fn default() -> Self {
        Self {
            minimum_ev100: -6.,
            maximum_ev100: 20.,
            low_percentile: 0.05,
            high_percentile: 0.95,
            compensation: 0.,
            adaptation: 2.,
        }
    }
}

/// Scene-linear white balance, contrast, saturation and vignette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grading {
    /// Per-channel white-balance gain (neutral is one).
    pub white_balance_gain: [f32; 3],
    /// Power contrast about 18% grey (neutral is one).
    pub contrast: f32,
    /// Saturation (neutral is one, zero is greyscale).
    pub saturation: f32,
    /// Vignette strength from zero to one.
    pub vignette: f32,
}
impl Default for Grading {
    fn default() -> Self {
        Self {
            white_balance_gain: [1.; 3],
            contrast: 1.,
            saturation: 1.,
            vignette: 0.,
        }
    }
}

/// Earth-like physical sky (Rayleigh, Mie and ozone over a spherical planet).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    /// World-space altitude of sea level, in metres.
    pub sea_level: f32,
    /// Aerosol (Mie) density relative to clear Earth air, from zero to twenty.
    pub aerosols: f32,
    /// Angular radius of the sun disk in radians.
    pub sun_radius: f32,
}
impl Default for Sky {
    fn default() -> Self {
        Self {
            sea_level: 0.,
            aerosols: 1.,
            sun_radius: 0.00465,
        }
    }
}

/// Exponential height fog lit by the sun, sky and local lights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    /// Extinction at `base`, in inverse metres (zero to 0.1).
    pub extinction: f32,
    /// Density scale height above `base`, in metres.
    pub height: f32,
    /// World altitude with full `extinction`, in metres.
    pub base: f32,
    /// Linear scattering colour, zero to one per channel.
    pub tint: [f32; 3],
    /// Single-scattering albedo from zero to one; multiplies `tint`.
    pub albedo: f32,
    /// Henyey-Greenstein anisotropy, from zero to 0.9.
    pub anisotropy: f32,
}
impl Default for Fog {
    fn default() -> Self {
        Self {
            extinction: 0.0008,
            height: 90.,
            base: 0.,
            tint: [1.; 3],
            albedo: 0.9,
            anisotropy: 0.3,
        }
    }
}

impl Look {
    /// Rejects values the shaders cannot represent.
    pub fn validate(&self) -> anyhow::Result<()> {
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        let g = &self.grading;
        anyhow::ensure!(
            (0.0..=0.25).contains(&self.bloom)
                && finite(&g.white_balance_gain)
                && g.white_balance_gain.iter().all(|x| *x >= 0.)
                && (0.25..=4.0).contains(&g.contrast)
                && (0.0..=4.0).contains(&g.saturation)
                && (0.0..=1.0).contains(&g.vignette),
            "Invalid look grading or bloom"
        );
        if let Some(a) = self.auto_exposure {
            anyhow::ensure!(
                finite(&[a.minimum_ev100, a.maximum_ev100, a.compensation])
                    && a.minimum_ev100 < a.maximum_ev100
                    && 0. <= a.low_percentile
                    && a.low_percentile < a.high_percentile
                    && a.high_percentile <= 1.
                    && a.adaptation > 0.
                    && a.adaptation.is_finite(),
                "Invalid automatic exposure"
            );
        }
        if let Some(s) = self.sky {
            anyhow::ensure!(
                s.sea_level.is_finite()
                    && (0.0..=20.0).contains(&s.aerosols)
                    && (0.0..=0.2).contains(&s.sun_radius),
                "Invalid sky"
            );
        }
        if let Some(f) = self.fog {
            anyhow::ensure!(
                (0.0..=0.1).contains(&f.extinction)
                    && f.height >= 1.
                    && f.height.is_finite()
                    && f.base.is_finite()
                    && f.tint.iter().all(|c| (0.0..=1.0).contains(c))
                    && (0.0..=1.0).contains(&f.albedo)
                    && (0.0..=0.9).contains(&f.anisotropy),
                "Invalid fog"
            );
        }
        Ok(())
    }
}
