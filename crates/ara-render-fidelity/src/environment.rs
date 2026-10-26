//! The scene's authored look (`ara_components::render`: `SkyAtmosphere`, `HeightFog`,
//! and the post-process volumes blended for the camera) mapped onto fidelity view
//! inputs, and the matching approximation for the raster renderer
//! (`ara_render::look::Look`): the same GPU sky, fog, exposure, grading and tone mapper.
//!
//! Hosts call [`FrameEnvironment::resolve`] each frame with the position of the camera
//! they draw from (volumes blend by it), then [`FidelityView::set_environment`]
//! (fidelity) or [`FrameEnvironment::raster`] (raster) at the frame boundary.
use crate::{
    FidelityView,
    atmosphere::Atmosphere,
    colour::{AutoExposure, Grading, ToneMapper},
    settings::PostSettings,
};
use anyhow::Result;
use ara_components::render::{ExposureMode, SceneLook, ToneMapping};
use glam::Vec3;

/// Fidelity atmosphere and post controls derived from a scene look. Lens and
/// cinematic controls (focus, aperture, shutter, quality) stay with the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameEnvironment {
    /// Physical sky and/or height fog; `None` keeps the environment map unfogged.
    pub atmosphere: Option<Atmosphere>,
    /// Manual EV100, or `None` to meter automatically.
    pub ev100: Option<f32>,
    /// Exposure compensation in stops.
    pub compensation: f32,
    /// Automatic adaptation rate, per second.
    pub adaptation: f32,
    /// Automatic metering limits.
    pub auto_exposure: AutoExposure,
    /// Display curve.
    pub tone_mapper: ToneMapper,
    /// Bloom energy fraction.
    pub bloom: f32,
    /// Scene-linear grading.
    pub grading: Grading,
    /// Display-space lift, gamma and gain after the tone curve (raster renderer).
    pub display: [[f32; 3]; 3],
    /// Chromatic aberration, depth of field and motion blur (raster renderer).
    pub lens: ara_render::look::Lens,
    /// Screen-space ambient occlusion tuning over the preset's (raster renderer).
    pub ao: ara_render::look::AoTuning,
    /// Screen-space reflection tuning over the preset's (raster renderer).
    pub ssr: ara_render::look::SsrTuning,
}

