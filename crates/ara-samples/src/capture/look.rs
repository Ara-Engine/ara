//! `<project>/look/<scene>.look.json`: what the fidelity captures need that a scene
//! document cannot express yet: camera shots, sky (physical atmosphere or an HDRI),
//! fog, exposure and tone mapping, the ground material and transport switches.
//! Scenes own geometry, lights, the sun and gameplay; looks only frame and grade them.
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
    /// Sky and image-based lighting.
    pub sky: Sky,
    /// Exposure, tone mapping and lens.
    #[serde(default)]
    pub post: Post,
    /// Ray-traced many-light sampling (ReSTIR DI/GI) for scenes lit by many emitters.
    #[serde(default)]
    pub reservoirs: bool,
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
    /// Homogeneous frustum fog extinction (1/m), in addition to the atmosphere.
    #[serde(default)]
    pub fog_density: f32,
}

impl Default for Post {
    fn default() -> Self {
        Self {
            ev100: None,
            compensation: 0.0,
            bloom: default_bloom(),
            neutral: false,
            fog_density: 0.0,
        }
    }
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
