//! Ara's extracted camera, sun, sky, local lights, exposure and display as
//! Renderlab engine values. Pure conversions; `bridge` applies them.
use crate::gaps::Gaps;
use anyhow::Result;
use ara_render_fidelity::ToneMapper;
use ara_render_fidelity::environment::FrameEnvironment;
use ara_runtime::{
    components::LightKind,
    extract::{LightView, RenderFrame},
};
use ara_runtime_render::{ExternalView, fidelity::EnvironmentMap};
use renderlab::engine::{
    Camera, DepthOfField, Exposure, Light, PostSettings, ReflectionSettings, Settings, Sun,
    Tonemapper, Vec3,
};
use std::{borrow::Cow, f32::consts::PI};

/// Where the camera looks from when the world has none (as `ara-runtime-render`).
const OVERVIEW: [f32; 3] = [12.0, 10.0, 12.0];
/// Clip planes in metres: Ara's extraction carries none.
const NEAR: f32 = 0.05;
const FAR: f32 = 4000.0;

/// The frame's camera, or an overview of the origin.
pub(crate) fn camera(frame: &RenderFrame) -> Result<Camera> {
    let (eye, forward, fov) = match frame.camera {
        Some(c) => (
            Vec3::from_array(c.position),
            Vec3::from_array(c.forward),
            c.fov.to_radians(),
        ),
        None => {
            let eye = Vec3::from_array(OVERVIEW);
            (eye, -eye, 55f32.to_radians())
        }
    };
    // Straight up or down, Y cannot be the view's up axis.
    let up = if forward.normalize_or_zero().y.abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Ok(Camera::look_at(eye, eye + forward, up, fov, NEAR, FAR)?)
}

/// The frame's shadowed directional light. Ara's direction points toward the light.
pub(crate) fn sun(frame: &RenderFrame) -> Option<Sun> {
    let (toward, lux) = frame.sun?;
    let toward = Vec3::from_array(toward);
    if lux.iter().all(|c| *c <= 0.) || toward.length_squared() < 1e-12 {
        return None;
    }
    let default = Sun::default();
    Some(Sun {
        travel: -toward,
        illuminance: lux,
        angular_radius: frame
            .sun_radius
            .unwrap_or(default.angular_radius)
            .clamp(0., 0.1),
    })
}

/// The strength of a map sky's light over the sky drawn (Ara's `Environment.skyLight`):
/// Renderlab's `Sky::Map` lights at this intensity and draws the map at 1
/// (`Background::ScaledSky`) when it differs from 1.
pub(crate) fn sky_light(view: &ExternalView<'_>) -> f32 {
    let light = view.frame.environment.sky_light;
    if light.is_finite() { light.max(0.) } else { 1. }
}

/// Where a frame's sky comes from.
pub(crate) enum SkySource<'a> {
    /// Renderlab's physical atmosphere, from an authored sky atmosphere.
    Atmosphere(renderlab::engine::AtmosphereMedium),
    /// An equirectangular radiance image (cd/m²): the content's HDRI, or the
    /// gradient sky. `environment` prepares it for `Sky::Map`.
    Map {
        size: [u32; 2],
        pixels: Cow<'a, [[f32; 4]]>,
    },
}

/// The sky that lights and backs the frame. An authored physical sky maps to
/// Renderlab's atmosphere with its medium (`atmosphere::medium`); an environment map
/// or the gradient sky is drawn as it is and lights the scene at `sky_light`.
pub(crate) fn sky<'a>(view: &ExternalView<'a>, gaps: &mut Gaps) -> SkySource<'a> {
    let frame = view.frame;
    if let Some(sky) = &frame.look.sky {
        if frame.environment.sky_light != 1. {
            gaps.unsupported(
                "a sky light strength other than 1 under a physical sky",
                "the sky lights the scene as brightly as it looks",
            );
        }
        return SkySource::Atmosphere(crate::atmosphere::medium(sky, gaps));
    }
    match view.environment {
        Some(EnvironmentMap { size, pixels }) => SkySource::Map {
            size: *size,
            pixels: Cow::Borrowed(pixels),
        },
        None => {
            let (size, pixels) = ara_runtime_render::sky(&frame.environment);
            SkySource::Map {
                size,
                pixels: Cow::Owned(pixels),
            }
        }
    }
}

