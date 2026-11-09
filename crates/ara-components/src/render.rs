//! The authorable look of a scene: physical sky and height fog (the first entity in
//! stable-ID order carrying each), local fog volumes, post-process volumes
//! (`crate::post`) and reflection captures (all of them), plus the project's rendering
//! quality setting. Renderers receive them through the runtime extraction; the sun is
//! the scene's directional light. Every field doc is an inspector tooltip.

use ara_schema::{Color, Schema};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Rendering quality. `auto` picks from the GPU: High on ray-tracing GPUs (Medium
/// below 8 GiB of video memory), Low without ray tracing. Epic is never automatic.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum QualitySetting {
    /// Detect from the GPU.
    #[default]
    Auto,
    /// Raster renderer with temporal antialiasing; no ray tracing.
    Low,
    /// Raster lighting with ambient occlusion and screen-space reflections.
    Medium,
    /// Raster lighting with stronger shadow, GI and screen-space effect budgets.
    High,
    /// The highest raster shadow, GI and reflection budgets. The most expensive.
    Epic,
}

/// Project-wide rendering settings, stored under `rendering` in `ara.project.json`.
/// A player's `--quality` flag overrides `quality` for that run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderSettings {
    /// Default quality preset for Play, the editor's lit viewport and exported players.
    pub quality: QualitySetting,
    /// Opt in to hardware ray tracing on GPUs that support it: the ray-traced
    /// renderer for Medium and above. Off (the default), every preset is raster and
    /// traces no rays, whatever the GPU, as in Unreal.
    pub ray_tracing: bool,
}

