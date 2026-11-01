//! Shared reflected importer policies; workers and project defaults use the same types.
use crate::CookPreset;
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Semantic texture usage selects transfer functions, mip filtering and BC format.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ara_schema::Schema,
)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase")]
pub enum TextureUsage {
    /// sRGB color (alpha remains linear), BC7 sRGB.
    #[default]
    Color,
    /// Linear RGBA data, BC7 UNORM.
    Linear,
    /// Tangent-space normals, normalized mips and BC5 XY.
    Normal,
    /// A single linear scalar channel, BC4.
    Mask,
    /// Nonnegative linear radiance, BC6H UFLOAT.
    Hdr,
    /// A colour lookup table strip (post-process volumes' grading): values kept as
    /// authored, uncompressed RGBA half floats, without mips.
    Lut,
}
/// Declarative texture import settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ara_schema::Schema)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct TextureSettings {
    /// Optional quality override; absent inherits the source/project preset.
    /// Changing it does not re-encode other texture tasks.
    pub preset: Option<CookPreset>,
    /// How color/normal/mask values are interpreted.
    pub usage: TextureUsage,
    /// Generate the complete mip chain through 1×1.
    #[ara(default = true)]
    pub mipmaps: bool,
    /// Invert normal Y for DirectX-authored tangent-space inputs.
    pub flip_normal_y: bool,
    /// Source channel for BC4, R=0/G=1/B=2/A=3.
    #[ara(range(min = 0, max = 3))]
    pub mask_channel: u8,
    /// Maximum permitted source dimension (decode limits are also enforced).
    #[ara(default = 16384, range(min = 1, max = 16384))]
    pub max_dimension: u32,
    /// Alpha-test cutoff whose coverage every mip keeps: a level whose averaged alpha
    /// would pass fewer (or more) texels than the full-size image has its partial alpha
    /// remapped until it passes as many, so masked fences and foliage stay as solid at a
    /// distance as up close, and a grate's holes stay open instead of averaging shut
    /// (Unity's "preserve coverage", Unreal's "preserve alpha coverage").
    /// Model importers set it for the base colour of alpha-masked materials. Absent:
    /// alpha mips are plain averages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha_coverage: Option<f32>,
}
impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            preset: None,
            usage: TextureUsage::Color,
            mipmaps: true,
            flip_normal_y: false,
            mask_channel: 0,
            max_dimension: 16384,
            alpha_coverage: None,
        }
    }
}

/// Deterministic geometry cook policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ara_schema::Schema)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct MeshSettings {
    /// Texture quality overrides by image URI or #image-index (FBX: #texture-id).
    /// Other images inherit the model/project preset and retain their cached work.
    #[serde(default)]
    pub texture_presets: std::collections::BTreeMap<String, CookPreset>,
    /// Maximum LODs including the original (1–8). Skin/morph meshes retain at most four.
    #[ara(default = 8, range(min = 1, max = 8))]
    pub lod_count: u32,
    /// Relative simplification error ceiling; skin/morph meshes clamp this to 1%.
    #[ara(default = 0.1, range(min = 0))]
    pub lod_error: f32,
    /// Cook a versioned virtual-geometry DAG for each static material primitive.
    /// Skinned and morph-deformed meshes retain the classic path.
    #[ara(default = true)]
    pub virtual_geometry: bool,
    /// glTF base-color image URI (or #image-index) to companion opacity image URI.
    /// The mask's red channel multiplies existing alpha; paths are relative to the model.
    #[serde(default)]
    pub opacity_maps: std::collections::BTreeMap<String, String>,
}
impl Default for MeshSettings {
    fn default() -> Self {
        Self {
            texture_presets: std::collections::BTreeMap::new(),
            lod_count: 8,
            lod_error: 0.1,
            virtual_geometry: true,
            opacity_maps: std::collections::BTreeMap::new(),
        }
    }
}

/// Audio use selects resident effects or streamed music/dialogue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "asset", name = "AudioImportUse")]
#[serde(rename_all = "camelCase")]
pub enum AudioImportUse {
    /// Resident sound effect.
    #[default]
    ShortEffect,
    /// Streamed music encoded as Opus.
    Music,
    /// Streamed dialogue encoded as Opus.
    Dialogue,
}

/// Codec for resident sound effects; streamed audio always uses Opus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "asset", name = "AudioEffectCodec")]
#[serde(rename_all = "camelCase")]
pub enum AudioEffectCodec {
    /// Signed 16-bit PCM.
    #[default]
    Pcm,
    /// IMA ADPCM.
    Adpcm,
}

/// Audio sidecar settings, validated identically by authoring commands and workers.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "asset", name = "AudioImportSettings")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioImportSettings {
    /// Resident effect, streamed music or streamed dialogue.
    pub usage: AudioImportUse,
    /// Codec for resident effects.
    pub codec: AudioEffectCodec,
    /// Stereo Opus bitrate for music/dialogue, in bits per second.
    #[ara(default = 128000, range(min = 6000, max = 512000))]
    pub opus_bitrate: i32,
    /// Resident decoded PCM budget in bytes; streamed cooking uses bounded blocks.
    #[ara(default = 67108864, range(min = 65536))]
    pub decoded_budget: u64,
}
impl Default for AudioImportSettings {
    fn default() -> Self {
        Self {
            usage: AudioImportUse::default(),
            codec: AudioEffectCodec::default(),
            opus_bitrate: 128000,
            decoded_budget: 64 * 1024 * 1024,
        }
    }
}
