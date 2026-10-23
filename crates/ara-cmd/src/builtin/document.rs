//! Workspace authoring commands. The host persists these through the ordinary op stream.
use crate::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_schema::{AssetRef, DocKindDesc, FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

/// Project marker represented as a record so project changes share history and undo.
#[derive(Schema)]
#[ara(record(prefix = "p"), module = "core")]
pub struct ProjectSettings {
    /// Project settings, including fields owned by plugins and future engine versions.
    pub settings: Value,
}

/// Parameters of `doc.create`.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocCreateParams {
    /// Project-relative output path, with the document kind's extension.
    pub path: String,
    /// Registered document kind; inferred from the path when omitted.
    pub kind: Option<String>,
    /// Stable asset ID or batch symbol; generated when omitted.
    pub id: Option<String>,
}

/// Create an empty canonical document with an attributed, reversible lifecycle op.
pub struct DocCreate;
impl Command for DocCreate {
    type Params = DocCreateParams;
    type Output = Value;
    const NAME: &'static str = "doc.create";
    const SUMMARY: &'static str =
        "Create a canonical empty scene, prefab or other registered document.";
    fn examples() -> Vec<Value> {
        vec![json!({"path":"levels/arena.scene","id":"sc_arena001"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        let kind = p
            .kind
            .or_else(|| {
                ctx.read()
                    .schema()
                    .kinds()
                    .find(|k| p.path.ends_with(&format!(".{}", k.extension)))
                    .map(|k| k.name.clone())
            })
            .ok_or_else(|| {
                Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "cannot infer document kind; supply kind",
                )
            })?;
        if kind == "project" {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "use ara init to create a project",
            ));
        }
        let id = ctx.create_document(&p.path, &kind, p.id.as_deref())?;
        Ok(json!({"id":id,"path":p.path,"kind":kind}))
    }
}

/// Parameters of `project.setMain`.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSetMainParams {
    /// Existing scene path or asset reference to open when the project starts.
    #[ara(asset_kind = "scene")]
    pub scene: AssetRef,
}

/// Change the startup scene through the same command and transaction path as scene edits.
pub struct ProjectSetMain;
impl Command for ProjectSetMain {
    type Params = ProjectSetMainParams;
    type Output = Value;
    const NAME: &'static str = "project.setMain";
    const SUMMARY: &'static str = "Set ara.project.json's startup scene with history and undo.";
    fn examples() -> Vec<Value> {
        vec![json!({"scene":"@levels/arena.scene#sc_arena001"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        if ctx.read().doc().kind != "project" {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "project.setMain targets ara.project.json",
            ));
        }
        let ws = ctx.read().workspace();
        let scene = ws
            .find(&p.scene.0)
            .and_then(|id| ws.doc(id.as_str()))
            .filter(|d| d.kind == "scene")
            .ok_or_else(|| {
                Diagnostic::error(codes::DOC_NOT_FOUND, "main must reference a loaded scene")
            })?;
        let path = ws
            .path_of(scene.id.as_str())
            .expect("loaded document has a path")
            .to_owned();
        let id = ara_doc::RecordId::parse("p_settings").expect("valid project settings ID");
        ctx.set(
            &id,
            FieldPath::field("settings").key("main"),
            Some(path.clone().into()),
        )?;
        Ok(json!({"main":path}))
    }
}

/// Register project metadata and workspace commands in the shared registry.
pub(super) fn register(registry: &mut Registry, db: &mut SchemaDb) {
    db.register::<ProjectSettings>();
    let _ = db.add_kind(DocKindDesc {
        name: "project".into(),
        version: 1,
        doc: "Project startup and plugin settings; persisted as ara.project.json.".into(),
        id_prefix: "pj".into(),
        extension: "project.json".into(),
        records: vec!["ProjectSettings".into()],
        single_root: false,
    });
    registry
        .command::<DocCreate>(db)
        .command::<ProjectSetMain>(db);
}
