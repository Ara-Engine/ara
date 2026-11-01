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
use ara_components::render::{ExposureMode, SceneLook, SkyAtmosphere, ToneMapping};
use ara_render::look::atmosphere::{Atmosphere as SkyModel, AtmosphereLight, LightAtmosphere};
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
    /// Automatic exposure's compensation by metered brightness (raster renderer,
    /// `ara_render::look::AutoExposure::compensation_curve`); all zero adds none.
    pub compensation_curve: [[f32; 2]; ara_render::look::EXPOSURE_CURVE_KEYS],
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
    /// The colour lookup table graded through last: an index into the resolved
    /// `SceneLook::luts` (raster renderer, `Renderer::set_color_lut`).
    pub lut: Option<u32>,
    /// Share of that table applied, from zero to one.
    pub lut_contribution: f32,
    /// Scale of the sky's light (ambient, reflections, probe bounce) over the sky
    /// drawn (raster renderer, `Look::sky_light`): `Environment.skyLight`, which
    /// `resolve` leaves at 1 for the caller to set.
    pub sky_light: f32,
    /// Scale of the sun's in-scattering in fog (raster renderer, `Look::sun_scattering`):
    /// the directional light's volumetric scattering, which `resolve` leaves at 1 for
    /// the caller to set.
    pub sun_scattering: f32,
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
                    a.model = sky_model(&s);
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
                    a.fog_volumetric = f.volumetric;
                    a.fog_volumetric_distance = f.volumetric_distance.clamp(1., 1000.);
                    a.fog_emissive = f
                        .volumetric_emissive
                        .map(|c| if c.is_finite() { c.max(0.) } else { 0. });
                    a.fog_extinction_scale = f.volumetric_extinction_scale.clamp(0.1, 10.);
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
                compensation_curve: [[0.; 2]; ara_render::look::EXPOSURE_CURVE_KEYS],
                tone_mapper: ToneMapper::Filmic,
                bloom: 0.04,
                grading: Grading::default(),
                display: [[0.; 3], [1.; 3], [1.; 3]],
                lens: Default::default(),
                ao: Default::default(),
                ssr: Default::default(),
                lut: None,
                lut_contribution: 0.,
                sky_light: 1.,
                sun_scattering: 1.,
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
            compensation_curve: ara_render::look::AutoExposure::curve(
                &p.compensation_curve
                    .iter()
                    .map(|k| [k.ev100, k.stops])
                    .collect::<Vec<_>>(),
            ),
            tone_mapper: match p.tone_mapping {
                ToneMapping::Filmic => ToneMapper::Filmic,
                ToneMapping::Neutral => ToneMapper::PbrNeutral,
                ToneMapping::AcesFit => ToneMapper::AcesFit,
                ToneMapping::Agx => ToneMapper::Agx,
                ToneMapping::AgxPunchy => ToneMapper::AgxPunchy,
                ToneMapping::AgxMediumHighContrast => ToneMapper::AgxMediumHighContrast,
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
            // A table the runtime could not load (missing or not a `lut` import)
            // grades as if absent.
            lut: p.lut.as_ref().and_then(|reference| {
                let id = ara_schema::grammar::parse_ref(&reference.0).ok()?.id?;
                let index = look.luts.iter().position(|(known, _)| *known == id)?;
                u32::try_from(index).ok()
            }),
            lut_contribution: p.lut_contribution.clamp(0., 1.),
            sky_light: 1.,
            sun_scattering: 1.,
        }
    }

    /// `base` with this environment's exposure, display and fog controls, and the
    /// volumes' depth of field and motion blur (which switch the fidelity view to its
    /// cinematic quality). Lift/gamma/gain, chromatic aberration, the lookup table and
    /// AO/SSR tuning are raster-only. The homogeneous frustum fog is off: authored fog
    /// is the atmosphere's height fog.
    pub fn post(&self, base: PostSettings) -> PostSettings {
        let lens = self.lens;
        let base = if lens.depth_of_field.is_some() || lens.motion_blur > 0. {
            let focus = lens.depth_of_field;
            PostSettings {
                quality: crate::settings::Quality::Cinematic,
                focus_distance: focus.map_or(base.focus_distance, |d| d.focus_distance),
                // Motion blur alone keeps everything in focus.
                aperture: 0.,
                f_number: focus.map(|d| d.aperture),
                shutter: lens.motion_blur,
                ..base
            }
        } else {
            base
        };
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
    /// transmittance along `direction` (toward the sun) as surfaces receive it (the
    /// global transmittance, or at the ground with the planet's shadow when the sun is
    /// per-pixel), or `lux` unchanged without an atmosphere sun. Use it for lighting
    /// that does not trace the atmosphere (particles).
    pub fn ground_sun(&self, direction: Vec3, lux: Vec3) -> Vec3 {
        let Some((a, sun)) = self
            .atmosphere
            .filter(|a| a.physical_sky)
            .and_then(|a| Some((a, a.sun?)))
        else {
            return lux;
        };
        let d = direction.normalize_or(Vec3::Y);
        if !sun.per_pixel_transmittance {
            return lux * a.model.ground_transmittance(d);
        }
        let r = a.model.bottom_radius;
        let horizon = -(1. - (r / (r + 1.)).powi(2)).max(0.).sqrt();
        let elevation =
            d.y.max(a.model.transmittance_min_light_elevation.to_radians().sin());
        if elevation < horizon {
            return Vec3::ZERO;
        }
        lux * a.model.transmittance(0., d)
    }

    /// Sets the atmosphere lights from the frame's lights: how the sun meets the
    /// atmosphere (`None`: it is not an atmosphere light), whether the sun is the
    /// second atmosphere light (index 1, a moon in the shadowed sun slot), and the other
    /// atmosphere light.
    pub fn set_atmosphere_lights(
        &mut self,
        sun: Option<LightAtmosphere>,
        sun_is_second: bool,
        other: Option<AtmosphereLight>,
    ) {
        if let Some(a) = &mut self.atmosphere {
            a.sun = sun;
            a.sun_is_second = sun_is_second && sun.is_some();
            a.other_light = other;
        }
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
                ToneMapper::Agx => ToneCurve::Agx,
                ToneMapper::AgxPunchy => ToneCurve::AgxPunchy,
                ToneMapper::AgxMediumHighContrast => ToneCurve::AgxMediumHighContrast,
            },
            auto_exposure: self.ev100.is_none().then_some(Metering {
                minimum_ev100: a.minimum_ev100,
                maximum_ev100: a.maximum_ev100,
                low_percentile: a.low_percentile,
                high_percentile: a.high_percentile,
                compensation: self.compensation,
                adaptation: self.adaptation,
                compensation_curve: self.compensation_curve,
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
                lut_contribution: if self.lut.is_some() {
                    self.lut_contribution
                } else {
                    0.
                },
            },
            lens: self.lens,
            ao: self.ao,
            ssr: self.ssr,
            sky_light: self.sky_light,
            sun_scattering: self.sun_scattering,
            sky: self.atmosphere.filter(|a| a.physical_sky).map(|a| Sky {
                sea_level: a.sea_level,
                atmosphere: a.model,
                sun: a.sun,
                sun_is_second: a.sun_is_second,
                other_light: a.other_light,
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
                    volumetric: a.fog_volumetric.then_some(ara_render::look::VolumetricFog {
                        distance: a.fog_volumetric_distance,
                        emissive: a.fog_emissive,
                        extinction_scale: a.fog_extinction_scale,
                    }),
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

/// The scene's local fog volumes for the raster renderer
/// (`ara_render::Renderer::set_fog_volumes`), seen from `camera` (world position):
/// the highest priority first, then the nearest (distance to each box's surface),
/// ties in entity order; at most `ara_render::look::MAX_FOG_VOLUMES`. Volumes with
/// non-finite poses or no extent are skipped; values clamp into their ranges.
pub fn fog_volumes(look: &SceneLook, camera: Vec3) -> Vec<ara_render::look::FogVolume> {
    use ara_components::render::FogVolumeShape;
    use ara_render::look::{FogVolume, FogVolumeShape as Shape, MAX_FOG_VOLUMES};
    let clean = |c: f32| if c.is_finite() { c.max(0.) } else { 0. };
    let mut volumes: Vec<(i32, f32, FogVolume)> = look
        .fog_volumes
        .iter()
        .filter_map(|v| {
            let half = Vec3::from_array(v.volume.size) * Vec3::from_array(v.scale).abs() * 0.5;
            let rotation = glam::Quat::from_array(v.rotation);
            let centre = Vec3::from_array(v.translation);
            let finite = half.is_finite() && centre.is_finite() && rotation.is_finite();
            if !finite || half.min_element() < 0.01 || rotation.length_squared() < 1e-6 {
                return None;
            }
            let rotation = rotation.normalize();
            // Distance from the camera to the oriented box around the volume.
            let local = (rotation.inverse() * (camera - centre)).abs() - half;
            let distance = local.max(Vec3::ZERO).length();
            let [r, g, b, _] = v.volume.albedo.0;
            Some((
                v.volume.priority,
                distance,
                FogVolume {
                    centre: centre.to_array(),
                    rotation: rotation.to_array(),
                    half_extent: half.to_array(),
                    shape: match v.volume.shape {
                        FogVolumeShape::Sphere => Shape::Sphere,
                        FogVolumeShape::Box => Shape::Box,
                    },
                    density: clean(v.volume.density).min(10.),
                    height_falloff: clean(v.volume.height_falloff).min(10.),
                    radial_falloff: clean(v.volume.radial_falloff).min(1.),
                    albedo: [r, g, b].map(|c| clean(c).min(1.)),
                    emissive: v.volume.emissive.map(clean),
                },
            ))
        })
        .collect();
    // Stable: equal keys keep entity order.
    volumes.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)));
    volumes
        .into_iter()
        .take(MAX_FOG_VOLUMES)
        .map(|(_, _, v)| v)
        .collect()
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

/// The renderers' atmosphere for an authored `SkyAtmosphere`: Unreal's parameters one
/// to one (kilometres to metres, per-kilometre coefficients times their colours to
/// per metre), clamped into the component's ranges.
pub fn sky_model(s: &SkyAtmosphere) -> SkyModel {
    let finite = |v: f32, or: f32| if v.is_finite() { v } else { or };
    let colour = |c: [f32; 3]| c.map(|x| finite(x, 0.).max(0.));
    let coefficient =
        |scale: f32, c: [f32; 3]| colour(c).map(|x| x * finite(scale, 0.).max(0.) * 1e-3);
    let km = |v: f32, lo: f32, hi: f32| finite(v, lo).clamp(lo, hi) * 1000.;
    SkyModel {
        bottom_radius: km(s.bottom_radius, 0.1, 10000.),
        height: km(s.atmosphere_height, 0.1, 10000.),
        ground_albedo: colour(s.ground_albedo).map(|x| x.min(1.)),
        multi_scattering: finite(s.multi_scattering, 1.).clamp(0., 100.),
        trace_sample_count_scale: finite(s.trace_sample_count_scale, 1.).clamp(0.25, 8.),
        rayleigh_scattering: coefficient(s.rayleigh_scattering_scale, s.rayleigh_scattering),
        rayleigh_scale_height: km(s.rayleigh_exponential_distribution, 0.001, 1000.),
        mie_scattering: coefficient(s.mie_scattering_scale, s.mie_scattering),
        mie_absorption: coefficient(s.mie_absorption_scale, s.mie_absorption),
        mie_anisotropy: finite(s.mie_anisotropy, 0.8).clamp(0., 0.999),
        mie_scale_height: km(s.mie_exponential_distribution, 0.001, 1000.),
        absorption: coefficient(s.absorption_scale, s.absorption),
        absorption_tip_altitude: finite(s.absorption_tip_altitude, 25.) * 1000.,
        absorption_tip_value: finite(s.absorption_tip_value, 1.).max(0.),
        absorption_width: km(s.absorption_width, 0., 1000.),
        sky_luminance_factor: colour(s.sky_luminance_factor),
        sky_and_aerial_perspective_luminance_factor: colour(
            s.sky_and_aerial_perspective_luminance_factor,
        ),
        aerial_perspective_view_distance_scale: finite(
            s.aerial_perspective_view_distance_scale,
            1.,
        )
        .clamp(0., 1000.),
        aerial_perspective_start_depth: km(s.aerial_perspective_start_depth, 0., 10000.),
        height_fog_contribution: finite(s.height_fog_contribution, 1.).clamp(0., 16.),
        transmittance_min_light_elevation: finite(s.transmittance_min_light_elevation_angle, -90.)
            .clamp(-90., 90.),
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
                mie_scattering_scale: 0.012,
                bottom_radius: 3390.,
                ..Default::default()
            }),
            fog: Some(HeightFog {
                density: 0.01,
                base_height: 4.,
                ..Default::default()
            }),
            fog_volumes: Vec::new(),
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
            luts: Vec::new(),
        };
        let env = FrameEnvironment::resolve(&look, Vec3::ZERO, 1.);
        let a = env.atmosphere.unwrap();
        assert!(a.physical_sky && a.fog_extinction == 0.01 && a.fog_base == 4.);
        // Unreal's units map one to one: kilometres to metres, 1/km x colour to 1/m.
        assert_eq!(a.model.bottom_radius, 3_390_000.);
        assert!((a.model.mie_scattering[0] - 12e-6).abs() < 1e-12);
        assert_eq!(
            a.model,
            SkyModel {
                bottom_radius: 3_390_000.,
                mie_scattering: [12e-6; 3],
                ..sky_model(&SkyAtmosphere::default())
            }
        );
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
    fn default_component_is_unreal_earth() {
        let model = sky_model(&SkyAtmosphere::default());
        let earth = SkyModel::EARTH;
        let close = |a: &[f32], b: &[f32]| {
            a.iter()
                .zip(b)
                .all(|(x, y)| (x - y).abs() <= 1e-4 * y.abs().max(1e-9))
        };
        assert!(
            close(&model.rayleigh_scattering, &earth.rayleigh_scattering),
            "{model:?}"
        );
        assert!(close(&model.absorption, &earth.absorption));
        assert!(close(&model.mie_scattering, &earth.mie_scattering));
        assert!(close(&model.mie_absorption, &earth.mie_absorption));
        assert_eq!(
            (
                model.bottom_radius,
                model.height,
                model.aerial_perspective_start_depth
            ),
            (
                earth.bottom_radius,
                earth.height,
                earth.aerial_perspective_start_depth
            )
        );
        model.validate().unwrap();
    }

    #[test]
    fn raster_look_matches_the_fidelity_mapping() {
        let look = SceneLook {
            sky: Some(SkyAtmosphere {
                multi_scattering: 0.5,
                ..Default::default()
            }),
            fog: Some(HeightFog {
                density: 0.01,
                ..Default::default()
            }),
            volumes: volume(Vec::new()),
            luts: Vec::new(),
            fog_volumes: Vec::new(),
        };
        let env = FrameEnvironment::resolve(&look, Vec3::ZERO, 1.);
        let raster = env.raster();
        raster.validate().unwrap();
        assert_eq!(raster.sky.map(|s| s.atmosphere.multi_scattering), Some(0.5));
        assert_eq!(raster.fog.map(|f| f.extinction), Some(0.01));
        assert_eq!(raster.auto_exposure.is_some(), env.ev100.is_none());
        let legacy = FrameEnvironment::resolve(&SceneLook::default(), Vec3::ZERO, 0.0015);
        let raster = legacy.raster();
        assert!(raster.sky.is_none() && raster.fog.is_none() && raster.auto_exposure.is_none());
        assert!((legacy.manual_exposure(1.) - 0.0015).abs() < 1e-7);
    }

    #[test]
    fn volume_lens_effects_map_onto_both_renderers() {
        let look = SceneLook {
            volumes: volume(vec![
                PostOverride {
                    module: PostModule::DepthOfField,
                    focus_distance: Some(8.),
                    aperture: Some(2.),
                    ..Default::default()
                },
                PostOverride {
                    module: PostModule::MotionBlur,
                    shutter_angle: Some(180.),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let env = FrameEnvironment::resolve(&look, Vec3::ZERO, 1.);
        let raster = env.raster();
        raster.validate().unwrap();
        let focus = raster.lens.depth_of_field.unwrap();
        assert_eq!((focus.focus_distance, focus.aperture), (8., 2.));
        assert_eq!(raster.lens.motion_blur, 0.5);
        let post = env.post(PostSettings::default());
        assert_eq!(post.quality, crate::settings::Quality::Cinematic);
        assert_eq!(
            (post.focus_distance, post.f_number, post.shutter),
            (8., Some(2.), 0.5)
        );
        // Without lens effects the host's own lens settings stay as they were.
        let plain = FrameEnvironment::resolve(&SceneLook::default(), Vec3::ZERO, 1.);
        assert_eq!(
            plain.post(PostSettings::default()).quality,
            crate::settings::Quality::Gameplay
        );
    }
}