/// Local lights. Ara's `lumens` is point-equivalent: 4 pi times the candela.
pub(crate) fn lights<'a>(
    lights: impl IntoIterator<Item = &'a LightView>,
    gaps: &mut Gaps,
) -> Vec<Light> {
    let mut out = Vec::new();
    for light in lights {
        let intensity = light.lumens.max(0.) / (4. * PI);
        let candela = light.color.map(|c| c.max(0.) * intensity);
        if candela.iter().all(|c| *c <= 0.) {
            continue;
        }
        let position = Vec3::from_array(light.position);
        let range = (light.range > 0.).then_some(light.range);
        let source_radius = light.radius.max(0.);
        if light.profile.is_some() {
            gaps.unsupported(
                "photometric light profiles (IES)",
                "the light emits without its profile",
            );
        }
        if light.length > 0. {
            gaps.unsupported("tube lights", "the light is a sphere of its radius");
        }
        let spot = |inner: f32, outer: f32, source_radius: f32| Light::Spot {
            position,
            travel: Vec3::from_array(light.direction),
            candela,
            range,
            inner,
            outer,
            source_radius,
        };
        out.push(match light.kind {
            LightKind::Spot => {
                let outer = light.cone[1].to_radians().clamp(0.01, 1.55);
                spot(
                    light.cone[0].to_radians().clamp(0., outer),
                    outer,
                    source_radius,
                )
            }
            LightKind::Rect | LightKind::Disc => {
                gaps.unsupported(
                    "rect and disc lights",
                    "each is a wide spot of the same on-axis intensity",
                );
                let radius = match light.kind {
                    LightKind::Rect => 0.5 * light.size[0].min(light.size[1]).max(0.),
                    _ => source_radius,
                };
                // A one-sided Lambertian emitter: cosine falloff over its hemisphere.
                spot(0., 1.55, radius)
            }
            _ => Light::Point {
                position,
                candela,
                range,
                source_radius,
            },
        });
    }
    out
}

/// A display-referred meter for frames Ara would meter automatically: Renderlab has
/// manual exposure only, so the bridge steers its stops until the image's log-mean
/// luminance sits at middle grey.
pub(crate) struct Metering {
    stops: Option<f32>,
}

impl Metering {
    pub fn new() -> Self {
        Self { stops: None }
    }

    /// The stops to render the next frame with: the meter (`fallback` seeds the first)
    /// plus the look's exposure compensation.
    pub fn stops(&self, fallback: f32, look: &FrameEnvironment) -> f32 {
        self.stops.unwrap_or(fallback) + look.compensation
    }

    /// Feeds the frame rendered with `stops` (display-encoded RGBA8) back, metering as
    /// Ara's automatic exposure does: the log mean of the luminance between the look's
    /// low and high percentiles, steered to middle grey before compensation and kept
    /// within its EV100 limits. Returns whether the meter moved (false inside its dead
    /// band).
    pub fn observe(&mut self, look: &FrameEnvironment, stops: f32, rgba8: &[u8]) -> bool {
        // Every 37th pixel: a few thousand samples of a 1080p frame.
        let mut samples: Vec<f32> = rgba8
            .as_chunks::<4>()
            .0
            .iter()
            .step_by(37)
            .map(|pixel| {
                let linear = |v: u8| (f32::from(v) / 255.).powf(2.2);
                (0.2126 * linear(pixel[0]) + 0.7152 * linear(pixel[1]) + 0.0722 * linear(pixel[2]))
                    .max(1e-4)
                    .ln()
            })
            .collect();
        if samples.is_empty() {
            return false;
        }
        samples.sort_by(f32::total_cmp);
        let limits = &look.auto_exposure;
        let at =
            |p: f32| ((samples.len() as f32 * p.clamp(0., 1.)) as usize).min(samples.len() - 1);
        let (low, high) = (
            at(limits.low_percentile),
            at(limits.high_percentile).max(at(limits.low_percentile) + 1),
        );
        let trimmed = &samples[low..high.min(samples.len())];
        let mean = (trimmed.iter().sum::<f32>() / trimmed.len().max(1) as f32).exp();
        // Meter without the compensation the frame was rendered with.
        let meter = stops - look.compensation;
        let error = (0.18 / (mean / look.compensation.exp2())).log2();
        // A dead band keeps a settled image (and its temporal history) still.
        let step = if error.abs() < 0.15 {
            0.
        } else {
            (0.5 * error).clamp(-1., 1.)
        };
        let stops_at = |ev: f32| ara_render_fidelity::settings::exposure_from_ev100(ev, 0.).log2();
        let (darkest, brightest) = (
            stops_at(limits.maximum_ev100),
            stops_at(limits.minimum_ev100),
        );
        self.stops = Some((meter + step).clamp(darkest.min(brightest), brightest.max(darkest)));
        step != 0.
    }
}

