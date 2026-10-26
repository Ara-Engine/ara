//! `<project>/look/<scene>.look.json`: what the captures need that a scene document
//! cannot express yet: camera shots, an HDRI sky and the ground material. Scenes own
//! geometry, lights, the sun, the physical sky, fog and post processing
//! (`SkyAtmosphere`, `HeightFog`, `PostProcessVolume`); `sky.kind = atmosphere` and
//! `post` remain as overrides for looks that predate those components.
use ara_render_fidelity::{ToneMapper, atmosphere::Atmosphere, environment::FrameEnvironment};
use serde::Deserialize;

/// Capture settings for one scene.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Look {
    /// Project-relative scene document (`levels/outpost.scene`).
    pub scene: String,
    /// Simulated ticks before capturing (physics settles, animation poses).
    #[serde(default = "default_ticks")]
    pub ticks: u32,
    /// Shot and target heights are metres above the terrain at their X/Z.
    #[serde(default)]
    pub ground_relative: bool,
    /// Sky and image-based lighting; absent uses the scene's own sky.
    #[serde(default)]
    pub sky: Option<Sky>,
    /// Exposure and display override; absent uses the scene's post-process volumes.
    pub post: Option<Post>,
    /// Look-component overrides applied over the scene's own while tuning a look;
    /// the scene owns the final values.
    #[serde(default)]
    pub components: Components,
    /// Directional light illuminance override (linear RGB lux), for tuning.
    pub sun_lux: Option<[f32; 3]>,
    /// Directional light direction override (toward the sun), for tuning.
    pub sun_direction: Option<[f32; 3]>,
    /// `VisualEffect` scale/intensity overrides by effect name, for tuning.
    #[serde(default)]
    pub effects: std::collections::BTreeMap<String, EffectTuning>,
    /// Ground material for terrain tiles (one tiled texture set).
    pub terrain: Option<Ground>,
    /// Material overrides by model asset ID (layers the scene schema cannot carry).
    #[serde(default)]
    pub details: Vec<Detail>,
    /// Still shots.
    pub shots: Vec<Shot>,
    /// A short camera move captured as a strip.
    pub sequence: Option<Sequence>,
}

fn default_ticks() -> u32 {
    60
}

/// Where sky radiance comes from.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields,
    tag = "kind"
)]
pub enum Sky {
    /// Physical sky, aerial perspective and height fog lit by the scene's sun.
    Atmosphere {
        /// Sea-level fog extinction (1/m).
        fog_extinction: f32,
        /// Fog scale height (m).
        fog_height: f32,
        /// Fog single-scattering albedo.
        #[serde(default = "default_albedo")]
        fog_albedo: f32,
        /// Cloud coverage, 0 disables clouds.
        #[serde(default)]
        cloud_coverage: f32,
        /// World altitude of sea level (m).
        #[serde(default)]
        sea_level: f32,
    },
    /// An equirectangular HDRI from the lock, scaled to cd/m².
    Hdri {
        /// Lock asset ID.
        asset: String,
        /// Radiance scale (HDR value 1 → this many cd/m²).
        intensity: f32,
        /// Rotation about +Y in degrees.
        #[serde(default)]
        rotation: f32,
    },
    /// The scene's own `Environment` gradient.
    Scene,
}

fn default_albedo() -> f32 {
    0.9
}

/// Exposure and display transform.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Post {
    /// Manual EV100; absent meters automatically.
    pub ev100: Option<f32>,
    /// Exposure compensation (stops).
    #[serde(default)]
    pub compensation: f32,
    /// Bloom energy fraction.
    #[serde(default = "default_bloom")]
    pub bloom: f32,
    /// Khronos PBR Neutral instead of the ACES fit.
    #[serde(default)]
    pub neutral: bool,
}

fn default_bloom() -> f32 {
    0.04
}

/// The terrain's tiled ground material, plus an optional rock material for cells
/// painted rocky (splat channel 1) or steep.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ground {
    /// Lock ID of a texture set.
    pub texture: String,
    /// Colour multiplier.
    #[serde(default = "white")]
    pub tint: [f32; 3],
    /// Lock ID of the rock texture set.
    pub rock: Option<String>,
    /// Rock colour multiplier.
    #[serde(default = "white")]
    pub rock_tint: [f32; 3],
}