impl FrameEnvironment {
    /// Maps the authored look for a camera at `camera` (world position): post-process
    /// volumes blend by it. Without any volume, exposure is the legacy
    /// `Environment.exposure` linear multiplier (as a manual EV100) with Filmic tone
    /// mapping, so existing scenes keep their brightness.
    pub fn resolve(look: &SceneLook, camera: Vec3, fallback_exposure: f32) -> Self {
        let atmosphere = match (look.sky, look.fog) {
            (None, None) => None,
            (sky, fog) => {
                let mut a = Atmosphere {
                    fog_extinction: 0.,
                    cloud_coverage: 0.,
                    physical_sky: sky.is_some(),
                    fog_albedo: 1.,
                    ..Atmosphere::default()
                };
                if let Some(s) = sky {
                    a.sea_level = s.sea_level;
                    a.aerosols = s.aerosols.clamp(0., 20.);
                    a.cloud_coverage = s.cloud_coverage.clamp(0., 1.);
                    a.cloud_height = s.cloud_height.max(500.);
                }
                if let Some(f) = fog {
                    a.fog_extinction = f.density.clamp(0., 0.1);
                    a.fog_height = f.falloff.max(1.);
                    a.fog_base = f.base_height;
                    a.fog_tint =
                        [f.albedo.0[0], f.albedo.0[1], f.albedo.0[2]].map(|c| c.clamp(0., 1.));
                    a.fog_anisotropy = f.anisotropy.clamp(0., 0.9);
                }
                Some(a)
            }
        };
        let Some(p) = look.post_at(camera.to_array()) else {
            return Self {
                atmosphere,
                ev100: Some(ev100_from_exposure(fallback_exposure)),
                compensation: 0.,
                adaptation: 2.,
                auto_exposure: AutoExposure::default(),
                tone_mapper: ToneMapper::Filmic,
                bloom: 0.04,
                grading: Grading::default(),
                display: [[0.; 3], [1.; 3], [1.; 3]],
                lens: Default::default(),
                ao: Default::default(),
                ssr: Default::default(),
            };
        };
        let (low, high) = (p.min_ev100.min(p.max_ev100), p.min_ev100.max(p.max_ev100));
        Self {
            atmosphere,
            ev100: match p.exposure {
                ExposureMode::Auto => None,
                ExposureMode::Manual => Some(p.ev100.clamp(-24., 32.)),
            },
            compensation: p.compensation.clamp(-16., 16.),
            adaptation: p.adaptation_speed.max(0.01),
            auto_exposure: AutoExposure {
                minimum_ev100: low.max(-24.),
                maximum_ev100: high.min(32.).max(low.max(-24.) + 0.01),
                ..AutoExposure::default()
            },
            tone_mapper: match p.tone_mapping {
                ToneMapping::Filmic => ToneMapper::Filmic,
                ToneMapping::Neutral => ToneMapper::PbrNeutral,
                ToneMapping::AcesFit => ToneMapper::AcesFit,
            },
            bloom: p.bloom.clamp(0., 0.25),
            grading: Grading {
                white_balance: p.white_balance.clamp(1000., 40000.),
                saturation: p.saturation.clamp(0., 2.),
                contrast: p.contrast.clamp(0.5, 2.),
                vignette: p.vignette.clamp(0., 1.),
            },
            display: [
                p.lift.map(|c| c.clamp(-1., 1.)),
                p.gamma.map(|c| c.clamp(0.01, 10.)),
                p.gain.map(|c| c.clamp(0., 10.)),
            ],
            lens: lens(&p),
            ao: ara_render::look::AoTuning {
                intensity: p.ao_intensity.clamp(0., 4.),
                radius: over_preset(p.ao_radius),
            },
            ssr: ara_render::look::SsrTuning {
                intensity: p.reflection_intensity.clamp(0., 1.),
                distance: over_preset(p.reflection_distance),
            },
        }
    }

    /// `base` with this environment's exposure, display and fog controls. The
    /// homogeneous frustum fog is off: authored fog is the atmosphere's height fog.
    pub fn post(&self, base: PostSettings) -> PostSettings {
        PostSettings {
            ev100: self.ev100,
            compensation: self.compensation,
            adaptation: self.adaptation,
            auto_exposure: self.auto_exposure,
            tone_mapper: self.tone_mapper,
            bloom: self.bloom,
            grading: self.grading,
            fog_density: 0.,
            ..base
        }
    }

    /// Sun illuminance reaching the ground: `lux` attenuated by the physical sky's
    /// transmittance along `direction` (toward the sun), or `lux` unchanged without
    /// one. Use it for lighting that does not trace the atmosphere (particles).
    pub fn ground_sun(&self, direction: Vec3, lux: Vec3) -> Vec3 {
        let Some(a) = self.atmosphere.filter(|a| a.physical_sky) else {
            return lux;
        };
        let p = Vec3::new(0., PLANET + 2., 0.);
        let d = direction.normalize_or(Vec3::Y);
        let (extent, ground) = atmosphere_length(p, d);
        if ground {
            return Vec3::ZERO;
        }
        let mut optical = Vec3::ZERO;
        for i in 0..64 {
            let at = p + d * (extent * (i as f32 + 0.5) / 64.);
            optical += extinction(at.length() - PLANET, a.aerosols).2 * (extent / 64.);
        }
        lux * (-optical).exp()
    }

