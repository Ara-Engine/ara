//! Typed resource creation and shared command registration for authoring panels.
pub(crate) mod animation;
mod commands;
mod material_fields;
use ara_host::{Cx, Method, Service, parse};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
/// Create a material, effect, generic graph or animation controller asset.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorResourceCreateParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateParams {
    /// Calling editor or agent session.
    session: String,
    /// New project-relative asset document path.
    path: String,
    /// material, vfx, graph, animation or scene (an empty scene document).
    domain: String,
    /// Optional complete domain source for material/VFX import.
    source: Option<String>,
    /// Reason displayed in the agent review history.
    why: Option<String>,
}
/// Register domain commands before the host loads its workspace.
pub(crate) fn register(registry: &mut ara_cmd::Registry, db: &mut SchemaDb) {
    commands::register(registry, db);
}
/// Nonsemantic controller canvas data, kept in its own shared graph document.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorAnimationLayoutParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayoutParams {
    /// Calling editor or agent session.
    session: String,
    /// Animation controller document identity.
    doc: String,
    /// Comments, groups, reroutes and positions; excluded from animation execution.
    editor: ara_graph::Editor,
    /// Reason shown in review.
    why: Option<String>,
}
pub(crate) struct AuthoringService;
impl Service for AuthoringService {
    fn name(&self) -> &'static str {
        "editor-authoring"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![Method::new::<LayoutParams>(db,"editor.animation.layout","Edit controller canvas layout through shared graph ops.").action("ara_session","editor.animation.layout"),Method::new::<CreateParams>(db,"editor.resource.create","Create a typed material, effect, graph or animation asset, with undoable initialization.").action("ara_session","editor.resource.create")]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<ara_diag::Diagnostic>> {
        if method == "editor.animation.layout" {
            let p: LayoutParams = parse(params)?;
            let id = cx
                .store()
                .workspace()
                .find(&p.doc)
                .ok_or_else(|| vec![crate::invalid("animation not found")])?;
            if cx
                .store()
                .workspace()
                .doc(id.as_str())
                .is_none_or(|d| d.kind != "animation")
            {
                return Err(vec![crate::invalid("expected animation document")]);
            }
            let path = animation::layout_path(id.as_str());
            let existing = cx.store().workspace().find(&path).map(ToString::to_string);
            let mut commands = Vec::new();
            let doc = if let Some(doc) = existing {
                doc
            } else {
                let doc = cx.create_document(&path, "graph").map_err(|d| vec![d])?;
                commands.push(
                    json!({"cmd":"editor.graph.initialize","args":{"domain":"animation-layout"}}),
                );
                doc
            };
            if cx
                .store()
                .workspace()
                .doc(&doc)
                .is_some_and(|d| d.records().is_empty())
                && commands.is_empty()
            {
                commands.push(
                    json!({"cmd":"editor.graph.initialize","args":{"domain":"animation-layout"}}),
                );
            }
            commands.push(json!({"cmd":"graph.editor.set","args":{"editor":p.editor}}));
            return cx.apply(parse(json!({"session":p.session,"doc":doc,"commands":commands,"why":p.why.unwrap_or_else(||"Edit animation canvas".into())}))?);
        }
        let p: CreateParams = parse(params)?;
        if p.domain == "scene" {
            // An empty scene document; File › New Scene opens it.
            let doc = cx.create_document(&p.path, "scene").map_err(|d| vec![d])?;
            return Ok(json!({"doc":doc,"path":p.path}));
        }
        if !matches!(
            p.domain.as_str(),
            "material" | "vfx" | "graph" | "animation"
        ) {
            return Err(vec![crate::invalid("unknown resource domain")]);
        }
        if let Some(source) = &p.source {
            match p.domain.as_str() {
                "material" => {
                    ara_material::dsl::parse(source, "gr_validate")?;
                }
                "vfx" => {
                    ara_vfx::Effect::parse(source).map_err(|d| vec![d])?;
                }
                _ => {}
            }
        }
        let doc = cx
            .create_document(
                &p.path,
                if p.domain == "animation" {
                    "animation"
                } else {
                    "graph"
                },
            )
            .map_err(|d| vec![d])?;
        let command = if p.domain == "animation" {
            json!({"cmd":"editor.animation.controller.set","args":{"record":"an_controller","controller":{}}})
        } else {
            json!({"cmd":"editor.graph.initialize","args":{"domain":p.domain,"source":p.source}})
        };
        let batch = parse(
            json!({"session":p.session,"doc":doc,"why":p.why.unwrap_or_else(||format!("Create {}",p.domain)),"commands":[command]}),
        )?;
        let transaction = cx.apply(batch)?;
        Ok(json!({"doc":doc,"path":p.path,"transaction":transaction}))
    }
}
