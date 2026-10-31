//! Import sidecar commands. Selection and versioning come from the installed registry.
use ara_asset::{
    CookPreset, ImportSettings, PROJECT_IMPORT_SETTINGS_FILE, ProjectImportSettings, SidecarCommand,
};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_schema::{AssetRef, Schema, SchemaDb, Validator};
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(test)]
mod project_defaults_tests;

fn invalid(message: impl ToString) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message.to_string())
}

/// Select the installed importer from a source extension; unknown formats diagnose.
pub fn importer_for_source(source: &str) -> Result<&'static str, Diagnostic> {
    let extension = std::path::Path::new(source)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "gltf" | "glb" => Ok("ara.gltf"),
        "fbx" => Ok("ara.fbx"),
        "png" | "jpg" | "jpeg" | "hdr" | "exr" | "tga" | "tif" | "tiff" | "webp" | "bmp"
        | "dds" => Ok("ara.texture"),
        "wav" | "flac" | "ogg" | "vorbis" | "opus" => Ok("ara.audio"),
        _ => Err(invalid(format!("no installed importer for .{extension}"))),
    }
}

fn validate_settings(importer: &str, settings: &Value, db: &SchemaDb) -> Result<(), Diagnostic> {
    let ty = match importer {
        "ara.gltf" | "ara.fbx" => "MeshSettings",
        "ara.texture" => "TextureSettings",
        "ara.audio" => "AudioImportSettings",
        _ => return Err(invalid("importer has no shared authoring settings")),
    };
    let settings = settings
        .as_object()
        .ok_or_else(|| invalid("settings must be an object"))?;
    let mut diagnostics = Vec::new();
    let desc = db
        .get(ty)
        .ok_or_else(|| invalid(format!("missing installed settings schema {ty}")))?;
    Validator::new(db, &mut diagnostics).fields(desc, settings, "params.settings");
    if let Some(diagnostic) = diagnostics.into_iter().find(Diagnostic::is_error) {
        return Err(diagnostic);
    }
    if importer == "ara.audio" {
        crate::audio::settings(&Value::Object(settings.clone())).map_err(invalid)?;
    }
    if importer == "ara.fbx"
        && settings
            .get("opacityMaps")
            .and_then(Value::as_object)
            .is_some_and(|maps| !maps.is_empty())
    {
        return Err(invalid(
            "opacityMaps currently names glTF images; FBX does not support this setting",
        ));
    }
    Ok(())
}

