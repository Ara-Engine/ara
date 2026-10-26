//! Post-process volumes (Unity's Volume, Unreal's PostProcessVolume): where a set of
//! post-processing overrides applies and how strongly. The runtime extraction gathers
//! every volume (`SceneLook::volumes`); renderers blend them for the camera they draw
//! from with `SceneLook::post_at`. Every field doc is an inspector tooltip.

mod blend;
pub use blend::{PostValues, PostVolume, PresetBlend};
#[cfg(test)]
mod tests;

use crate::render::{ExposureMode, ToneMapping};
use ara_schema::{AssetRef, Color, Schema};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Where a post-process volume applies.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum VolumeShape {
    /// Everywhere, wherever the camera is.
    #[default]
    Global,
    /// Inside a 1 m cube centred on the entity, sized and turned by its transform.
    Box,
    /// Inside a 1 m diameter sphere centred on the entity, sized by its transform's scale.
    Sphere,
}

/// The group of settings one `PostOverride` entry changes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum PostModule {
    /// Automatic metering or a fixed exposure, compensation and adaptation.
    #[default]
    Exposure,
    /// The display transform.
    ToneMapping,
    /// The glow around bright pixels.
    Bloom,
    /// White balance, saturation, contrast, lift/gamma/gain and a lookup table.
    ColorGrading,
    /// Darkening toward the frame corners.
    Vignette,
    /// Lens focus blur.
    DepthOfField,
    /// Blur along screen motion while the shutter is open.
    MotionBlur,
    /// Colour fringes toward the frame edges.
    ChromaticAberration,
    /// Screen-space ambient occlusion strength and radius (presets that run it).
    AmbientOcclusion,
    /// Screen-space reflection strength and reach (presets that run them).
    Reflections,
}