/// A physical sky and aerial perspective (Unreal's SkyAtmosphere, value for value: the
/// same parameters, units and defaults). Lit by the directional lights marked as
/// atmosphere lights (the sun, and a second light such as a moon); the sun's elevation
/// sets the time of day. Without this component the scene uses its `Environment`
/// gradient. The first entity in ID order wins. Distances are in kilometres and
/// coefficients per kilometre, as in Unreal; colours are linear.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render", version = 2, upgrade = upgrade_sky_atmosphere)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SkyAtmosphere {
    /// World altitude of the planet's surface (sea level); the sky and fog measure
    /// height from here (Unreal's planet top at the absolute world origin or at the
    /// component).
    #[ara(unit = Meters, example = 0)]
    pub sea_level: f32,
    /// Radius from the planet's centre to the ground (Unreal's Ground Radius).
    #[ara(default = 6360, unit = Kilometers, range(min = 0.1, max = 10000))]
    pub bottom_radius: f32,
    /// Colour of the ground lighting the atmosphere from below through multiple
    /// scattering, and of the planet seen below the horizon (linear; Unreal's sRGB 170 grey).
    #[ara(default = [0.401978, 0.401978, 0.401978], range(min = 0, max = 1), editor(color))]
    pub ground_albedo: [f32; 3],
    /// Thickness of the atmosphere above the ground.
    #[ara(default = 60, unit = Kilometers, range(min = 0.1, max = 10000))]
    pub atmosphere_height: f32,
    /// How much light scatters more than once: 0 is single scattering only (a darker,
    /// more saturated sky), 1 physical.
    #[ara(default = 1, range(min = 0, max = 100))]
    pub multi_scattering: f32,
    /// Scales the number of samples the sky's ray marches take: more removes banding
    /// in thick or tall atmospheres at a higher LUT cost.
    #[ara(default = 1, range(min = 0.25, max = 8))]
    pub trace_sample_count_scale: f32,
    /// Rayleigh (air molecule) scattering at the ground, per kilometre, times
    /// `rayleighScattering`: the blue of the sky.
    #[ara(default = 0.0331, range(min = 0, max = 2))]
    pub rayleigh_scattering_scale: f32,
    /// Rayleigh scattering colour (linear), times `rayleighScatteringScale`.
    #[ara(default = [0.175287, 0.409607, 1], range(min = 0), editor(color))]
    pub rayleigh_scattering: [f32; 3],
    /// Altitude over which Rayleigh density falls by a factor of e.
    #[ara(default = 8, unit = Kilometers, range(min = 0.001, max = 20))]
    pub rayleigh_exponential_distribution: f32,
    /// Mie (aerosol: haze, dust, pollution) scattering at the ground, per kilometre,
    /// times `mieScattering`: a whiter horizon and a wider glow around the sun.
    #[ara(default = 0.003996, range(min = 0, max = 5))]
    pub mie_scattering_scale: f32,
    /// Mie scattering colour (linear), times `mieScatteringScale`.
    #[ara(default = [1, 1, 1], range(min = 0), editor(color))]
    pub mie_scattering: [f32; 3],
    /// Mie absorption at the ground, per kilometre, times `mieAbsorption`: darker,
    /// browner haze.
    #[ara(default = 0.000444, range(min = 0, max = 5))]
    pub mie_absorption_scale: f32,
    /// Mie absorption colour (linear), times `mieAbsorptionScale`.
    #[ara(default = [1, 1, 1], range(min = 0), editor(color))]
    pub mie_absorption: [f32; 3],
    /// How strongly aerosols scatter forward: 0 evenly in all directions, towards 1 a
    /// tight bright halo around the sun.
    #[ara(default = 0.8, range(min = 0, max = 0.999))]
    pub mie_anisotropy: f32,
    /// Altitude over which Mie density falls by a factor of e.
    #[ara(default = 1.2, unit = Kilometers, range(min = 0.001, max = 10))]
    pub mie_exponential_distribution: f32,
    /// Absorption of a layer such as ozone, per kilometre at its peak, times
    /// `absorption`: deepens the twilight blue (Unreal's Absorption Scale).
    #[ara(default = 0.001881, range(min = 0, max = 0.2))]
    pub absorption_scale: f32,
    /// Absorption colour (linear), times `absorptionScale`.
    #[ara(default = [0.345561, 1, 0.045189], range(min = 0), editor(color))]
    pub absorption: [f32; 3],
    /// Altitude of the absorbing layer's peak (Unreal's tent distribution tip altitude).
    #[ara(default = 25, unit = Kilometers, range(min = 0, max = 60))]
    pub absorption_tip_altitude: f32,
    /// Density at the absorbing layer's peak (Unreal's tent tip value).
    #[ara(default = 1, range(min = 0, max = 1))]
    pub absorption_tip_value: f32,
    /// Altitude over which the layer's density falls from its peak to zero, above and
    /// below it (Unreal's tent width); 0 removes the layer.
    #[ara(default = 15, unit = Kilometers, range(min = 0, max = 20))]
    pub absorption_width: f32,
    /// Multiplies the luminance of the sky itself (and so the sky light it casts), not
    /// the aerial perspective (linear).
    #[ara(default = [1, 1, 1], range(min = 0), editor(color, hdr))]
    pub sky_luminance_factor: [f32; 3],
    /// Multiplies the luminance of the sky and of the aerial perspective (linear).
    #[ara(default = [1, 1, 1], range(min = 0), editor(color, hdr))]
    pub sky_and_aerial_perspective_luminance_factor: [f32; 3],
    /// Makes the aerial perspective look thicker by scaling distances from the camera to
    /// surfaces (not to the sky).
    #[ara(default = 1, range(min = 0, max = 3))]
    pub aerial_perspective_view_distance_scale: f32,
    /// Scales the sun and sky light the height fog scatters beyond the volumetric fog
    /// (Unreal's sky atmosphere contribution to exponential height fog).
    #[ara(default = 1, range(min = 0, max = 1))]
    pub height_fog_contribution: f32,
    /// The lowest sun elevation used for the sunlight on surfaces: keeps a setting sun
    /// lighting and shadowing the scene. -90 is physical. The sky is unaffected.
    #[ara(default = -90, unit = Degrees, range(min = -90, max = 90))]
    pub transmittance_min_light_elevation_angle: f32,
    /// Distance from the camera where the aerial perspective starts.
    #[ara(default = 0.1, unit = Kilometers, range(min = 0.001, max = 10))]
    pub aerial_perspective_start_depth: f32,
    /// Fraction of the sky covered by the lit cloud layer; 0 disables clouds.
    #[ara(range(min = 0, max = 1))]
    pub cloud_coverage: f32,
    /// Altitude of the cloud layer above sea level.
    #[ara(default = 2200, unit = Meters, range(min = 500, max = 20000))]
    pub cloud_height: f32,
}
impl Default for SkyAtmosphere {
    fn default() -> Self {
        Self {
            sea_level: 0.0,
            bottom_radius: 6360.0,
            ground_albedo: [0.401978; 3],
            atmosphere_height: 60.0,
            multi_scattering: 1.0,
            trace_sample_count_scale: 1.0,
            rayleigh_scattering_scale: 0.0331,
            rayleigh_scattering: [0.175287, 0.409607, 1.0],
            rayleigh_exponential_distribution: 8.0,
            mie_scattering_scale: 0.003996,
            mie_scattering: [1.0; 3],
            mie_absorption_scale: 0.000444,
            mie_absorption: [1.0; 3],
            mie_anisotropy: 0.8,
            mie_exponential_distribution: 1.2,
            absorption_scale: 0.001881,
            absorption: [0.345561, 1.0, 0.045189],
            absorption_tip_altitude: 25.0,
            absorption_tip_value: 1.0,
            absorption_width: 15.0,
            sky_luminance_factor: [1.0; 3],
            sky_and_aerial_perspective_luminance_factor: [1.0; 3],
            aerial_perspective_view_distance_scale: 1.0,
            height_fog_contribution: 1.0,
            transmittance_min_light_elevation_angle: -90.0,
            aerial_perspective_start_depth: 0.1,
            cloud_coverage: 0.0,
            cloud_height: 2200.0,
        }
    }
}

