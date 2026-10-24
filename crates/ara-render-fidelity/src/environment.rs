//! The scene's authored look (`ara_components::render`: `SkyAtmosphere`, `HeightFog`,
//! `PostProcess`) mapped onto fidelity view inputs, and the matching approximation
//! for the baseline raster renderer: a CPU-integrated sky from the same atmosphere
//! model and sun, the same exposure and the same tone mapper.
//!
//! Hosts call [`FrameEnvironment::resolve`] when the extraction's look changes, then
//! [`FidelityView::set_environment`] (fidelity) or [`FrameEnvironment::baseline`]
//! (raster) at the frame boundary.
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
}

impl FrameEnvironment {
    /// Maps the authored look. Without a `PostProcess`, exposure is the legacy
    /// `Environment.exposure` linear multiplier (as a manual EV100) with Filmic
    /// tone mapping, so existing scenes keep their brightness.
    pub fn resolve(look: &SceneLook, fallback_exposure: f32) -> Self {
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
        let Some(p) = look.post else {
            return Self {
                atmosphere,
                ev100: Some(ev100_from_exposure(fallback_exposure)),
                compensation: 0.,
                adaptation: 2.,
                auto_exposure: AutoExposure::default(),
                tone_mapper: ToneMapper::Filmic,
                bloom: 0.04,
                grading: Grading::default(),
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

    /// The raster approximation for the same sun (`direction` towards the sun,
    /// illuminance in lux): a physical sky map when the look has a `SkyAtmosphere`,
    /// a linear exposure and the tone mapper.
    pub fn baseline(&self, sun_direction: Vec3, sun_lux: Vec3) -> BaselineEnvironment {
        let sky = self
            .atmosphere
            .filter(|a| a.physical_sky)
            .map(|a| sky_map(&a, sun_direction.normalize_or(Vec3::Y), sun_lux));
        let ev100 = self.ev100.unwrap_or_else(|| {
            // Grey-world metering of an 18% surface under the sun and sky, the
            // same target the GPU histogram converges to in an open scene.
            let sky_lux = sky.as_ref().map_or(0., |(_, _, lux)| *lux);
            let sun =
                crate::colour::luminance(sun_lux) * sun_direction.normalize_or(Vec3::Y).y.max(0.);
            let luminance = 0.18 * (sun + sky_lux) / std::f32::consts::PI;
            (luminance.max(1e-6) / (0.18 * 1.2)).log2().clamp(
                self.auto_exposure.minimum_ev100,
                self.auto_exposure.maximum_ev100,
            )
        });
        BaselineEnvironment {
            sky: sky.map(|(size, pixels, _)| (size, pixels)),
            exposure: crate::settings::exposure_from_ev100(ev100, self.compensation),
            tone_mapper: self.tone_mapper,
        }
    }
}

/// What the baseline renderer needs from the look.
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineEnvironment {
    /// Equirectangular sky radiance in cd/m² (`size`, row-major RGBA), replacing the
    /// `Environment` gradient; `None` keeps the gradient.
    pub sky: Option<([u32; 2], Vec<[f32; 4]>)>,
    /// Linear exposure multiplier for the baseline tone map pass.
    pub exposure: f32,
    /// Display curve.
    pub tone_mapper: ToneMapper,
}

impl FidelityView {
    /// Applies an authored environment at a frame boundary: atmosphere and fog
    /// (resetting temporal transport when they change) and exposure/display controls.
    pub fn set_environment(&mut self, environment: &FrameEnvironment) -> Result<()> {
        self.set_atmosphere(environment.atmosphere)?;
        self.set_post(environment.post(self.settings.post))
    }
}

/// EV100 of a linear exposure multiplier (inverse of `exposure_from_ev100`).
pub fn ev100_from_exposure(exposure: f32) -> f32 {
    (1. / (1.2 * exposure.max(1e-12))).log2().clamp(-24., 32.)
}

const PLANET: f32 = 6_360_000.;
const TOP: f32 = 6_460_000.;
const SKY_SIZE: [u32; 2] = [128, 64];

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

/// CPU twin of the GPU transmittance and multiple-scattering LUTs.
struct Luts {
    aerosols: f32,
    transmittance: Vec<Vec3>,
    multiple: Vec<Vec3>,
}
const T_SIZE: [usize; 2] = [64, 32];
const M_SIZE: usize = 16;

impl Luts {
    fn new(aerosols: f32) -> Self {
        let mut luts = Self {
            aerosols,
            transmittance: Vec::with_capacity(T_SIZE[0] * T_SIZE[1]),
            multiple: Vec::new(),
        };
        for y in 0..T_SIZE[1] {
            for x in 0..T_SIZE[0] {
                let (h, mu) = lut_coordinates(x, y, T_SIZE[0], T_SIZE[1]);
                let p = Vec3::new(0., PLANET + h, 0.);
                let d = Vec3::new((1. - mu * mu).max(0.).sqrt(), mu, 0.);
                let (extent, ground) = atmosphere_length(p, d);
                let mut optical = Vec3::ZERO;
                for i in 0..40 {
                    let at = p + d * (extent * (i as f32 + 0.5) / 40.);
                    optical += extinction(at.length() - PLANET, aerosols).2 * (extent / 40.);
                }
                luts.transmittance
                    .push(if ground { Vec3::ZERO } else { (-optical).exp() });
            }
        }
        let mut multiple = Vec::with_capacity(M_SIZE * M_SIZE);
        for y in 0..M_SIZE {
            for x in 0..M_SIZE {
                let (h, mu) = lut_coordinates(x, y, M_SIZE, M_SIZE);
                let sun = Vec3::new((1. - mu * mu).max(0.).sqrt(), mu, 0.);
                let p = Vec3::new(0., PLANET + h + 1., 0.);
                let (mut incoming, mut feedback) = (Vec3::ZERO, Vec3::ZERO);
                for j in 0..16 {
                    let yy = 1. - 2. * (j as f32 + 0.5) / 16.;
                    let phi = j as f32 * 2.399963;
                    let r = (1. - yy * yy).sqrt();
                    let d = Vec3::new(r * phi.cos(), yy, r * phi.sin());
                    let (extent, _) = atmosphere_length(p, d);
                    incoming += luts.integrate(p, d, extent, sun, 12, false).0;
                    let (mut path, mut scatter) = (Vec3::ONE, Vec3::ZERO);
                    let ds = extent / 12.;
                    for i in 0..12 {
                        let (sr, sm, e) = extinction(
                            (p + d * (ds * (i as f32 + 0.5))).length() - PLANET,
                            aerosols,
                        );
                        let segment = (-e * ds).exp();
                        scatter += path * (sr + Vec3::splat(sm)) * (Vec3::ONE - segment)
                            / e.max(Vec3::splat(1e-9));
                        path *= segment;
                    }
                    feedback += scatter;
                }
                multiple
                    .push((incoming / 16.) / (Vec3::ONE - feedback / 16.).max(Vec3::splat(0.05)));
            }
        }
        luts.multiple = multiple;
        luts
    }

    fn sample(table: &[Vec3], size: [usize; 2], height: f32, cosine: f32) -> Vec3 {
        let u = (cosine * 0.5 + 0.5).clamp(0., 1.) * size[0] as f32 - 0.5;
        let v = (height / 100_000.).clamp(0., 1.).sqrt() * size[1] as f32 - 0.5;
        let (x0, y0) = (u.floor(), v.floor());
        let (ax, ay) = (u - x0, v - y0);
        let at = |x: f32, y: f32| {
            let x = (x as i32).clamp(0, size[0] as i32 - 1) as usize;
            let y = (y as i32).clamp(0, size[1] as i32 - 1) as usize;
            table[y * size[0] + x]
        };
        at(x0, y0)
            .lerp(at(x0 + 1., y0), ax)
            .lerp(at(x0, y0 + 1.).lerp(at(x0 + 1., y0 + 1.), ax), ay)
    }

    fn transmittance(&self, height: f32, cosine: f32) -> Vec3 {
        Self::sample(&self.transmittance, T_SIZE, height, cosine)
    }

    fn multiple_scattering(&self, height: f32, cosine: f32) -> Vec3 {
        Self::sample(&self.multiple, [M_SIZE, M_SIZE], height, cosine)
    }

    /// Matches `integrateSky` for unit sun illuminance; returns radiance and throughput.
    fn integrate(
        &self,
        p: Vec3,
        d: Vec3,
        distance: f32,
        sun: Vec3,
        steps: u32,
        multiple: bool,
    ) -> (Vec3, Vec3) {
        let (mut sum, mut throughput, mut previous) = (Vec3::ZERO, Vec3::ONE, 0.);
        let cosine = d.dot(sun);
        for i in 0..steps {
            let end = distance * ((i + 1) as f32 / steps as f32).powi(2);
            let ds = end - previous;
            let at = p + d * (previous + ds * 0.5);
            previous = end;
            let height = (at.length() - PLANET).max(0.);
            let mu = at.normalize().dot(sun);
            let (sr, sm, e) = extinction(height, self.aerosols);
            let segment = (-e * ds).exp();
            let mut source = self.transmittance(height, mu)
                * (sr * phase_rayleigh(cosine) + Vec3::splat(sm * phase_mie(cosine, 0.8)));
            if multiple {
                source += (sr + Vec3::splat(sm)) * self.multiple_scattering(height, mu);
            }
            sum += throughput * source * (Vec3::ONE - segment) / e.max(Vec3::splat(1e-9));
            throughput *= segment;
        }
        (sum, throughput)
    }
}

fn lut_coordinates(x: usize, y: usize, w: usize, h: usize) -> (f32, f32) {
    let u = (x as f32 + 0.5) / w as f32;
    let v = (y as f32 + 0.5) / h as f32;
    (v * v * 100_000., u * 2. - 1.)
}

fn phase_rayleigh(c: f32) -> f32 {
    3. * (1. + c * c) / (16. * std::f32::consts::PI)
}

fn phase_mie(c: f32, g: f32) -> f32 {
    (1. - g * g) / (4. * std::f32::consts::PI * (1. + g * g - 2. * g * c).max(0.001).powf(1.5))
}

/// Equirectangular sky radiance (cd/m²) seen from 2 m above sea level, with the
/// GPU sky-view's ground bounce, plus the horizontal sky illuminance (lux).
fn sky_map(atmosphere: &Atmosphere, sun: Vec3, lux: Vec3) -> ([u32; 2], Vec<[f32; 4]>, f32) {
    let luts = Luts::new(atmosphere.aerosols);
    let p = Vec3::new(0., PLANET + 2., 0.);
    let [w, h] = SKY_SIZE;
    let mut pixels = Vec::with_capacity((w * h) as usize);
    let mut illuminance = 0.;
    for y in 0..h {
        let theta = (y as f32 + 0.5) / h as f32 * std::f32::consts::PI;
        for x in 0..w {
            let phi = ((x as f32 + 0.5) / w as f32 - 0.5) * std::f32::consts::TAU;
            let d = Vec3::new(
                phi.cos() * theta.sin(),
                theta.cos(),
                phi.sin() * theta.sin(),
            );
            let (extent, ground) = atmosphere_length(p, d);
            let (mut light, t) = luts.integrate(p, d, extent, sun, 24, true);
            if ground {
                let n = (p + d * extent).normalize();
                let mu = n.dot(sun);
                light += t
                    * 0.15
                    * (luts.transmittance(0., mu) * mu.max(0.)
                        + luts.multiple_scattering(0., mu) * std::f32::consts::PI)
                    / std::f32::consts::PI;
            }
            let radiance = light * lux;
            // Solid angle of an equirect texel times the cosine to +Y.
            let solid = (std::f32::consts::TAU / w as f32)
                * (std::f32::consts::PI / h as f32)
                * theta.sin();
            illuminance += crate::colour::luminance(radiance) * d.y.max(0.) * solid;
            pixels.push([
                radiance.x.max(0.),
                radiance.y.max(0.),
                radiance.z.max(0.),
                1.,
            ]);
        }
    }
    (SKY_SIZE, pixels, illuminance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_components::render::{HeightFog, PostProcess, SkyAtmosphere};

    #[test]
    fn empty_look_keeps_legacy_exposure_and_no_atmosphere() {
        let env = FrameEnvironment::resolve(&SceneLook::default(), 0.0015);
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
            post: Some(PostProcess {
                exposure: ExposureMode::Manual,
                ev100: 12.,
                tone_mapping: ToneMapping::Neutral,
                ..Default::default()
            }),
        };
        let env = FrameEnvironment::resolve(&look, 1.);
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
            1.,
        );
        assert!(!fog_only.atmosphere.unwrap().physical_sky);
    }

    #[test]
    fn baseline_sky_is_blue_at_noon_and_warm_at_sunset() {
        let env = FrameEnvironment::resolve(
            &SceneLook {
                sky: Some(SkyAtmosphere::default()),
                post: Some(PostProcess::default()),
                ..Default::default()
            },
            1.,
        );
        let lux = Vec3::splat(110_000.);
        let noon = env.baseline(Vec3::Y, lux);
        let ([w, h], pixels) = noon.sky.clone().unwrap();
        let zenith = pixels[(w / 2) as usize];
        assert!(zenith[2] > zenith[0] * 1.5, "{zenith:?}");
        let sunset = env.baseline(Vec3::new(1., 0.03, 0.).normalize(), lux);
        let (_, pixels) = sunset.sky.unwrap();
        // Horizon texel facing the sun (+X is u = 0.5).
        let horizon = pixels[((h / 2 - 1) * w + w / 2) as usize];
        assert!(horizon[0] > horizon[2], "{horizon:?}");
        // Auto exposure: a sunny day meters brighter (higher EV) than sunset.
        assert!(noon.exposure < sunset.exposure);
        let ev = ev100_from_exposure(noon.exposure);
        assert!((13.0..17.0).contains(&ev), "{ev}");
    }
}
