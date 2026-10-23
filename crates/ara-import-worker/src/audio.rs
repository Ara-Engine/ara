//! ara-audio's cooker behind the ara-asset importer contract. Settings:
//! `{"usage": "shortEffect" | "music" | "dialogue", "codec": "pcm" | "adpcm",
//! "opusBitrate": 128000}`. Artifacts: `audio.bin` (the cooked WAV or Ogg Opus) and
//! `audio.json` (`codec`, `sampleRate`, `hash`).
use ara_asset::{
    Artifact, ArtifactKind, Error, ImportContext, ImportInput, ImportOutput, Importer,
};
use ara_audio::importer::{AudioUse, CookCodec, CookSettings, Importer as _};
use serde_json::{Value, json};

/// The `ara.audio` importer.
pub struct AudioAsset;

fn settings(value: &Value) -> Result<CookSettings, Error> {
    let mut settings = CookSettings::default();
    settings.usage = match value["usage"].as_str().unwrap_or("shortEffect") {
        "shortEffect" => AudioUse::ShortEffect,
        "music" => AudioUse::Music,
        "dialogue" => AudioUse::Dialogue,
        other => return Err(Error::Invalid(format!("unknown audio usage `{other}`"))),
    };
    settings.short_codec = match value["codec"].as_str().unwrap_or("pcm") {
        "pcm" => CookCodec::Pcm,
        "adpcm" => CookCodec::Adpcm,
        other => {
            return Err(Error::Invalid(format!(
                "short effects cook to pcm or adpcm, not `{other}`"
            )));
        }
    };
    if let Some(bitrate) = value["opusBitrate"].as_i64() {
        settings.opus_bitrate = i32::try_from(bitrate)
            .map_err(|_| Error::Invalid("opusBitrate out of range".into()))?;
    }
    Ok(settings)
}

impl Importer for AudioAsset {
    fn id(&self) -> &str {
        "ara.audio"
    }
    fn version(&self) -> &str {
        // ara-audio's cooker version (codec pins); bump with it.
        "2"
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
        let meta = json!({"codec": codec, "sampleRate": cooked.sample_rate, "hash": cooked.hash});
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
