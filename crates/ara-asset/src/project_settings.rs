//! Reflected, command-authored project defaults, separate from source overrides.
use crate::{CookPreset, Error, Result};
use ara_doc::{AssetId, Document, RecordId};
use ara_schema::{DocKindDesc, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Project import policy document, editable through the inspector and typed commands.
pub const PROJECT_IMPORT_SETTINGS_FILE: &str = "ara.imports";
/// Optional scoped policy, read without changing the default creation location.
pub(crate) const SCOPED_PROJECT_IMPORT_SETTINGS_FILE: &str = "settings/ara.imports";

/// Defaults inherited by import sidecars which do not specify a preset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "pi"), module = "asset")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ProjectImportSettings {
    /// Default texture encoding quality. Balanced opens new projects quickly;
    /// HighQuality performs the full search. Source/texture overrides take priority.
    pub preset: CookPreset,
}

impl ProjectImportSettings {
    /// Reads the typed project policy; older projects inherit Balanced when absent.
    pub fn read(root: &Path, db: &SchemaDb) -> Result<Self> {
        let path = if root.join(SCOPED_PROJECT_IMPORT_SETTINGS_FILE).is_file() {
            SCOPED_PROJECT_IMPORT_SETTINGS_FILE
        } else {
            PROJECT_IMPORT_SETTINGS_FILE
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
    pub fn document(self) -> Result<Document> {
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
    pub fn canonical_text(self) -> Result<String> {
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
            doc: "Project texture import defaults, overridden by individual sources/textures."
                .into(),
            id_prefix: "ip".into(),
            extension: "imports".into(),
            records: vec!["ProjectImportSettings".into()],
            single_root: false,
        });
    }
}
