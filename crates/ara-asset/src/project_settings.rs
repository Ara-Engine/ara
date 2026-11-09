//! Reflected, command-authored project defaults, separate from source overrides.
use crate::{CookPreset, Error, Result};
use ara_doc::{AssetId, Document, RecordId};
use ara_schema::{DocKindDesc, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Project import policy document, editable through the inspector and typed commands.
pub const PROJECT_IMPORT_SETTINGS_FILE: &str = "settings/ara.imports";
/// Pre-layout project policy; accepted until `ara project migrate` relocates it.
pub const LEGACY_PROJECT_IMPORT_SETTINGS_FILE: &str = "ara.imports";

/// Defaults inherited by import sidecars which do not specify a preset.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "pi"), module = "asset")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ProjectImportSettings {
    /// Default texture encoding quality. Balanced opens new projects quickly;
    /// HighQuality performs the full search. Source/texture overrides take priority.
    pub preset: CookPreset,
    /// Typed importer-wide defaults. Explicit source settings take priority.
    pub defaults: ImporterDefaults,
}

/// Optional policies use the exact settings schemas consumed by installed workers.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "asset")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ImporterDefaults {
    /// PNG/JPEG/HDR/EXR texture import defaults; absent inherits built-in policy.
    pub texture: Option<crate::TextureSettings>,
    /// glTF model import defaults, including embedded texture policy.
    pub gltf: Option<crate::MeshSettings>,
    /// FBX model import defaults; glTF-only opacity maps are not supported.
    pub fbx: Option<crate::MeshSettings>,
    /// Audio use, codec, streaming bitrate and decoded-memory budget defaults.
    pub audio: Option<crate::AudioImportSettings>,
}

impl ProjectImportSettings {
    /// Merge typed per-importer policy with explicit source keys. Workers supply
    /// built-in defaults for every remaining absent field.
    pub fn effective_settings(
        &self,
        importer: &str,
        source: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let defaults = match importer {
            "ara.texture" => self
                .defaults
                .texture
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?,
            "ara.gltf" => self
                .defaults
                .gltf
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?,
            "ara.fbx" => self
                .defaults
                .fbx
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?,
            "ara.audio" => self
                .defaults
                .audio
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?,
            _ => None,
        };
        let mut effective = defaults.unwrap_or_else(|| serde_json::json!({}));
        if !source.is_null() {
            let overrides = source
                .as_object()
                .ok_or_else(|| Error::Invalid("source import settings must be an object".into()))?;
            for (key, value) in overrides {
                effective[key] = value.clone();
            }
        }
        Ok(effective)
    }
    /// Source texture keys win over source quality, then typed texture policy,
    /// then the global texture/model quality. Audio retains its engine preset.
    pub fn effective_preset(
        &self,
        importer: &str,
        source_preset: Option<CookPreset>,
        source: &serde_json::Value,
        effective: &serde_json::Value,
    ) -> Result<CookPreset> {
        if importer == "ara.texture"
            && let Some(value) = source.get("preset").filter(|value| !value.is_null())
        {
            return Ok(serde_json::from_value(value.clone())?);
        }
        if let Some(preset) = source_preset {
            return Ok(preset);
        }
        if importer == "ara.texture"
            && let Some(value) = effective.get("preset").filter(|value| !value.is_null())
        {
            return Ok(serde_json::from_value(value.clone())?);
        }
        Ok(
            if matches!(importer, "ara.texture" | "ara.gltf" | "ara.fbx") {
                self.preset
            } else {
                CookPreset::default()
            },
        )
    }
    /// Exact settings and quality supplied to dependency discovery and cooking.
    /// Resolve the worker's nested texture preset too, so a project field cannot
    /// override an explicit source quality or fragment cache invalidation.
    pub fn resolved_settings(
        &self,
        importer: &str,
        source_preset: Option<CookPreset>,
        source: &serde_json::Value,
    ) -> Result<(serde_json::Value, CookPreset)> {
        let mut settings = self.effective_settings(importer, source)?;
        let preset = self.effective_preset(importer, source_preset, source, &settings)?;
        if importer == "ara.texture"
            && settings.get("preset").is_some_and(|value| !value.is_null())
            && settings["preset"] != serde_json::json!(preset)
        {
            settings["preset"] = serde_json::json!(preset);
        }
        Ok((settings, preset))
    }
}

impl ProjectImportSettings {
    /// Reads the typed project policy; older projects inherit Balanced when absent.
    pub fn read(root: &Path, db: &SchemaDb) -> Result<Self> {
        let path = if root.join(PROJECT_IMPORT_SETTINGS_FILE).is_file() {
            PROJECT_IMPORT_SETTINGS_FILE
        } else {
            LEGACY_PROJECT_IMPORT_SETTINGS_FILE
        };
        let text = match std::fs::read_to_string(root.join(path)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        let read = ara_doc::read(&text, path, db, None);
        if read.diags.iter().any(ara_diag::Diagnostic::is_error) {
            return Err(Error::Invalid(
                read.diags
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }
        let doc = read
            .doc
            .ok_or_else(|| Error::Invalid("missing project import settings".into()))?;
        let record = doc
            .record("pi_settings")
            .ok_or_else(|| Error::Invalid("missing pi_settings record".into()))?;
        Ok(serde_json::from_value(serde_json::Value::Object(
            record.clone(),
        ))?)
    }

    /// Initial project document; callers persist it through the canonical writer.
    pub fn document(&self) -> Result<Document> {
        let mut doc = Document::new(
            "imports",
            AssetId::parse("ip_project").map_err(Error::Invalid)?,
        );
        let record = serde_json::to_value(self)?
            .as_object()
            .cloned()
            .ok_or_else(|| Error::Invalid("project import settings must be an object".into()))?;
        doc.insert_unchecked(
            RecordId::parse("pi_settings").map_err(Error::Invalid)?,
            record,
        );
        Ok(doc)
    }
    /// Canonical initial project policy for project/template creation.
    pub fn canonical_text(&self) -> Result<String> {
        let mut db = SchemaDb::new();
        crate::register_schema(&mut db);
        Ok(ara_doc::write(&self.document()?, &db))
    }
}

pub(crate) fn register(db: &mut SchemaDb) {
    db.register::<ProjectImportSettings>();
    if db.kind("imports").is_none() {
        let _ = db.add_kind(DocKindDesc {
            name: "imports".into(),
            version: 1,
            doc: "Typed per-importer project defaults and global texture quality, overridden by explicit source settings."
                .into(),
            id_prefix: "ip".into(),
            extension: "imports".into(),
            records: vec!["ProjectImportSettings".into()],
            single_root: false,
        });
    }
}