/// Parameters of `import.initialize`.
#[derive(Schema, Deserialize)]
#[ara(params, module = "asset")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportInitializeParams {
    /// Existing source media path, relative to the authored root.
    pub source: String,
    /// Stable asset ID or batch symbol; generated when omitted.
    pub id: Option<String>,
    /// Importer-specific settings; defaults when omitted.
    pub settings: Option<Value>,
    /// Source texture quality override; omitted inherits the project preset.
    pub preset: Option<CookPreset>,
    /// Referenced authoring assets used by the cook.
    pub dependencies: Option<Vec<AssetRef>>,
}
struct Initialize;
impl Command for Initialize {
    type Params = ImportInitializeParams;
    type Output = Value;
    const NAME: &'static str = "import.initialize";
    const SUMMARY: &'static str =
        "Create an import sidecar with automatic importer selection and installed version.";
    fn examples() -> Vec<Value> {
        vec![json!({"source":"models/crate.glb","id":"as_crate"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        let importer = importer_for_source(&p.source)?;
        let registry = crate::registry().map_err(invalid)?;
        let version = registry
            .get(importer)
            .map_err(invalid)?
            .version()
            .to_owned();
        let settings = p.settings.unwrap_or_else(|| json!({}));
        validate_settings(importer, &settings, cx.read().schema())?;
        let path = format!("{}.import.json", p.source);
        let id = cx.create_document(&path, "import", p.id.as_deref())?;
        let inherit = p.preset.is_none();
        let settings = ImportSettings {
            importer: importer.into(),
            version,
            settings,
            preset: p.preset,
            dependencies: p.dependencies.unwrap_or_default(),
        };
        cx.apply_edit(
            (if inherit {
                SidecarCommand::InitializeInherited(settings.clone())
            } else {
                SidecarCommand::Initialize(settings.clone())
            })
            .edit(id.clone())
            .map_err(invalid)?,
        )?;
        Ok(json!({"id":id,"path":path,"importer":settings.importer,"version":settings.version}))
    }
}

/// Parameters of `import.configure`.
#[derive(Schema, Deserialize)]
#[ara(params, module = "asset")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportConfigureParams {
    /// Source media path whose existing sidecar should change.
    pub source: String,
    /// Replacement importer-specific settings; omitted retains them.
    pub settings: Option<Value>,
    /// Replacement shipping preset; omitted retains it.
    pub preset: Option<CookPreset>,
    /// Remove the source quality override and follow the project import policy.
    #[serde(default)]
    #[ara(default = false)]
    pub use_project_preset: bool,
    /// Replacement authoring dependencies; omitted retains them.
    pub dependencies: Option<Vec<AssetRef>>,
}
struct Configure;
impl Command for Configure {
    type Params = ImportConfigureParams;
    type Output = Value;
    const NAME: &'static str = "import.configure";
    const SUMMARY: &'static str =
        "Configure a source's import settings and refresh its installed importer version.";
    fn examples() -> Vec<Value> {
        vec![json!({"source":"models/crate.glb","settings":{"lodCount":3}})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        if p.use_project_preset && p.preset.is_some() {
            return Err(invalid("choose either preset or useProjectPreset"));
        }
        let path = cx
            .read()
            .workspace()
            .storage_path(&format!("{}.import.json", p.source));
        let doc = cx
            .read()
            .workspace()
            .docs()
            .find(|(name, _)| *name == path)
            .map(|(_, d)| d)
            .ok_or_else(|| invalid("source has no sidecar; use import.initialize"))?;
        let id = doc.id.clone();
        let existing: ImportSettings = serde_json::from_value(Value::Object(
            doc.record("i_settings")
                .ok_or_else(|| invalid("sidecar has no settings"))?
                .clone(),
        ))
        .map_err(invalid)?;
        let expected = importer_for_source(&p.source)?;
        if existing.importer != expected {
            return Err(invalid("sidecar importer does not match its source format"));
        }
        let registry = crate::registry().map_err(invalid)?;
        let version = registry
            .get(expected)
            .map_err(invalid)?
            .version()
            .to_owned();
        if let Some(settings) = p.settings {
            validate_settings(expected, &settings, cx.read().schema())?;
            cx.apply_edit(
                SidecarCommand::Settings(settings)
                    .edit(id.clone())
                    .map_err(invalid)?,
            )?;
        }
        if let Some(preset) = p.preset {
            cx.apply_edit(
                SidecarCommand::Preset(preset)
                    .edit(id.clone())
                    .map_err(invalid)?,
            )?;
        }
        if p.use_project_preset {
            cx.apply_edit(
                SidecarCommand::InheritPreset
                    .edit(id.clone())
                    .map_err(invalid)?,
            )?;
        }
        if let Some(dependencies) = p.dependencies {
            cx.apply_edit(
                SidecarCommand::Dependencies(dependencies)
                    .edit(id.clone())
                    .map_err(invalid)?,
            )?;
        }
        if existing.version != version {
            cx.apply_edit(
                SidecarCommand::Version(version.clone())
                    .edit(id.clone())
                    .map_err(invalid)?,
            )?;
        }
        Ok(json!({"id":id,"importer":expected,"version":version}))
    }
}

/// Register shared sidecar schemas and authoring commands; no source content is imported here.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) -> Result<(), Diagnostic> {
    ara_asset::register_schema(db);
    crate::registry().map_err(invalid)?.register_schemas(db);
    registry
        .command::<Initialize>(db)
        .command::<Configure>(db)
        .command::<ConfigureProject>(db);
    Ok(())
}

/// Parameters of `import.project.configure`; reflected in the inspector and MCP.
#[derive(Schema, Deserialize)]
#[ara(params, module = "asset")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportProjectConfigureParams {
    /// Default texture quality for sources and model textures without an override.
    pub preset: Option<CookPreset>,
    /// Texture defaults, using the exact installed texture settings schema.
    pub texture: Option<crate::TextureSettings>,
    /// glTF model defaults, overridden independently by explicit source keys.
    pub gltf: Option<crate::MeshSettings>,
    /// FBX model defaults; glTF-only opacity maps are rejected.
    pub fbx: Option<crate::MeshSettings>,
    /// Audio defaults, using the exact installed audio settings schema.
    pub audio: Option<crate::audio::Settings>,
    /// Remove one importer policy and return that type to built-in defaults.
    pub reset_type: Option<ImportDefaultsType>,
}
/// Built-in importer policies which can return to their engine defaults.
#[derive(Debug, Clone, Copy, Schema, Deserialize)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase")]
pub enum ImportDefaultsType {
    /// PNG/JPEG/HDR/EXR textures.
    Texture,
    /// glTF models and their embedded texture policy.
    Gltf,
    /// FBX models and their embedded texture policy.
    Fbx,
    /// Resident and streamed audio.
    Audio,
}
struct ConfigureProject;
impl Command for ConfigureProject {
    type Params = ImportProjectConfigureParams;
    type Output = Value;
    const NAME: &'static str = "import.project.configure";
    const SUMMARY: &'static str =
        "Configure global texture quality and typed per-importer project defaults.";
    fn examples() -> Vec<Value> {
        vec![json!({"preset":"balanced"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        if p.preset.is_none()
            && p.texture.is_none()
            && p.gltf.is_none()
            && p.fbx.is_none()
            && p.audio.is_none()
            && p.reset_type.is_none()
        {
            return Err(invalid(
                "Choose a project preset, typed importer defaults, or resetType",
            ));
        }
        let existing = cx
            .read()
            .workspace()
            .docs()
            .find(|(path, _)| {
                *path == PROJECT_IMPORT_SETTINGS_FILE
                    || *path == ara_asset::LEGACY_PROJECT_IMPORT_SETTINGS_FILE
            })
            .map(|(path, doc)| (path.to_owned(), doc.id.clone()));
        let mut policy = match &existing {
            Some((_, id)) => serde_json::from_value::<ProjectImportSettings>(Value::Object(
                cx.read()
                    .workspace()
                    .doc(id.as_str())
                    .and_then(|doc| doc.record("pi_settings"))
                    .ok_or_else(|| invalid("project import policy has no settings record"))?
                    .clone(),
            ))
            .map_err(invalid)?,
            None => ProjectImportSettings::default(),
        };
        let defaults_changed = p.texture.is_some()
            || p.gltf.is_some()
            || p.fbx.is_some()
            || p.audio.is_some()
            || p.reset_type.is_some();
        let conflict = match p.reset_type {
            Some(ImportDefaultsType::Texture) => p.texture.is_some(),
            Some(ImportDefaultsType::Gltf) => p.gltf.is_some(),
            Some(ImportDefaultsType::Fbx) => p.fbx.is_some(),
            Some(ImportDefaultsType::Audio) => p.audio.is_some(),
            None => false,
        };
        if conflict {
            return Err(invalid(
                "Choose either typed defaults or resetType for the same importer",
            ));
        }
        for (importer, value) in [
            ("ara.texture", p.texture.as_ref().map(serde_json::to_value)),
            ("ara.gltf", p.gltf.as_ref().map(serde_json::to_value)),
            ("ara.fbx", p.fbx.as_ref().map(serde_json::to_value)),
            ("ara.audio", p.audio.as_ref().map(serde_json::to_value)),
        ] {
            if let Some(value) = value {
                validate_settings(importer, &value.map_err(invalid)?, cx.read().schema())?;
            }
        }
        if let Some(preset) = p.preset {
            policy.preset = preset;
        }
        if let Some(value) = p.texture {
            policy.defaults.texture = Some(value);
        }
        if let Some(value) = p.gltf {
            policy.defaults.gltf = Some(value);
        }
        if let Some(value) = p.fbx {
            policy.defaults.fbx = Some(value);
        }
        if let Some(value) = p.audio {
            policy.defaults.audio = Some(value);
        }
        match p.reset_type {
            Some(ImportDefaultsType::Texture) => policy.defaults.texture = None,
            Some(ImportDefaultsType::Gltf) => policy.defaults.gltf = None,
            Some(ImportDefaultsType::Fbx) => policy.defaults.fbx = None,
            Some(ImportDefaultsType::Audio) => policy.defaults.audio = None,
            None => {}
        }
        let record = ara_doc::RecordId::parse("pi_settings").map_err(invalid)?;
        let (path, id) = if let Some((path, id)) = existing {
            if p.preset.is_some() {
                cx.apply_edit(ara_doc::Edit::Set {
                    doc: id.clone(),
                    id: record.clone(),
                    path: ara_schema::FieldPath::field("preset"),
                    value: Some(json!(policy.preset)),
                })?;
            }
            if defaults_changed {
                cx.apply_edit(ara_doc::Edit::Set {
                    doc: id.clone(),
                    id: record,
                    path: ara_schema::FieldPath::field("defaults"),
                    value: Some(json!(policy.defaults)),
                })?;
            }
            (path, id)
        } else {
            let id =
                cx.create_document(PROJECT_IMPORT_SETTINGS_FILE, "imports", Some("ip_project"))?;
            cx.apply_edit(ara_doc::Edit::Create {
                doc: id.clone(),
                id: record,
                record: serde_json::to_value(&policy)
                    .map_err(invalid)?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| invalid("project import settings must be an object"))?,
            })?;
            (PROJECT_IMPORT_SETTINGS_FILE.to_owned(), id)
        };
        Ok(json!({"id":id,"path":path,"preset":policy.preset,"defaults":policy.defaults}))
    }
}
