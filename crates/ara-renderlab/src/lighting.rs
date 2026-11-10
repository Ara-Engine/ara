//! Ara's extracted camera, sun, sky, local lights, exposure and display as
//! Renderlab engine values. Pure conversions; `bridge` applies them.
use crate::gaps::Gaps;
use anyhow::Result;
use ara_render_fidelity::ToneMapper;
use ara_runtime::{
    components::LightKind,
    extract::{LightView, RenderFrame},
};
use ara_runtime_render::{ExternalView, fidelity::EnvironmentMap};
use renderlab::engine::{
    AtmosphereMedium, Camera, Exposure, HeightFog, Light, PostSettings, ReflectionSettings,
    Settings, Sky, Sun, Tonemapper, Vec3, VolumetricFog,
};
use std::f32::consts::PI;

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

/// Mean radiance (cd/m²) of an equirectangular map's upper hemisphere, weighted by
/// solid angle: the constant sky that sends the same light down.
pub(crate) fn mean_sky_radiance(size: [u32; 2], pixels: &[[f32; 4]]) -> [f32; 3] {
    let [width, height] = size.map(|v| v as usize);
    let (mut sum, mut weight) = ([0f64; 3], 0f64);
    for y in 0..height / 2 {
        let elevation = PI / 2. - (y as f32 + 0.5) / height as f32 * PI;
        let w = f64::from(elevation.cos());
        for pixel in &pixels[y * width..(y + 1) * width] {
            for (s, c) in sum.iter_mut().zip(pixel) {
                *s += f64::from(c.max(0.)) * w;
            }
            weight += w;
        }
    }
    sum.map(|s| if weight > 0. { (s / weight) as f32 } else { 0. })
}

/// The sky that lights and backs the frame. An authored physical sky maps to
/// Renderlab's Earth atmosphere; an environment map or the gradient sky to a
/// constant sky of the same mean radiance.
pub(crate) fn sky(view: &ExternalView<'_>, gaps: &mut Gaps) -> Sky {
    let frame = view.frame;
    if frame.look.sky.is_some() {
        gaps.unsupported(
            "authored sky atmosphere parameters",
            "the sky is Renderlab's default Earth atmosphere",
        );
        return Sky::Atmosphere(AtmosphereMedium::default());
    }
    let radiance = match view.environment {
        Some(EnvironmentMap { size, pixels }) => {
            gaps.unsupported(
                "environment maps (HDRI skies)",
                "the sky is a constant of the map's mean radiance",
            );
            mean_sky_radiance(*size, pixels)
        }
        None => {
            gaps.unsupported(
                "gradient skies",
                "the sky is a constant of the gradient's mean radiance",
            );
            let (size, pixels) = ara_runtime_render::sky(&frame.environment);
            mean_sky_radiance(size, &pixels)
        }
    };
    Sky::Uniform(radiance)
}

/// Local lights. Ara's `lumens` is point-equivalent: 4 pi times the candela.
pub(crate) fn lights(lights: &[LightView], gaps: &mut Gaps) -> Vec<Light> {
    let mut out = Vec::with_capacity(lights.len());
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

    /// The stops to render the next frame with; `fallback` seeds the first.
    pub fn stops(&self, fallback: f32) -> f32 {
        self.stops.unwrap_or(fallback)
    }

    /// Feeds the frame rendered with `stops` (display-encoded RGBA8) back.
    /// Returns whether the meter moved (false inside its dead band).
    pub fn observe(&mut self, stops: f32, rgba8: &[u8]) -> bool {
        // Every 37th pixel: a few thousand samples of a 1080p frame.
        let (mut sum, mut count) = (0f64, 0u32);
        for pixel in rgba8.as_chunks::<4>().0.iter().step_by(37) {
            let linear = |v: u8| (f32::from(v) / 255.).powf(2.2);
            let luminance =
                0.2126 * linear(pixel[0]) + 0.7152 * linear(pixel[1]) + 0.0722 * linear(pixel[2]);
            sum += f64::from(luminance.max(1e-4).ln());
            count += 1;
        }
        if count == 0 {
            return false;
        }
        let mean = (sum / f64::from(count)).exp() as f32;
        let error = (0.18 / mean).log2();
        // A dead band keeps a settled image (and its temporal history) still.
        let step = if error.abs() < 0.15 {
            0.
        } else {
            (0.5 * error).clamp(-1., 1.)
        };
        self.stops = Some((stops + step).clamp(-24., 8.));
        step != 0.
    }
}

/// Exposure stops for the frame: Ara's manual EV100 exactly, else the meter.
pub(crate) fn exposure_stops(view: &ExternalView<'_>, metering: &Metering) -> (f32, bool) {
    let fallback = view.frame.environment.exposure.max(1e-9).log2();
    match view.look.ev100 {
        Some(_) => (view.look.manual_exposure(1.).max(1e-12).log2(), false),
        None => (metering.stops(fallback), true),
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
    let fog = frame.look.fog.is_some();
    if fog {
        gaps.unsupported(
            "authored height-fog parameters",
            "fog uses Renderlab's default medium",
        );
    }
    if !frame.look.fog_volumes.is_empty() {
        gaps.unsupported("local fog volumes", "they are not drawn");
    }
    if look.lens.depth_of_field.is_some() {
        gaps.unsupported("depth of field", "the image is sharp at every depth");
    }
    if look.lut.is_some() {
        gaps.unsupported("colour lookup tables", "the grade is not applied");
    }
    if !matches!(look.tone_mapper, ToneMapper::Filmic | ToneMapper::AcesFit) {
        gaps.unsupported(
            "the Neutral and AgX tone mappers",
            "the display curve is Renderlab's filmic",
        );
    }
    let defaults = Settings::default();
    Settings {
        exposure: Exposure::Manual { stops },
        aerial_perspective: frame.look.sky.is_some(),
        height_fog: fog.then(HeightFog::default),
        volumetric_fog: fog.then(VolumetricFog::default),
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
    fn mean_sky_radiance_weights_the_upper_hemisphere_by_solid_angle() {
        // Two rows above the horizon (zenith ring 4, horizon ring 1), two below (99).
        let row = |v: f32| [[v, v, v, 1.]; 2];
        let pixels = [row(4.), row(1.), row(99.), row(99.)].concat();
        let mean = mean_sky_radiance([2, 4], &pixels)[0];
        let (zenith, horizon) = ((3. * PI / 8.).cos(), (PI / 8.).cos());
        let expected = (4. * zenith + horizon) / (zenith + horizon);
        assert!((mean - expected).abs() < 1e-4, "{mean} vs {expected}");
    }

    #[test]
    fn metering_steers_toward_middle_grey_and_rests_in_its_dead_band() {
        let mut metering = Metering::new();
        assert_eq!(metering.stops(-3.), -3.);
        metering.observe(-3., &[255; 4 * 64]);
        assert!(metering.stops(0.) < -3., "a white frame stops down");
        // sRGB 118 is about 18% linear: no change.
        metering.observe(-5., &[118; 4 * 64]);
        assert_eq!(metering.stops(0.), -5.);
    }
}