/// Replacement `SkyAtmosphere`/`HeightFog`/`PostProcessVolume` values (partial JSON
/// fills the component defaults).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct Components {
    /// Physical sky.
    pub sky_atmosphere: Option<ara_components::render::SkyAtmosphere>,
    /// Height fog.
    pub height_fog: Option<ara_components::render::HeightFog>,
    /// A volume replacing every post-process volume of the scene (placed at the origin).
    pub post_process_volume: Option<ara_components::post::PostProcessVolume>,
}

/// A `VisualEffect`'s size and brightness.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectTuning {
    /// Uniform scale.
    pub scale: f32,
    /// Radiance multiplier.
    pub intensity: f32,
}

impl Look {
    /// The scene's extracted look with this look's component overrides.
    pub fn scene_look(
        &self,
        scene: ara_components::render::SceneLook,
    ) -> ara_components::render::SceneLook {
        let c = &self.components;
        ara_components::render::SceneLook {
            sky: c.sky_atmosphere.or(scene.sky),
            fog: c.height_fog.or(scene.fog),
            volumes: match &c.post_process_volume {
                Some(volume) => vec![ara_components::post::PostVolume {
                    volume: volume.clone(),
                    translation: [0.; 3],
                    rotation: [0., 0., 0., 1.],
                    scale: [1.; 3],
                }],
                None => scene.volumes,
            },
        }
    }
    /// Applies effect overrides to the extracted effects.
    pub fn tune_effects(&self, effects: &mut [ara_runtime::extract::EffectView]) {
        for e in effects {
            if let Some(t) = self.effects.get(&e.effect) {
                e.scale = t.scale;
                e.intensity = t.intensity;
            }
        }
    }
    /// Applies this look's legacy sky/post overrides on top of the scene's environment.
    pub fn override_environment(&self, environment: &mut FrameEnvironment) {
        if let Some(Sky::Atmosphere {
            fog_extinction,
            fog_height,
            fog_albedo,
            cloud_coverage,
            sea_level,
        }) = self.sky
        {
            environment.atmosphere = Some(Atmosphere {
                sea_level,
                fog_extinction,
                fog_height,
                fog_albedo,
                cloud_coverage,
                cloud_height: 2200.0,
                fog_base: sea_level,
                ..Atmosphere::default()
            });
        }
        if let Some(post) = &self.post {
            environment.ev100 = post.ev100;
            environment.compensation = post.compensation;
            environment.bloom = post.bloom;
            environment.tone_mapper = if post.neutral {
                ToneMapper::PbrNeutral
            } else {
                ToneMapper::AcesFit
            };
        }
    }
}

fn white() -> [f32; 3] {
    [1.0; 3]
}

/// Material layers for every material of one model asset.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Detail {
    /// Model asset ID (`as_…`).
    pub asset: String,
    /// Clearcoat coverage.
    #[serde(default)]
    pub clearcoat: f32,
    /// Cloth sheen colour.
    #[serde(default)]
    pub sheen: [f32; 3],
    /// Thin-sheet diffuse transmission (leaves).
    #[serde(default)]
    pub transmission: f32,
}

/// One still.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Shot {
    /// File stem of the capture.
    pub name: String,
    /// Eye position (m).
    pub position: [f32; 3],
    /// Aim point (m).
    pub target: [f32; 3],
    /// Vertical field of view (degrees).
    #[serde(default = "default_fov")]
    pub fov: f32,
}

fn default_fov() -> f32 {
    50.0
}

/// A dolly between two shots, sampled into a strip.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sequence {
    /// Start shot name.
    pub from: String,
    /// End shot name.
    pub to: String,
    /// Frames in the strip.
    pub frames: u32,
    /// Rendered frames between strip frames (temporal history keeps running).
    #[serde(default = "default_step")]
    pub step: u32,
}

fn default_step() -> u32 {
    8
}
