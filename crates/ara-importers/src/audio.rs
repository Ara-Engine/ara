//! ara-audio's cooker behind the ara-asset importer contract. Settings:
//! `{"usage": "shortEffect" | "music" | "dialogue", "codec": "pcm" | "adpcm",
//! "opusBitrate": 128000}`. Artifacts: `audio.bin` (the cooked WAV or Ogg Opus) and
//! `audio.json` (`codec`, `sampleRate`, `hash`).
use ara_asset::{
    Artifact, ArtifactKind, Error, ImportContext, ImportInput, ImportOutput, Importer,
};
use ara_audio::importer::{AudioUse, CookCodec, CookSettings, Importer as _};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Audio use selects resident effects or streamed music/dialogue.
#[derive(Debug, Clone, Copy, Default, Schema, Serialize, Deserialize)]
#[ara(module = "asset", name = "AudioImportUse")]
#[serde(rename_all = "camelCase")]
pub enum Usage {
    /// Resident sound effect.
    #[default]
    ShortEffect,
    /// Streamed music encoded as Opus.
    Music,
    /// Streamed dialogue encoded as Opus.
    Dialogue,
}

/// Codec for resident sound effects; streamed audio always uses Opus.
#[derive(Debug, Clone, Copy, Default, Schema, Serialize, Deserialize)]
#[ara(module = "asset", name = "AudioEffectCodec")]
#[serde(rename_all = "camelCase")]
pub enum EffectCodec {
    /// Signed 16-bit PCM.
    #[default]
    Pcm,
    /// IMA ADPCM.
    Adpcm,
}

/// Audio sidecar settings, validated identically by authoring commands and workers.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(module = "asset", name = "AudioImportSettings")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// Resident effect, streamed music or streamed dialogue.
    pub usage: Usage,
    /// Codec for resident effects.
    pub codec: EffectCodec,
    /// Stereo Opus bitrate for music/dialogue, in bits per second.
    #[ara(default = 128000, range(min = 6000, max = 512000))]
    pub opus_bitrate: i32,
    /// Resident decoded PCM budget in bytes; streamed cooking uses bounded blocks.
    #[ara(default = 67108864, range(min = 65536))]
    pub decoded_budget: u64,
}
impl Default for Settings {
    fn default() -> Self {
        let defaults = CookSettings::default();
        Self {
            usage: Usage::default(),
            codec: EffectCodec::default(),
            opus_bitrate: defaults.opus_bitrate,
            decoded_budget: defaults.decoded_budget as u64,
        }
    }
}

/// The `ara.audio` importer.
pub struct AudioAsset {
    version: String,
}

impl Default for AudioAsset {
    fn default() -> Self {
        Self {
            version: ara_audio::importer::AudioImporter.version().to_string(),
        }
    }
}

pub(crate) fn settings(value: &Value) -> Result<CookSettings, Error> {
    let typed: Settings = serde_json::from_value(if value.is_null() {
        json!({})
    } else {
        value.clone()
    })?;
    if !(6000..=512000).contains(&typed.opus_bitrate) || typed.decoded_budget < 65536 {
        return Err(Error::Invalid("audio settings need a 6000..512000 Opus bitrate and decodedBudget of at least 65536 bytes".into()));
    }
    Ok(CookSettings {
        usage: match typed.usage {
            Usage::ShortEffect => AudioUse::ShortEffect,
            Usage::Music => AudioUse::Music,
            Usage::Dialogue => AudioUse::Dialogue,
        },
        short_codec: match typed.codec {
            EffectCodec::Pcm => CookCodec::Pcm,
            EffectCodec::Adpcm => CookCodec::Adpcm,
        },
        opus_bitrate: typed.opus_bitrate,
        decoded_budget: usize::try_from(typed.decoded_budget).map_err(|_| {
            Error::Invalid("decodedBudget exceeds this host's address space".into())
        })?,
    })
}
impl Importer for AudioAsset {
    fn id(&self) -> &str {
        "ara.audio"
    }
    fn version(&self) -> &str {
        &self.version
    }
    fn register_settings(&self, db: &mut ara_schema::SchemaDb) {
        db.register::<Settings>();
    }
    fn import(
        &self,
        input: &ImportInput,
        _: &mut ImportContext<'_>,
    ) -> ara_asset::Result<ImportOutput> {
        let extension = std::path::Path::new(&input.source_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let cooked = ara_audio::importer::AudioImporter
            .import(&ara_audio::importer::ImportInput {
                extension: &extension,
                bytes: &input.source,
                settings: settings(&input.settings)?,
            })
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let codec = match cooked.codec {
            CookCodec::Opus => "opus",
            CookCodec::Adpcm => "adpcm",
            CookCodec::Pcm => "pcm",
        };
        let meta = json!({
            "codec": codec, "sampleRate": cooked.sample_rate, "hash": cooked.hash,
            "streamed": cooked.codec == CookCodec::Opus,
            "loudness": cooked.loudness, "peak": cooked.peak,
        });
        Ok(ImportOutput {
            artifacts: vec![
                Artifact {
                    name: "audio.bin".into(),
                    kind: ArtifactKind::Custom,
                    bytes: cooked.bytes,
                },
                Artifact {
                    name: "audio.json".into(),
                    kind: ArtifactKind::Custom,
                    bytes: ara_json::to_canonical_string(&meta).into_bytes(),
                },
            ],
        })
    }
}