/// Version 1 had `aerosols`, the Mie density relative to clear Earth air; version 2
/// takes Unreal's Mie scattering and absorption scales: Earth's times that density.
fn upgrade_sky_atmosphere(from: u32, v: &mut serde_json::Value) -> Result<(), String> {
    if from != 1 {
        return Ok(());
    }
    let Some(record) = v.as_object_mut() else {
        return Ok(());
    };
    if let Some(aerosols) = record.remove("aerosols") {
        let density = aerosols.as_f64().ok_or("`aerosols` must be a number")?;
        let earth = SkyAtmosphere::default();
        for (key, value) in [
            ("mieScatteringScale", earth.mie_scattering_scale),
            ("mieAbsorptionScale", earth.mie_absorption_scale),
        ] {
            record
                .entry(key)
                .or_insert_with(|| serde_json::json!(f64::from(value) * density));
        }
    }
    Ok(())
}

/// Exponential height fog lit by the sun, the sky and local lights. Volumetric fog
/// (the default) casts shafts where shadows cut the beams, lets lamps glow in it and
/// renders the near field at screen-tile detail from the Medium preset up (Unreal's
/// volumetric fog). Density halves every `falloff × ln 2` metres above `baseHeight`.
/// The first entity in ID order wins.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct HeightFog {
    /// Extinction at `baseHeight`, per metre. 0.002 is a light haze (visibility ≈ 1.5 km),
    /// 0.02 a thick fog (≈ 150 m).
    #[ara(default = 0.002, range(min = 0, max = 0.1), example = 0.004)]
    pub density: f32,
    /// World altitude where the fog has its full density.
    #[ara(unit = Meters)]
    pub base_height: f32,
    /// Scale height: density falls by a factor of e over this many metres of altitude.
    #[ara(default = 60, unit = Meters, range(min = 1, max = 10000))]
    pub falloff: f32,
    /// Scattering colour (single-scattering albedo, linear). Grey-white for water
    /// fog, warm for dust.
    #[ara(default = [0.9, 0.9, 0.9, 1])]
    pub albedo: Color,
    /// Forward-scattering bias: 0 is uniform, toward 1 the fog glows around the sun and lamps.
    #[ara(default = 0.3, range(min = 0, max = 0.9))]
    pub anisotropy: f32,
    /// Volumetric fog: shadows cut sun shafts and lamp cones into the fog (and into
    /// local fog volumes) and local lights scatter in it. Off, the fog is plain
    /// height fog: unshadowed sun glow and sky light only.
    #[ara(default = true)]
    pub volumetric: bool,
    /// How far from the camera the detailed (froxel) volumetric fog reaches; beyond it
    /// a coarser grid carries the fog. Larger distances spread the same slices thinner.
    #[ara(default = 60, unit = Meters, range(min = 1, max = 1000))]
    pub volumetric_distance: f32,
    /// Light the fog itself emits (linear radiance per metre of fog at `density`,
    /// following the height falloff), for glowing haze. Black emits none.
    #[ara(range(min = 0), editor(color, hdr))]
    pub volumetric_emissive: [f32; 3],
    /// Scales how much light the fog absorbs (not how much it scatters): above 1 the
    /// fog darkens what lies behind it more for the same glow.
    #[ara(default = 1, range(min = 0.1, max = 10))]
    pub volumetric_extinction_scale: f32,
}
impl Default for HeightFog {
    fn default() -> Self {
        Self {
            density: 0.002,
            base_height: 0.0,
            falloff: 60.0,
            albedo: Color([0.9, 0.9, 0.9, 1.0]),
            anisotropy: 0.3,
            volumetric: true,
            volumetric_distance: 60.0,
            volumetric_emissive: [0.0; 3],
            volumetric_extinction_scale: 1.0,
        }
    }
}