/// One group of settings a post-process volume overrides: pick the group in `module`,
/// then tick the properties to change. Unticked properties are left to
/// lower-priority volumes and the defaults.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PostOverride {
    /// The settings group this entry changes.
    pub module: PostModule,
    /// Automatic metering or a fixed exposure.
    #[ara(example = "auto", editor(show_if(field = "module", one_of = ["exposure"])))]
    pub exposure: Option<ExposureMode>,
    /// Manual exposure value at ISO 100 (manual mode). Sunny day ≈ 15, overcast ≈ 12,
    /// sunset ≈ 10, lit interior ≈ 7, night street ≈ 3.
    #[ara(range(min = -10, max = 24), example = 10, editor(show_if(field = "module", one_of = ["exposure"])))]
    pub ev100: Option<f32>,
    /// Exposure compensation in stops, applied in both modes; +1 doubles the brightness.
    #[ara(range(min = -8, max = 8), example = 0, editor(show_if(field = "module", one_of = ["exposure"])))]
    pub compensation: Option<f32>,
    /// Darkest exposure automatic metering may pick (lower = brighter in dark scenes).
    #[ara(range(min = -10, max = 24), example = -2, editor(show_if(field = "module", one_of = ["exposure"])))]
    pub min_ev100: Option<f32>,
    /// Brightest exposure automatic metering may pick.
    #[ara(range(min = -10, max = 24), example = 16, editor(show_if(field = "module", one_of = ["exposure"])))]
    pub max_ev100: Option<f32>,
    /// How fast automatic exposure adapts, per second; higher reacts faster.
    #[ara(range(min = 0.1, max = 20), example = 2, editor(show_if(field = "module", one_of = ["exposure"])))]
    pub adaptation_speed: Option<f32>,
    /// Display transform that compresses scene radiance into the screen's range.
    #[ara(example = "filmic", editor(show_if(field = "module", one_of = ["toneMapping"])))]
    pub tone_mapping: Option<ToneMapping>,
    /// Fraction of light spread into the bloom glow around bright pixels.
    #[ara(range(min = 0, max = 0.25), example = 0.04, editor(show_if(field = "module", one_of = ["bloom"])))]
    pub bloom: Option<f32>,
    /// White balance: the colour temperature that renders as neutral white. Lower warms
    /// the image's whites to neutral (tungsten ≈ 3200), higher cools them.
    #[ara(unit = Kelvin, range(min = 1500, max = 15000), example = 6500, editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub white_balance: Option<f32>,
    /// Colour saturation; 0 is greyscale, 1 unchanged.
    #[ara(range(min = 0, max = 2), example = 1, editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub saturation: Option<f32>,
    /// Contrast around middle grey; 1 unchanged.
    #[ara(range(min = 0.5, max = 2), example = 1, editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub contrast: Option<f32>,
    /// Lift: a colour added to the shadows after tone mapping, fading out toward
    /// white; black leaves them unchanged.
    #[ara(example = [0, 0, 0, 1], editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub lift: Option<Color>,
    /// Gamma: a per-channel midtone power after tone mapping; white is neutral, a
    /// darker channel darkens that channel's midtones.
    #[ara(example = [1, 1, 1, 1], editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub gamma: Option<Color>,
    /// Gain: a per-channel multiplier on the whole image after tone mapping; white is neutral.
    #[ara(example = [1, 1, 1, 1], editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub gain: Option<Color>,
    /// Colour lookup table applied last, as in Unity and Unreal: a strip texture of 16
    /// (256x16) or 32 (1024x32) sRGB slices along red and green, one slice per blue
    /// step, imported uncompressed.
    #[ara(asset_kind = "texture", editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub lut: Option<AssetRef>,
    /// How much of the lookup table applies, from 0 (none) to 1 (all).
    #[ara(range(min = 0, max = 1), example = 1, editor(show_if(field = "module", one_of = ["colorGrading"])))]
    pub lut_contribution: Option<f32>,
    /// Darkening toward the frame corners; 0 disables it.
    #[ara(range(min = 0, max = 1), example = 0.3, editor(show_if(field = "module", one_of = ["vignette"])))]
    pub vignette: Option<f32>,
    /// Distance from the camera that is in focus; 0 turns depth of field off.
    #[ara(unit = Meters, range(min = 0, max = 10000), example = 10, editor(show_if(field = "module", one_of = ["depthOfField"])))]
    pub focus_distance: Option<f32>,
    /// Lens aperture as an f-number: smaller numbers blur more (f/1.4 is shallow,
    /// f/16 keeps nearly everything sharp). The focal length follows the camera's
    /// field of view on a full-frame sensor.
    #[ara(range(min = 0.7, max = 32), example = 2.8, editor(show_if(field = "module", one_of = ["depthOfField"])))]
    pub aperture: Option<f32>,
    /// Strength of the blur in front of the focus distance, from 0 (sharp) to 1 (physical).
    #[ara(range(min = 0, max = 1), example = 1, editor(show_if(field = "module", one_of = ["depthOfField"])))]
    pub near_blur: Option<f32>,
    /// Strength of the blur behind the focus distance, from 0 (sharp) to 1 (physical).
    #[ara(range(min = 0, max = 1), example = 1, editor(show_if(field = "module", one_of = ["depthOfField"])))]
    pub far_blur: Option<f32>,
    /// Shutter angle: 180° blurs motion over half a frame, 360° over a whole frame;
    /// 0 turns motion blur off. Samples per pixel follow the quality preset.
    #[ara(unit = Degrees, range(min = 0, max = 360), example = 180, editor(show_if(field = "module", one_of = ["motionBlur"])))]
    pub shutter_angle: Option<f32>,
    /// Strength of the red/blue fringes toward the frame edges; 0 turns it off.
    #[ara(range(min = 0, max = 1), example = 0.5, editor(show_if(field = "module", one_of = ["chromaticAberration"])))]
    pub chromatic_aberration: Option<f32>,
    /// Ambient occlusion strength: 0 none, 1 physical, above 1 exaggerated.
    #[ara(range(min = 0, max = 4), example = 1, editor(show_if(field = "module", one_of = ["ambientOcclusion"])))]
    pub ao_intensity: Option<f32>,
    /// Ambient occlusion search radius.
    #[ara(unit = Meters, range(min = 0.05, max = 10), example = 1, editor(show_if(field = "module", one_of = ["ambientOcclusion"])))]
    pub ao_radius: Option<f32>,
    /// Screen-space reflection strength, from 0 (probe reflections only) to 1.
    #[ara(range(min = 0, max = 1), example = 1, editor(show_if(field = "module", one_of = ["reflections"])))]
    pub reflection_intensity: Option<f32>,
    /// Longest screen-space reflection ray.
    #[ara(unit = Meters, range(min = 1, max = 500), example = 30, editor(show_if(field = "module", one_of = ["reflections"])))]
    pub reflection_distance: Option<f32>,
}

