//! The authorable look of a scene: physical sky and height fog (the first entity in
//! stable-ID order carrying each), post-process volumes (`crate::post`, all of them),
//! plus the project's rendering quality setting. Renderers receive them through the
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
