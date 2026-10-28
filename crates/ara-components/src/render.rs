//! The authorable look of a scene: physical sky and height fog (the first entity in
//! stable-ID order carrying each), post-process volumes (`crate::post`, all of them),
//! reflection captures (all of them), plus the project's rendering quality setting. Renderers receive them through the
//! runtime extraction; the sun is the scene's directional light. Every field doc is an
//! inspector tooltip.

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
    /// Opt in to experimental hardware ray tracing for supported presets. False
    /// keeps every preset on raster and creates no ray-tracing presentation device.
    /// Takes effect when the player or editor GPU device is opened.
    pub ray_tracing: bool,
}

/// A physical sky and aerial perspective lit by the scene's directional light.
/// The sun's elevation sets the time of day; without this component the scene
/// uses its `Environment` gradient. The first entity in ID order wins.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SkyAtmosphere {
    /// World altitude of the planet surface (sea level); the sky and fog measure height from here.
    #[ara(unit = Meters, example = 0)]
    pub sea_level: f32,
    /// Aerosol (haze, dust) density relative to clear Earth air. Raise it for dusty
    /// or humid skies: a whiter horizon, a wider sun glow and stronger aerial perspective.
    #[ara(default = 1, range(min = 0, max = 20))]
    pub aerosols: f32,
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
            aerosols: 1.0,
            cloud_coverage: 0.0,
            cloud_height: 2200.0,
        }
    }
}

/// Exponential height fog lit by the sun, the sky and local lights (volumetric
/// shafts where shadows cut the beams). Density halves every `falloff × ln 2`
/// metres above `baseHeight`. The first entity in ID order wins.
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
}
impl Default for HeightFog {
    fn default() -> Self {
        Self {
            density: 0.002,
            base_height: 0.0,
            falloff: 60.0,
            albedo: Color([0.9, 0.9, 0.9, 1.0]),
            anisotropy: 0.3,
        }
    }
}

/// An authored reflection probe (High and Epic): the renderer captures the lit scene
/// around the entity into a six-face map, and smooth surfaces inside its box of
/// influence reflect it, so mirrors and metals show the room around them, including
/// what is off screen. The entity's Transform places the capture origin and orients
/// and scales the box. Captures are derived runtime data (a recapture job; cached
/// under `.ara/cache/probes`), never authored edits: a change to these settings, the
/// geometry, materials, lighting or environment marks the capture stale, and the last
/// valid capture stays in use until its replacement is published. Without any probe,
/// the renderer places captures over the scene's content automatically.
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
/// carrying the sky and the fog, and every post-process volume (blend them for a
/// camera with `post_at`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SceneLook {
    /// Physical sky, when authored.
    pub sky: Option<SkyAtmosphere>,
    /// Height fog, when authored.
    pub fog: Option<HeightFog>,
    /// Post-process volumes with their entity poses, in entity ID order.
    pub volumes: Vec<crate::post::PostVolume>,
    /// The decoded lookup tables the volumes reference, by asset ID (shared with the
    /// runtime's library; never copied per frame).
    pub luts: Vec<(String, std::sync::Arc<crate::post::ColorLut>)>,
}
