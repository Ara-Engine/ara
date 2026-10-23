//! Import configuration stays in the same attributed command history as scene edits.
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorImportSettingsParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Typed importer authoring request.
struct Params {
    /// Complete importer settings, validated by the importer-specific reflected type.
    settings: Value,
    /// Desired final cook quality.
    preset: ara_asset::CookPreset,
}
struct Set;
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorImportInitializeParams")]
#[serde(deny_unknown_fields)]
/// Typed importer authoring request.
struct InitializeParams {
    /// Reflected importer identity and initial settings.
    settings: ara_asset::ImportSettings,
}
struct Initialize;
impl Command for Initialize {
    type Params = InitializeParams;
    type Output = Value;
    const NAME: &'static str = "editor.import.initialize";
    const SUMMARY: &'static str =
        "Initialize an empty import document with attributed settings ops.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"settings":{"importer":"ara.texture","version":"3-image025-ispc050-tiled-f32text","settings":{}}}),
        ]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: InitializeParams) -> Result<Value, Diagnostic> {
        if cx.read().doc().kind != "import" {
            return Err(crate::invalid("expected import document"));
        }
        let id = ara_doc::RecordId::parse("i_settings").map_err(crate::invalid)?;
        let value = ara_json::to_value(&p.settings).map_err(|e| crate::invalid(e.to_string()))?;
        cx.create(
            id,
            value
                .as_object()
                .cloned()
                .ok_or_else(|| crate::invalid("settings must be an object"))?,
        )?;
        Ok(Value::Null)
    }
}
impl Command for Set {
    type Params = Params;
    type Output = Value;
    const NAME: &'static str = "editor.import.settings";
    const SUMMARY: &'static str = "Set import settings and quality as one undoable transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"settings":{"usage":"color","mipmaps":true},"preset":"fast"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Params) -> Result<Value, Diagnostic> {
        let record = cx
            .read()
            .doc()
            .record("i_settings")
            .ok_or_else(|| crate::invalid("not an import settings document"))?;
        let importer = record.get("importer").and_then(Value::as_str).unwrap_or("");
        if importer == "ara.texture" {
            serde_json::from_value::<ara_importers::TextureSettings>(p.settings.clone())
                .map_err(|e| crate::invalid(e.to_string()))?;
        } else {
            serde_json::from_value::<ara_importers::MeshSettings>(p.settings.clone())
                .map_err(|e| crate::invalid(e.to_string()))?;
        }
        let id = ara_doc::RecordId::parse("i_settings").map_err(crate::invalid)?;
        cx.set(&id, FieldPath::field("settings"), Some(p.settings))?;
        cx.set(
            &id,
            FieldPath::field("preset"),
            Some(ara_json::to_value(&p.preset).map_err(|e| crate::invalid(e.to_string()))?),
        )?;
        Ok(Value::Null)
    }
}
pub(super) fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry.command::<Set>(db).command::<Initialize>(db);
}