    /// The same look for the raster renderer (`ara_render::Renderer::set_look`):
    /// the physical sky (with its cloud layer) and height fog run on the GPU there too.
    pub fn raster(&self) -> ara_render::look::Look {
        use ara_render::look::{
            AutoExposure as Metering, Fog, Grading as RasterGrading, Look, Sky, ToneCurve,
        };
        let a = self.auto_exposure;
        Look {
            curve: match self.tone_mapper {
                ToneMapper::AcesFit => ToneCurve::AcesFit,
                ToneMapper::PbrNeutral => ToneCurve::Neutral,
                ToneMapper::Filmic => ToneCurve::Filmic,
            },
            auto_exposure: self.ev100.is_none().then_some(Metering {
                minimum_ev100: a.minimum_ev100,
                maximum_ev100: a.maximum_ev100,
                low_percentile: a.low_percentile,
                high_percentile: a.high_percentile,
                compensation: self.compensation,
                adaptation: self.adaptation,
            }),
            bloom: self.bloom,
            grading: RasterGrading {
                white_balance_gain: self.grading.white_balance_gain().to_array(),
                contrast: self.grading.contrast,
                saturation: self.grading.saturation,
                vignette: self.grading.vignette,
                lift: self.display[0],
                gamma: self.display[1],
                gain: self.display[2],
            },
            lens: self.lens,
            ao: self.ao,
            ssr: self.ssr,
            sky: self.atmosphere.filter(|a| a.physical_sky).map(|a| Sky {
                sea_level: a.sea_level,
                aerosols: a.aerosols,
                cloud_coverage: a.cloud_coverage,
                cloud_height: a.cloud_height,
                ..Sky::default()
            }),
            fog: self
                .atmosphere
                .filter(|a| a.fog_extinction > 0.)
                .map(|a| Fog {
                    extinction: a.fog_extinction,
                    height: a.fog_height,
                    base: a.fog_base,
                    tint: a.fog_tint,
                    albedo: a.fog_albedo,
                    anisotropy: a.fog_anisotropy,
                }),
        }
    }

    /// Linear exposure for a manual look (`fallback` under automatic metering,
    /// where the raster renderer meters itself and only captures use it).
    pub fn manual_exposure(&self, fallback: f32) -> f32 {
        self.ev100.map_or(fallback, |ev| {
            crate::settings::exposure_from_ev100(ev, self.compensation)
        })
    }
}

impl FidelityView {
    /// Applies an authored environment at a frame boundary: atmosphere and fog
    /// (resetting temporal transport when they change) and exposure/display controls.
    pub fn set_environment(&mut self, environment: &FrameEnvironment) -> Result<()> {
        self.set_atmosphere(environment.atmosphere)?;
        self.set_post(environment.post(self.settings.post))
    }
}

/// A volume blend's lens effects: depth of field where the focus distance is positive,
/// motion blur for an open shutter (angle over 360°).
fn lens(p: &ara_components::post::PostValues) -> ara_render::look::Lens {
    ara_render::look::Lens {
        chromatic_aberration: p.chromatic_aberration.clamp(0., 1.),
        depth_of_field: (p.focus_distance > 0. && p.focus_distance.is_finite()).then(|| {
            ara_render::look::DepthOfField {
                focus_distance: p.focus_distance,
                aperture: p.aperture.clamp(0.7, 32.),
                near: p.near_blur.clamp(0., 1.),
                far: p.far_blur.clamp(0., 1.),
            }
        }),
        motion_blur: (p.shutter_angle / 360.).clamp(0., 1.),
    }
}

/// A volume blend's preset-relative value (finite, non-negative share).
fn over_preset(v: ara_components::post::PresetBlend) -> ara_render::look::OverPreset {
    let finite = |x: f32, or: f32| if x.is_finite() { x } else { or };
    ara_render::look::OverPreset {
        keep: finite(v.keep, 1.).max(0.),
        add: finite(v.add, 0.),
    }
}

/// EV100 of a linear exposure multiplier (inverse of `exposure_from_ev100`).
pub fn ev100_from_exposure(exposure: f32) -> f32 {
    (1. / (1.2 * exposure.max(1e-12))).log2().clamp(-24., 32.)
}

const PLANET: f32 = 6_360_000.;
const TOP: f32 = 6_460_000.;

/// Matches `extinction` in atmosphere.slangh.
fn extinction(height: f32, aerosols: f32) -> (Vec3, f32, Vec3) {
    let h = height.max(0.);
    let rayleigh = Vec3::new(5.802e-6, 13.558e-6, 33.1e-6) * (-h / 8000.).exp();
    let mie = 3.996e-6 * (-h / 1200.).exp() * aerosols;
    let ozone = (1. - (h - 25000.).abs() / 15000.).max(0.);
    (
        rayleigh,
        mie,
        rayleigh + Vec3::splat(mie / 0.9) + Vec3::new(0.650e-6, 1.881e-6, 0.085e-6) * ozone,
    )
}