/// Stops that put middle grey where Ara's display curves put it. Ara's Filmic, Neutral
/// and AgX curves keep 18% grey at 18%; Renderlab's filmic curve (Narkowicz's ACES
/// fit, which is also Ara's `AcesFit`) lifts it to about 27%, so a scene exposed for
/// Ara would display about half a stop brighter.
const GREY_MATCH_STOPS: f32 = -0.467;

/// Exposure stops for the frame: Ara's manual EV100, matched for the display curve,
/// else the meter (which steers the displayed image and needs no matching).
pub(crate) fn exposure_stops(view: &ExternalView<'_>, metering: &Metering) -> (f32, bool) {
    let fallback = view.frame.environment.exposure.max(1e-9).log2();
    let grey = match view.look.tone_mapper {
        ToneMapper::AcesFit => 0.,
        _ => GREY_MATCH_STOPS,
    };
    match view.look.ev100 {
        Some(_) => (
            view.look.manual_exposure(1.).max(1e-12).log2() + grey,
            false,
        ),
        None => (metering.stops(fallback, view.look), true),
    }
}

/// Renderlab settings for the frame's look. Fog and aerial perspective follow what
/// the scene authored: Renderlab's own defaults would add haze the scene never had.
pub(crate) fn settings(
    view: &ExternalView<'_>,
    stops: f32,
    max_reflection_roughness: Option<f32>,
    gaps: &mut Gaps,
) -> Settings {
    let (frame, look) = (view.frame, view.look);
    let fog = frame
        .look
        .fog
        .as_ref()
        .map(|fog| crate::atmosphere::fog(fog, gaps));
    if !frame.look.fog_volumes.is_empty() {
        gaps.unsupported("local fog volumes", "they are not drawn");
    }
    // Depth of field: Ara's lens is a full-frame camera whose focal length follows the
    // vertical field of view on a 24 mm sensor, as Renderlab's physical lens takes it.
    let depth_of_field = look
        .lens
        .depth_of_field
        .zip(frame.camera)
        .map(|(dof, camera)| {
            if dof.near != 1. || dof.far != 1. {
                gaps.unsupported(
                    "separate near and far depth-of-field strengths",
                    "both sides blur physically",
                );
            }
            let focal = (12. / (camera.fov.to_radians() * 0.5).tan()).clamp(1., 1000.);
            DepthOfField {
                focus_distance_m: dof.focus_distance.max(focal / 1000. + 0.01).min(1e7),
                focal_length_mm: focal,
                f_number: dof.aperture.clamp(0.5, 64.),
                sensor_height_mm: 24.,
                // Ara's largest radius is about 2% of the render height.
                max_radius_px: (0.02 * view.size[1] as f32).clamp(1., 32.),
            }
        });
    if look.lut.is_some() {
        gaps.unsupported("colour lookup tables", "the grade is not applied");
    }
    if !matches!(look.tone_mapper, ToneMapper::Filmic | ToneMapper::AcesFit) {
        gaps.unsupported(
            "the Neutral and AgX tone mappers",
            "the display curve is Renderlab's filmic, exposed so middle grey matches",
        );
    }
    let defaults = Settings::default();
    Settings {
        exposure: Exposure::Manual { stops },
        aerial_perspective: frame.look.sky.is_some(),
        height_fog: fog.map(|(height, _)| height),
        volumetric_fog: fog.and_then(|(_, volumetric)| volumetric),
        reflections: ReflectionSettings {
            max_roughness: max_reflection_roughness
                .unwrap_or(defaults.reflections.max_roughness)
                .clamp(0., 1.),
            ..defaults.reflections
        },
        post: PostSettings {
            bloom: look.bloom.clamp(0., 1.),
            motion_blur: look.lens.motion_blur > 0.,
            tonemapper: Tonemapper::Filmic,
            depth_of_field,
            // The grade itself is baked into the colour lookup table (`grade`).
            lut_strength: if crate::grade::lut_needed(look) {
                1.
            } else {
                0.
            },
            // Ara darkens by 1 / (1 + 2.5 v |d|²)² (d from the centre, aspect-corrected),
            // Renderlab by up to its strength at the corners: equal at a 16:9 corner.
            vignette: {
                let v = look.grading.vignette.clamp(0., 1.);
                1. - 1. / ((1. + 2.6 * v) * (1. + 2.6 * v))
            },
            ..defaults.post
        },
        ..defaults
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_points_away_from_ara_direction_and_dark_suns_are_none() {
        let mut frame = RenderFrame::default();
        assert!(sun(&frame).is_none());
        frame.sun = Some(([0., 1., 0.], [100_000.; 3]));
        frame.sun_radius = Some(0.5);
        let sun = sun(&frame).expect("lit sun");
        assert_eq!(sun.travel, Vec3::NEG_Y);
        assert_eq!(sun.angular_radius, 0.1);
        frame.sun = Some(([0., 1., 0.], [0.; 3]));
        assert!(super::sun(&frame).is_none());
    }

    #[test]
    fn lumens_become_candela_and_area_lights_report_a_gap() {
        let mut gaps = Gaps::default();
        let point = LightView {
            kind: LightKind::Point,
            color: [1., 0.5, 0.],
            lumens: 4. * PI * 100.,
            range: 10.,
            ..Default::default()
        };
        let rect = LightView {
            kind: LightKind::Rect,
            color: [1.; 3],
            lumens: 4. * PI,
            direction: [0., -1., 0.],
            size: [2., 1.],
            ..Default::default()
        };
        let dark = LightView::default();
        let out = lights(&[point, rect, dark], &mut gaps);
        assert_eq!(out.len(), 2);
        assert!(matches!(
            out[0],
            Light::Point { candela, range: Some(r), .. } if candela == [100., 50., 0.] && r == 10.
        ));
        assert!(matches!(out[1], Light::Spot { source_radius, .. } if source_radius == 0.5));
        assert!(gaps.reported("rect and disc lights"));
    }

    #[test]
    fn grey_match_puts_18_percent_grey_at_18_percent_on_renderlabs_filmic_curve() {
        let x = 0.18 * 2f32.powf(GREY_MATCH_STOPS);
        let displayed = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
        assert!((displayed - 0.18).abs() < 2e-3, "{displayed}");
    }

    #[test]
    fn metering_steers_toward_middle_grey_and_rests_in_its_dead_band() {
        let mut look = FrameEnvironment::resolve(&Default::default(), glam30::Vec3::ZERO, 1.);
        let mut metering = Metering::new();
        assert_eq!(metering.stops(-3., &look), -3.);
        metering.observe(&look, -3., &[255; 4 * 64]);
        assert!(metering.stops(0., &look) < -3., "a white frame stops down");
        // sRGB 118 is about 18% linear: no change.
        metering.observe(&look, -5., &[118; 4 * 64]);
        assert_eq!(metering.stops(0., &look), -5.);
        // Compensation renders brighter, and the meter does not fight it.
        look.compensation = 1.;
        assert_eq!(metering.stops(0., &look), -4.);
        metering.observe(&look, -4., &[160; 4 * 64]);
        assert_eq!(metering.stops(0., &look), -4.);
        // The EV100 limits bound the meter: EV100 5 is about -5.3 stops.
        look.auto_exposure.maximum_ev100 = 5.;
        metering.observe(&look, -4., &[255; 4 * 64]);
        assert!(metering.stops(0., &look) - look.compensation >= -5.27 - 1e-3);
    }
}
