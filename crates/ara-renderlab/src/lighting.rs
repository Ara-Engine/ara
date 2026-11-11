//! Ara's extracted camera, sun, sky, local lights, exposure and display as
//! Renderlab engine values. Pure conversions; `bridge` applies them.
use crate::gaps::Gaps;
use anyhow::Result;
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

/// The strength of the sky's light over the sky drawn (Ara's `Environment.skyLight`):
/// Renderlab's atmosphere takes it as `Settings::sky_light`; a `Sky::Map` lights at this
/// intensity and draws the map at 1 (`Background::ScaledSky`) when it differs from 1.
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
/// or the gradient sky is drawn as it is. Both light the scene at `sky_light`
/// (`Settings::sky_light` for the atmosphere, `Sky::Map`'s intensity for a map).
pub(crate) fn sky<'a>(view: &ExternalView<'a>, gaps: &mut Gaps) -> SkySource<'a> {
    let frame = view.frame;
    if let Some(sky) = &frame.look.sky {
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
/// luminance sits at middle grey. Each rendered frame adapts toward the target the
/// last frame read back measured (`next`); frames are read back every frame while the
/// meter snaps and every `READBACK_EVERY`th otherwise (`due`, `observe`).
pub(crate) struct Metering {
    /// The meter: exposure stops before the compensation and its curve.
    stops: Option<f32>,
    /// Where the meter adapts to: the last frame read back's meter plus its error,
    /// within the EV100 limits (the meter itself inside the dead band).
    target: Option<f32>,
    /// Snap to the meter's target instead of adapting, until the image settles or the
    /// meter is pinned at an EV100 limit: the first frame and every cut, as Ara resets
    /// its exposure history.
    snapping: bool,
    /// Frames until the meter next wants a frame read back.
    wait: u32,
}

/// The time each rendered frame adapts by, as Ara's renderer (`FRAME_SECONDS` in
/// ara-render's look): it has no wall clock and does not follow the simulation tick, so
/// a paused game or a redrawn tick still adapts.
const FRAME_SECONDS: f32 = 1. / 60.;

/// Frames per readback once the meter adapts or rests; between readbacks it adapts
/// toward the last target, so a two-stop change costs a handful of readbacks.
const READBACK_EVERY: u32 = 16;

/// Ara's compensation curve at scene EV100 `ev` (`exposureCurve`, shaders/look.slangh):
/// stops from (EV100, stops) keys, linear between them and held past both ends. All-zero
/// keys add none.
fn compensation_curve(look: &FrameEnvironment, ev: f32) -> f32 {
    let keys = &look.compensation_curve;
    let mut a = keys[0];
    let (mut stops, mut done) = (a[1], ev <= a[0]);
    for b in keys {
        if !done && ev <= b[0] {
            let t = ((ev - a[0]) / (b[0] - a[0]).max(1e-6)).clamp(0., 1.);
            stops = a[1] + (b[1] - a[1]) * t;
            done = true;
        }
        if !done {
            stops = b[1];
        }
        a = *b;
    }
    stops
}

/// The scene EV100 a meter of `stops` exposes for: Ara's exposure is 1 / (1.2 * 2^EV).
fn scene_ev(stops: f32) -> f32 {
    -stops - 1.2f32.log2()
}

/// The share of the way to its target Ara's exposure adapts over `seconds` at `rate`
/// per second (`exposureMain`: `1 - exp(-seconds * rate)`).
fn adaptation(rate: f32, seconds: f32) -> f32 {
    1. - (-(rate.max(0.) * seconds.max(0.))).exp()
}

impl Metering {
    pub fn new() -> Self {
        Self {
            stops: None,
            target: None,
            snapping: true,
            wait: 0,
        }
    }

    /// After a cut: the next frames are read back and snap to the meter's target until
    /// it settles.
    pub fn snap(&mut self) {
        self.snapping = true;
        self.wait = 0;
    }

    /// Advances the meter by one rendered frame and returns the meter for it (`fallback`
    /// seeds the first) and the stops to render it with: the meter plus the look's
    /// compensation and, as Ara's automatic exposure, its compensation curve at the
    /// scene EV100 the meter exposes for. Call once per rendered frame.
    pub fn next(&mut self, fallback: f32, look: &FrameEnvironment) -> (f32, f32) {
        let mut meter = self.stops.unwrap_or(fallback);
        if let Some(target) = self.target.filter(|_| !self.snapping) {
            // Ara adapts the exposure multiplier itself: lerp(old, target, a).
            let share = adaptation(look.adaptation, FRAME_SECONDS);
            meter += (1. + ((target - meter).exp2() - 1.) * share).log2();
            self.stops = Some(meter);
        }
        let curve = compensation_curve(look, scene_ev(meter));
        (meter, meter + look.compensation + curve)
    }

    /// Whether the frame just rendered should be read back for `observe`: every frame
    /// while the meter snaps, then every `READBACK_EVERY`th. Call once per frame.
    pub fn due(&mut self) -> bool {
        let due = self.wait == 0;
        self.wait = self.wait.saturating_sub(1);
        due
    }

    /// Feeds back the frame rendered with `stops` from `meter` (display-encoded RGBA8),
    /// metering as Ara's automatic exposure does: the log mean of the luminance between
    /// the look's low and high percentiles, steered to middle grey before the
    /// compensation and its curve, within its EV100 limits. Sets the target `next` adapts
    /// toward, or snaps to it after a cut.
    pub fn observe(&mut self, look: &FrameEnvironment, meter: f32, stops: f32, rgba8: &[u8]) {
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
            return;
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
        // The meter's error, without the compensation and curve the frame was rendered with.
        let error = (0.18 / (mean / (stops - meter).exp2())).log2();
        let stops_at = |ev: f32| ara_render_fidelity::settings::exposure_from_ev100(ev, 0.).log2();
        let (darkest, brightest) = (
            stops_at(limits.maximum_ev100),
            stops_at(limits.minimum_ev100),
        );
        // How far the limits let the meter go: a meter pinned at a limit has settled as
        // far as it can, so a snap ends there too.
        let reach = (meter + error).clamp(darkest.min(brightest), brightest.max(darkest)) - meter;
        self.stops = Some(meter);
        if reach.abs() < 0.15 {
            // A dead band keeps a settled image (and its temporal history) still.
            self.target = Some(meter);
            self.snapping = false;
        } else if self.snapping {
            // The display curve compresses: a few frames reach the target.
            let snapped = meter + reach.clamp(-4., 4.);
            self.stops = Some(snapped);
            self.target = Some(snapped);
        } else {
            self.target = Some(meter + reach);
        }
        self.wait = if self.snapping { 0 } else { READBACK_EVERY - 1 };
    }
}

/// Exposure stops for the frame: Ara's manual EV100 (the colour table maps Renderlab's
/// curve to the look's, so exposure needs no matching), else the meter's, with the meter
/// they came from.
pub(crate) fn exposure_stops(
    view: &ExternalView<'_>,
    metering: &mut Metering,
) -> (f32, Option<f32>) {
    let fallback = view.frame.environment.exposure.max(1e-9).log2();
    match view.look.ev100 {
        Some(_) => (view.look.manual_exposure(1.).max(1e-12).log2(), None),
        None => {
            let (meter, stops) = metering.next(fallback, view.look);
            (stops, Some(meter))
        }
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
    let defaults = Settings::default();
    Settings {
        exposure: Exposure::Manual { stops },
        // Only the atmosphere reads it; a map's strength rides on `Sky::Map` (`bridge`).
        sky_light: sky_light(view),
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

    fn look() -> FrameEnvironment {
        FrameEnvironment::resolve(&Default::default(), glam30::Vec3::ZERO, 1.)
    }

    /// A flat grey frame whose display luminance is `grey` (display-encoded RGBA8).
    fn frame(grey: f32) -> Vec<u8> {
        let v = (grey.powf(1. / 2.2) * 255.).round() as u8;
        [v, v, v, 255].repeat(64 * 64)
    }

    /// As Ara's `exposureCurve`: held past both ends, linear between keys, none when
    /// every key is zero.
    #[test]
    fn the_compensation_curve_reads_as_aras() {
        let mut look = look();
        assert_eq!(compensation_curve(&look, 3.), 0.);
        look.compensation_curve = ara_render::look::AutoExposure::curve(&[[4., -1.], [-2., 1.]]);
        assert_eq!(compensation_curve(&look, -5.), 1.);
        assert!(compensation_curve(&look, 1.).abs() < 1e-6);
        assert_eq!(compensation_curve(&look, 10.), -1.);
    }

    /// Renders frames until the meter stops snapping, each read back and showing
    /// `shown(stops)`; returns how many it took.
    fn settle(meter: &mut Metering, look: &FrameEnvironment, shown: impl Fn(f32) -> f32) -> u32 {
        let mut frames = 0;
        while meter.snapping {
            let (m, stops) = meter.next(-6., look);
            assert!(meter.due(), "a snapping meter reads every frame back");
            meter.observe(look, m, stops, &frame(shown(stops)));
            frames += 1;
            assert!(frames < 6, "the snap converges");
        }
        frames
    }

    /// After a cut the meter snaps to middle grey within a few frames; once settled it
    /// adapts by Ara's share per rendered frame.
    #[test]
    fn the_meter_snaps_after_a_cut_then_adapts_at_aras_rate() {
        let look = look();
        let mut meter = Metering::new();
        // A scene whose display luminance is the exposure itself: 0.18 at stops log2(0.18).
        let shown = |stops: f32| stops.exp2().min(1.);
        settle(&mut meter, &look, shown);
        let (m, stops) = meter.next(-6., &look);
        assert!((stops - 0.18f32.log2()).abs() < 0.2, "{stops}");
        // The scene brightens by two stops: the next frame moves Ara's share of the way.
        meter.observe(&look, m, stops, &frame(shown(stops + 2.)));
        let moved = m - meter.next(-6., &look).0;
        let a = adaptation(look.adaptation, FRAME_SECONDS);
        let expected = -(1. + ((-2f32).exp2() - 1.) * a).log2();
        assert!((moved - expected).abs() < 0.01, "{moved} vs {expected}");
    }

    /// The meter takes no clock from the simulation: frames rendered while the tick
    /// stands still (a paused game, a redraw) adapt as any others, as Ara's renderer
    /// adapts per frame. A second of frames covers 1 - exp(-rate) of the way.
    #[test]
    fn the_meter_adapts_while_the_tick_stands_still() {
        let look = look();
        let mut meter = Metering::new();
        let shown = |stops: f32| stops.exp2().min(1.);
        settle(&mut meter, &look, shown);
        let (m, stops) = meter.next(-6., &look);
        meter.observe(&look, m, stops, &frame(shown(stops + 2.)));
        for _ in 0..59 {
            meter.next(-6., &look);
        }
        let covered = (1. - (meter.next(-6., &look).0 - m).exp2()) / (1. - (-2f32).exp2());
        let expected = 1. - (-look.adaptation).exp();
        assert!((covered - expected).abs() < 0.02, "{covered} vs {expected}");
    }

    /// A scene the EV100 limits cannot expose pins the meter at the limit and ends the
    /// snap: a later change is followed at Ara's rate, not snapped.
    #[test]
    fn a_meter_pinned_at_a_limit_stops_snapping() {
        let mut look = look();
        // EV100 8 at the brightest: about -8.26 stops, far short of this dark scene.
        look.auto_exposure.minimum_ev100 = 8.;
        let limit = ara_render_fidelity::settings::exposure_from_ev100(8., 0.).log2();
        let mut meter = Metering::new();
        let dark = |stops: f32| (stops - 6.).exp2().min(1.);
        settle(&mut meter, &look, dark);
        let (m, stops) = meter.next(-6., &look);
        assert!((m - limit).abs() < 1e-4, "pinned: {m} vs {limit}");
        // The lights come on, two stops past middle grey at the limit.
        let lit = |s: f32| (0.72 * (s - stops).exp2()).min(1.);
        meter.observe(&look, m, stops, &frame(lit(stops)));
        assert!(!meter.snapping);
        let moved = m - meter.next(-6., &look).0;
        assert!(moved > 0. && moved < 0.1, "adapts, does not snap: {moved}");
    }

    /// While the meter adapts it reads back one frame in `READBACK_EVERY`, adapting
    /// toward the last target in between: a two-stop change settles over two seconds
    /// of frames with no more readbacks than that.
    #[test]
    fn adapting_reads_back_one_frame_in_sixteen() {
        let look = look();
        let mut meter = Metering::new();
        let mut brightness = 0.;
        let shown = |stops: f32, b: f32| (stops + b).exp2().min(1.);
        settle(&mut meter, &look, |s| shown(s, 0.));
        brightness += 2.;
        let mut readbacks = 0;
        let mut last = 0.;
        for _ in 0..120 {
            let (m, stops) = meter.next(-6., &look);
            last = stops;
            if meter.due() {
                meter.observe(&look, m, stops, &frame(shown(stops, brightness)));
                readbacks += 1;
            }
        }
        assert!(
            readbacks <= 120 / READBACK_EVERY + 1,
            "{readbacks} readbacks"
        );
        let displayed = shown(last, brightness);
        assert!(
            (displayed / 0.18).log2().abs() < 0.25,
            "settles: {displayed}"
        );
    }

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
    fn metering_steers_toward_middle_grey_and_rests_in_its_dead_band() {
        let mut look = FrameEnvironment::resolve(&Default::default(), glam30::Vec3::ZERO, 1.);
        let mut metering = Metering::new();
        assert_eq!(metering.next(-3., &look), (-3., -3.));
        metering.observe(&look, -3., -3., &[255; 4 * 64]);
        assert!(metering.next(0., &look).1 < -3., "a white frame stops down");
        // sRGB 118 is about 18% linear: no change.
        metering.stops = Some(-5.);
        metering.observe(&look, -5., -5., &[118; 4 * 64]);
        assert_eq!(metering.next(0., &look).1, -5.);
        // Compensation renders brighter, and the meter does not fight it.
        look.compensation = 1.;
        assert_eq!(metering.next(0., &look), (-5., -4.));
        metering.observe(&look, -5., -4., &[160; 4 * 64]);
        assert_eq!(metering.next(0., &look), (-5., -4.));
        // The EV100 limits bound the meter: EV100 5 is about -5.3 stops.
        look.auto_exposure.maximum_ev100 = 5.;
        metering.snap();
        metering.observe(&look, -5., -4., &[255; 4 * 64]);
        assert!(metering.next(0., &look).0 >= -5.27 - 1e-3);
    }
}