/// The shape of a [`LocalFogVolume`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum FogVolumeShape {
    /// An ellipsoid filling `size`.
    #[default]
    Sphere,
    /// A box of `size`.
    Box,
}

/// A local volume of fog, mist or smoke placed by the entity's transform (Unreal's
/// Local Fog Volume). It is lit like height fog (sun shafts, lamps, the sky or probe
/// GI) and rendered in the volumetric fog; overlapping volumes add up. At most 16
/// render at once: the highest priority first, then the nearest to the camera.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalFogVolume {
    /// Whether the volume renders.
    #[ara(default = true)]
    pub enabled: bool,
    /// Sphere (an ellipsoid filling `size`) or box.
    pub shape: FogVolumeShape,
    /// Full size along the entity's axes, centred on its origin and scaled by its
    /// transform.
    #[ara(default = [10, 10, 10], unit = Meters, range(min = 0.02, max = 100000))]
    pub size: [f32; 3],
    /// Extinction at the centre, per metre: 0.05 is a light mist (visibility about
    /// 60 m), 1 a thick smoke (about 3 m).
    #[ara(default = 0.1, range(min = 0, max = 10), example = 0.2)]
    pub density: f32,
    /// Share of the radius, from the edge inwards, over which the fog thins to
    /// nothing: 0 is a hard edge, 1 fades all the way from the centre.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub radial_falloff: f32,
    /// How fast density falls with height above the centre (and rises below it), per
    /// metre: 0 is uniform; around 0.5 the fog pools near the bottom.
    #[ara(default = 0, range(min = 0, max = 10))]
    pub height_falloff: f32,
    /// Scattering colour (single-scattering albedo, linear).
    #[ara(default = [0.9, 0.9, 0.9, 1])]
    pub albedo: Color,
    /// Light the fog emits (linear radiance per metre at the centre's density), for
    /// glowing haze. Black emits none.
    #[ara(range(min = 0), editor(color, hdr))]
    pub emissive: [f32; 3],
    /// Which volumes render when more than 16 are in view: higher first, then nearer
    /// the camera.
    #[ara(default = 0, range(min = -1000, max = 1000))]
    pub priority: i32,
}
impl Default for LocalFogVolume {
    fn default() -> Self {
        Self {
            enabled: true,
            shape: FogVolumeShape::Sphere,
            size: [10.0; 3],
            density: 0.1,
            radial_falloff: 1.0,
            height_falloff: 0.0,
            albedo: Color([0.9, 0.9, 0.9, 1.0]),
            emissive: [0.0; 3],
            priority: 0,
        }
    }
}

