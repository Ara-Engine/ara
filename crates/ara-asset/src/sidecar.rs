use crate::{Error, Result};
use ara_doc::{AssetId, AssetIndex, Document, Edit, RecordId};
use ara_schema::{AssetRef, DocKindDesc, FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};

/// Declarative importer configuration, stored as the single `i_settings` record.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "i"), module = "asset")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ImportSettings {
    /// Texture encoding quality override. Omit this field to inherit the project
    /// preset in settings/ara.imports; Balanced is the default for new projects.
    pub preset: Option<crate::CookPreset>,
    /// Stable worker importer ID.
    pub importer: String,
    /// Exact importer version, including encoder implementation versions.
    pub version: String,
    /// Importer-specific typed settings, canonicalized recursively.
    pub settings: serde_json::Value,
    /// Referenced authoring assets, using authored-root-relative `@path#id` syntax.
    pub dependencies: Vec<AssetRef>,
}
impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            preset: None,
            importer: String::new(),
            version: String::new(),
            settings: serde_json::Value::Null,
            dependencies: Vec::new(),
        }
    }
}
/// How the installed importer relates to the version a sidecar records. A version's
/// generation is its leading number (`3` in `3-gltf14-…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionCheck {
    /// Identical: cook as recorded.
    Current,
    /// The installed importer is newer (a higher generation, or the same generation
    /// with other encoder versions): reimport with it and record it
    /// (`SidecarCommand::Version`).
    Upgrade,
    /// The sidecar was written by a newer importer generation than is installed.
    Newer,
}
impl ImportSettings {
    /// Compares the recorded version with the `installed` importer's version.
    pub fn check_version(&self, installed: &str) -> VersionCheck {
        fn generation(version: &str) -> Option<u64> {
            version.split('-').next()?.parse().ok()
        }
        if self.version == installed {
            return VersionCheck::Current;
        }
        match (generation(&self.version), generation(installed)) {
            (Some(recorded), Some(installed)) if recorded > installed => VersionCheck::Newer,
            _ => VersionCheck::Upgrade,
        }
    }
}
/// Registers the import document kind and reflected settings in the kernel DB.
pub fn register_schema(db: &mut SchemaDb) {
    db.register::<crate::CookPreset>();
    db.register::<ImportSettings>();
    crate::project_settings::register(db);
    let _ = db.add_kind(DocKindDesc {
        name: "import".into(),
        version: 1,
        doc: "Binary source asset identity and pure importer settings.".into(),
        id_prefix: "as".into(),
        extension: "import.json".into(),
        records: vec!["ImportSettings".into()],
        single_root: false,
    });
}
/// A sidecar document. Editing uses SidecarCommand → kernel Edit → Op/transaction.
pub struct Sidecar {
    document: Document,
}
impl Sidecar {
    /// Creates an empty sidecar document; insert settings using `SidecarCommand`.
    pub fn empty(id: AssetId) -> Self {
        Self {
            document: Document::new("import", id),
        }
    }
    /// Parses/validates a sidecar through the kernel canonical reader.
    pub fn read(text: &str, path: &str, db: &SchemaDb) -> Result<Self> {
        let _trace = crate::trace::Scope::new("sidecar.read", path);
        let read = ara_doc::read(text, path, db, None);
        if ara_diag::has_errors(&read.diags) {
            return Err(Error::Invalid(
                read.diags
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }
        let document = read
            .doc
            .ok_or_else(|| Error::Invalid("sidecar has no document".into()))?;
        if document.kind != "import"
            || document.records().len() != 1
            || document.record("i_settings").is_none()
        {
            return Err(Error::Invalid(
                "expected one i_settings import record".into(),
            ));
        }
        let sidecar = Self { document };
        sidecar.settings()?;
        Ok(sidecar)
    }
    /// Read-only document for the kernel store, persistence and undo integration.
    pub fn document(&self) -> &Document {
        &self.document
    }
    /// Moves this document into an ara-ops Store/Workspace.
    pub fn into_document(self) -> Document {
        self.document
    }
    /// Serializes through the canonical document writer; does not write files.
    pub fn write(&self, db: &SchemaDb) -> String {
        ara_doc::write(&self.document, db)
    }
    /// Reads the current settings, filling reflected/serde defaults.
    pub fn settings(&self) -> Result<ImportSettings> {
        let record = self
            .document
            .record("i_settings")
            .ok_or_else(|| Error::Invalid("missing import settings".into()))?;
        let settings: ImportSettings =
            serde_json::from_value(serde_json::Value::Object(record.clone()))?;
        if settings.importer.is_empty() || settings.version.is_empty() {
            return Err(Error::Invalid("importer and version must be set".into()));
        }
        Ok(settings)
    }
    /// Explicit quality override; an absent field inherits the project policy.
    pub fn preset_override(&self) -> Result<Option<crate::CookPreset>> {
        self.document
            .record("i_settings")
            .and_then(|r| r.get("preset"))
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone()).map_err(Error::from))
            .transpose()
    }
    /// Indexes the source media path (without `.import.json`) for `@path#id` repair.
    pub fn index(&self, source_path: &str, index: &mut AssetIndex) {
        index.insert(self.document.id.to_string(), source_path, None);
    }
}
/// Typed authoring commands, translated into the existing kernel op vocabulary.
pub enum SidecarCommand {
    /// Initialize the settings record in an empty document.
    Initialize(ImportSettings),
    /// Initialize a source which inherits the project quality policy.
    InitializeInherited(ImportSettings),
    /// Replace only importer-specific settings; inverse restores prior settings.
    Settings(serde_json::Value),
    /// Change final shipping quality; this op is undoable like other settings.
    Preset(crate::CookPreset),
    /// Remove the override so subsequent imports follow the project policy.
    InheritPreset,
    /// Replace declared asset dependencies.
    Dependencies(Vec<AssetRef>),
    /// Record the importer version the asset is cooked with (after an importer upgrade).
    Version(String),
}
impl SidecarCommand {
    /// Produces a validated-op request for `Store::transact` or `Document::apply`.
    pub fn edit(self, doc: AssetId) -> Result<Edit> {
        let id = RecordId::parse("i_settings").map_err(Error::Invalid)?;
        let inherit = matches!(&self, Self::InitializeInherited(_));
        Ok(match self {
            Self::Initialize(settings) | Self::InitializeInherited(settings) => {
                if settings.importer.is_empty() || settings.version.is_empty() {
                    return Err(Error::Invalid("importer and version must be set".into()));
                }
                let mut record = ara_json::to_value(&settings)?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| Error::Invalid("settings must be an object".into()))?;
                if inherit {
                    record.remove("preset");
                }
                Edit::Create { doc, id, record }
            }
            Self::Settings(settings) => Edit::Set {
                doc,
                id,
                path: FieldPath::field("settings"),
                value: Some(settings),
            },
            Self::Preset(preset) => Edit::Set {
                doc,
                id,
                path: FieldPath::field("preset"),
                value: Some(ara_json::to_value(&preset)?),
            },
            Self::InheritPreset => Edit::Set {
                doc,
                id,
                path: FieldPath::field("preset"),
                value: None,
            },
            Self::Dependencies(deps) => Edit::Set {
                doc,
                id,
                path: FieldPath::field("dependencies"),
                value: Some(ara_json::to_value(&deps)?),
            },
            Self::Version(version) => {
                if version.is_empty() {
                    return Err(Error::Invalid("version must be set".into()));
                }
                Edit::Set {
                    doc,
                    id,
                    path: FieldPath::field("version"),
                    value: Some(serde_json::Value::String(version)),
                }
            }
        })
    }
}