fn sphere_exit(p: Vec3, d: Vec3, radius: f32) -> f32 {
    let b = p.dot(d);
    let q = b * b - p.dot(p) + radius * radius;
    if q < 0. { 0. } else { (-b + q.sqrt()).max(0.) }
}

/// Distance to the top of the atmosphere or the ground, and whether it hits ground.
fn atmosphere_length(p: Vec3, d: Vec3) -> (f32, bool) {
    let top = sphere_exit(p, d, TOP);
    let b = p.dot(d);
    let q = b * b - p.dot(p) + PLANET * PLANET;
    if q > 0. && -b - q.sqrt() > 0. {
        ((-b - q.sqrt()).min(top), true)
    } else {
        (top, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_components::post::{PostModule, PostOverride, PostProcessVolume, PostVolume};
    use ara_components::render::{HeightFog, SkyAtmosphere};

    /// A global volume with the given overrides.
    fn volume(overrides: Vec<PostOverride>) -> Vec<PostVolume> {
        vec![PostVolume {
            volume: PostProcessVolume {
                overrides,
                ..Default::default()
            },
            translation: [0.; 3],
            rotation: [0., 0., 0., 1.],
            scale: [1.; 3],
        }]
    }

    #[test]
    fn empty_look_keeps_legacy_exposure_and_no_atmosphere() {
        let env = FrameEnvironment::resolve(&SceneLook::default(), Vec3::ZERO, 0.0015);
        assert_eq!(env.atmosphere, None);
        let ev = env.ev100.unwrap();
        assert!((crate::settings::exposure_from_ev100(ev, 0.) - 0.0015).abs() < 1e-7);
    }

    #[test]
    fn components_map_onto_view_controls() {
        let look = SceneLook {
            sky: Some(SkyAtmosphere {
                aerosols: 3.,
                ..Default::default()
            }),
            fog: Some(HeightFog {
                density: 0.01,
                base_height: 4.,
                ..Default::default()
            }),
            volumes: volume(vec![
                PostOverride {
                    module: PostModule::Exposure,
                    exposure: Some(ExposureMode::Manual),
                    ev100: Some(12.),
                    ..Default::default()
                },
                PostOverride {
                    module: PostModule::ToneMapping,
                    tone_mapping: Some(ToneMapping::Neutral),
                    ..Default::default()
                },
            ]),
        };
        let env = FrameEnvironment::resolve(&look, Vec3::ZERO, 1.);
        let a = env.atmosphere.unwrap();
        assert!(a.physical_sky && a.aerosols == 3. && a.fog_extinction == 0.01 && a.fog_base == 4.);
        a.validate().unwrap();
        let post = env.post(PostSettings::default());
        assert_eq!(
            (post.ev100, post.tone_mapper),
            (Some(12.), ToneMapper::PbrNeutral)
        );
        let fog_only = FrameEnvironment::resolve(
            &SceneLook {
                fog: look.fog,
                ..Default::default()
            },
            Vec3::ZERO,
            1.,
        );
        assert!(!fog_only.atmosphere.unwrap().physical_sky);
    }

    #[test]
    fn raster_look_matches_the_fidelity_mapping() {
        let look = SceneLook {
            sky: Some(SkyAtmosphere {
                aerosols: 3.,
                ..Default::default()
            }),
            fog: Some(HeightFog {
                density: 0.01,
                ..Default::default()
            }),
            volumes: volume(Vec::new()),
        };
        let env = FrameEnvironment::resolve(&look, Vec3::ZERO, 1.);
        let raster = env.raster();
        raster.validate().unwrap();
        assert_eq!(raster.sky.map(|s| s.aerosols), Some(3.));
        assert_eq!(raster.fog.map(|f| f.extinction), Some(0.01));
        assert_eq!(raster.auto_exposure.is_some(), env.ev100.is_none());
        let legacy = FrameEnvironment::resolve(&SceneLook::default(), Vec3::ZERO, 0.0015);
        let raster = legacy.raster();
        assert!(raster.sky.is_none() && raster.fog.is_none() && raster.auto_exposure.is_none());
        assert!((legacy.manual_exposure(1.) - 0.0015).abs() < 1e-7);
    }
}