/// A local fog volume as extracted for one frame: the component and its entity's
/// world pose. `SceneLook::fog_volumes` keeps them in entity ID order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogVolumeView {
    /// The authored volume.
    pub volume: LocalFogVolume,
    /// World position of the entity, in metres.
    pub translation: [f32; 3],
    /// World rotation of the entity (unit quaternion, x y z w).
    pub rotation: [f32; 4],
    /// World scale of the entity, multiplying `size`.
    pub scale: [f32; 3],
}

/// An authored reflection probe (High and Epic): the renderer captures the lit scene
/// around the entity into a six-face map, and smooth surfaces inside its box of
/// influence reflect it, so mirrors and metals show the room around them, including
/// what is off screen. The entity's Transform places the capture origin and orients
/// and scales the box. Captures are derived runtime data (a recapture job; cached
/// under `.ara/cache/probes`), never authored edits: a change to these settings, the
/// geometry, materials, lighting or environment marks the capture stale, and the last
/// valid capture stays in use until its replacement is published. Authored probes
/// always capture at High and Epic; a scene without any places captures itself where
/// its probe volume saw enclosed open space (`ARA_REFLECTION_CAPTURES=0` turns that
/// off).
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ReflectionProbe {
    /// Whether this probe contributes reflections. A disabled probe keeps its last
    /// valid capture but contributes nothing.
    #[ara(default = true)]
    pub enabled: bool,
    /// Full size of the box of influence along the entity's axes, centred on its
    /// origin and scaled by its transform: surfaces inside reflect this probe.
    #[ara(default = [10, 10, 10], unit = Meters, range(min = 0.01, max = 100000))]
    pub bounds: [f32; 3],
    /// Selection among overlapping probes: a larger priority wins; equal priorities
    /// blend by influence (ties ordered by document and entity identity).
    #[ara(default = 0, range(min = -1000, max = 1000))]
    pub priority: i32,
    /// Distance outside the box over which the probe's contribution fades to zero.
    /// Zero is a hard boundary.
    #[ara(default = 1, unit = Meters, range(min = 0, max = 1000))]
    pub blend_distance: f32,
    /// Texels per side of each captured face: one of 64, 128, 256, 512 or 1024
    /// ([`ReflectionProbe::RESOLUTIONS`]).
    #[ara(default = 256, range(min = 64, max = 1024))]
    pub capture_resolution: u32,
    /// Advanced: full size of the box reflections are projected onto (match the
    /// room's walls and floor). Zero uses `bounds`.
    #[ara(default = [0, 0, 0], unit = Meters, range(min = 0, max = 100000))]
    pub projection_bounds: [f32; 3],
    /// Advanced: centre of the projection box relative to the entity.
    #[ara(unit = Meters)]
    pub projection_offset: [f32; 3],
    /// Advanced: where the capture is taken, relative to the entity. Keep it in open
    /// space, near the reflective surfaces it serves.
    #[ara(unit = Meters)]
    pub capture_offset: [f32; 3],
    /// Advanced: brightness multiplier of the reflection.
    #[ara(default = 1, range(min = 0, max = 8))]
    pub intensity: f32,
}

impl ReflectionProbe {
    /// The supported `captureResolution` values (texels per face side).
    pub const RESOLUTIONS: [u32; 5] = [64, 128, 256, 512, 1024];

    /// Why these settings cannot be captured, or `None` if they can: nonfinite or
    /// out-of-range sizes, or a resolution outside [`Self::RESOLUTIONS`]. Checked
    /// before any allocation.
    pub fn invalid(&self) -> Option<String> {
        let size = |v: &[f32; 3], min: f32| {
            v.iter()
                .all(|x| x.is_finite() && (min..=100_000.0).contains(x))
        };
        if !size(&self.bounds, 0.01) {
            return Some(format!(
                "bounds {:?} must be finite and 0.01-100000 m",
                self.bounds
            ));
        }
        if !size(&self.projection_bounds, 0.0) {
            return Some(format!(
                "projectionBounds {:?} must be finite and 0-100000 m",
                self.projection_bounds
            ));
        }
        if !(self.blend_distance.is_finite() && (0.0..=1000.0).contains(&self.blend_distance)) {
            return Some(format!(
                "blendDistance {} must be finite and 0-1000 m",
                self.blend_distance
            ));
        }
        if !(-1000..=1000).contains(&self.priority) {
            return Some(format!("priority {} must be -1000..1000", self.priority));
        }
        if !Self::RESOLUTIONS.contains(&self.capture_resolution) {
            return Some(format!(
                "captureResolution {} must be one of {:?}",
                self.capture_resolution,
                Self::RESOLUTIONS
            ));
        }
        if !(self
            .projection_offset
            .iter()
            .chain(&self.capture_offset)
            .all(|x| x.is_finite())
            && self.intensity.is_finite()
            && (0.0..=8.0).contains(&self.intensity))
        {
            return Some("offsets must be finite and intensity 0-8".to_owned());
        }
        None
    }
}