/// A region whose post-processing overrides blend in as the camera approaches and
/// enters it. Volumes apply in ascending `priority` (ties in entity ID order); each
/// property one overrides moves toward its value by the volume's weight at the camera.
/// Properties no volume overrides keep the engine defaults. Scenes saved with the
/// former `PostProcess` component load as one global volume that overrides everything.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render", version = 2, alias = "PostProcess", upgrade = upgrade_post_process)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PostProcessVolume {
    /// Where the volume applies: everywhere, or inside a box or sphere placed and sized
    /// by the entity's transform.
    pub shape: VolumeShape,
    /// Distance outside the box or sphere over which the volume fades out; 0 is a hard edge.
    #[ara(unit = Meters, range(min = 0, max = 1000), editor(show_if(field = "shape", one_of = ["box", "sphere"])))]
    pub blend_distance: f32,
    /// Order among overlapping volumes: higher priorities apply after, and over, lower ones.
    #[ara(range(min = -1000, max = 1000))]
    pub priority: f32,
    /// Overall strength of the volume's overrides, from 0 (none) to 1 (full).
    #[ara(default = 1, range(min = 0, max = 1))]
    pub weight: f32,
    /// The settings this volume changes, one entry per group. Anything not listed comes
    /// from lower-priority volumes or the defaults.
    pub overrides: Vec<PostOverride>,
}
impl Default for PostProcessVolume {
    fn default() -> Self {
        Self {
            shape: VolumeShape::Global,
            blend_distance: 0.0,
            priority: 0.0,
            weight: 1.0,
            overrides: Vec::new(),
        }
    }
}

/// Version 1 was the scene-wide `PostProcess` component (flat exposure, display and
/// grading fields). It becomes a global volume overriding every one of them, the
/// former defaults included, so a migrated scene renders as before.
fn upgrade_post_process(from: u32, v: &mut Value) -> Result<(), String> {
    if from != 1 {
        return Ok(());
    }
    let old = std::mem::take(v);
    let d = PostValues::default();
    let get = |key: &str, default: Value| old.get(key).cloned().unwrap_or(default);
    *v = json!({
        "overrides": [
            {
                "module": "exposure",
                "exposure": get("exposure", json!(d.exposure)),
                "ev100": get("ev100", json!(d.ev100)),
                "compensation": get("compensation", json!(d.compensation)),
                "minEv100": get("minEv100", json!(d.min_ev100)),
                "maxEv100": get("maxEv100", json!(d.max_ev100)),
                "adaptationSpeed": get("adaptationSpeed", json!(d.adaptation_speed)),
            },
            {"module": "toneMapping", "toneMapping": get("toneMapping", json!(d.tone_mapping))},
            {"module": "bloom", "bloom": get("bloom", json!(d.bloom))},
            {
                "module": "colorGrading",
                "whiteBalance": get("whiteBalance", json!(d.white_balance)),
                "saturation": get("saturation", json!(d.saturation)),
                "contrast": get("contrast", json!(d.contrast)),
            },
            {"module": "vignette", "vignette": get("vignette", json!(d.vignette))},
        ]
    });
    Ok(())
}
