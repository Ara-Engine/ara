//! Import sidecar commands. Selection and versioning come from the installed registry.
use ara_asset::{CookPreset, ImportSettings, SidecarCommand};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_schema::{AssetRef, Schema, SchemaDb, Validator};
use serde::Deserialize;
use serde_json::{Value, json};

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
        "png" | "jpg" | "jpeg" | "hdr" | "exr" => Ok("ara.texture"),
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
    Ok(())
}

/// Parameters of `import.initialize`.
#[derive(Schema, Deserialize)]
#[ara(params, module = "asset")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportInitializeParams {
    /// Existing source media path, relative to the project.
    pub source: String,
    /// Stable asset ID or batch symbol; generated when omitted.
    pub id: Option<String>,
    /// Importer-specific settings; defaults when omitted.
    pub settings: Option<Value>,
    /// Shipping cook quality; defaults to the engine's shipping preset.
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
        let settings = ImportSettings {
            importer: importer.into(),
            version,
            settings,
            preset: p.preset.unwrap_or_default(),
            dependencies: p.dependencies.unwrap_or_default(),
        };
        cx.apply_edit(
            SidecarCommand::Initialize(settings.clone())
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
        let path = format!("{}.import.json", p.source);
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
    registry.command::<Initialize>(db).command::<Configure>(db);
    Ok(())
}