impl Default for ReflectionProbe {
    fn default() -> Self {
        Self {
            enabled: true,
            bounds: [10.0; 3],
            priority: 0,
            blend_distance: 1.0,
            capture_resolution: 256,
            projection_bounds: [0.0; 3],
            projection_offset: [0.0; 3],
            capture_offset: [0.0; 3],
            intensity: 1.0,
        }
    }
}

/// How the camera exposes the scene (a post-process volume's exposure override).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum ExposureMode {
    /// Meter the frame's luminance and adapt within `minEv100`..`maxEv100`.
    #[default]
    Auto,
    /// Fixed photographic exposure `ev100`.
    Manual,
}

/// The display transform that compresses scene radiance into the screen's range.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum ToneMapping {
    /// Filmic ACES-style curve with a soft toe and highlight desaturation, similar to
    /// Unreal's default; the most cinematic.
    #[default]
    Filmic,
    /// Khronos PBR Neutral: keeps material colours accurate up to bright highlights.
    Neutral,
    /// Narkowicz's per-channel ACES fit (the legacy curve; saturated highlights).
    AcesFit,
    /// AgX as Blender 5.1 forms it (Cycles' default view): keeps mid grey, bends
    /// bright saturated light smoothly towards white instead of clipping its hue.
    Agx,
    /// AgX with Blender's Punchy look: deeper shadows and more contrast.
    AgxPunchy,
    /// AgX with Blender's Medium High Contrast look: contrast 1.2 about mid grey.
    AgxMediumHighContrast,
}

/// The scene's authored look as extracted for renderers: the first entity in ID order
/// carrying the sky and the fog, every local fog volume and every post-process volume
/// (blend them for a camera with `post_at`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SceneLook {
    /// Physical sky, when authored.
    pub sky: Option<SkyAtmosphere>,
    /// Height fog, when authored.
    pub fog: Option<HeightFog>,
    /// Post-process volumes with their entity poses, in entity ID order.
    pub volumes: Vec<crate::post::PostVolume>,
    /// Enabled local fog volumes with their entity poses, in entity ID order.
    pub fog_volumes: Vec<FogVolumeView>,
    /// The decoded lookup tables the volumes reference, by asset ID (shared with the
    /// runtime's library; never copied per frame).
    pub luts: Vec<(String, std::sync::Arc<crate::post::ColorLut>)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_schema::SchemaDb;

    #[test]
    fn a_version_one_sky_keeps_its_aerosol_density() {
        let mut db = SchemaDb::new();
        crate::register(&mut db);
        let ty = db.get("SkyAtmosphere").expect("registered");
        let mut value = serde_json::json!({"aerosols": 1.5, "cloudCoverage": 0.12});
        let mut notes = Vec::new();
        assert!(db.normalize_component(ty, 1, &mut value, "SkyAtmosphere", &mut notes));
        let sky: SkyAtmosphere = serde_json::from_value(value).expect("sky");
        assert!((sky.mie_scattering_scale - 0.005994).abs() < 1e-7);
        assert!((sky.mie_absorption_scale - 0.000666).abs() < 1e-7);
        assert_eq!(sky.cloud_coverage, 0.12);
        assert_eq!(sky.rayleigh_scattering_scale, 0.0331);
    }
}
