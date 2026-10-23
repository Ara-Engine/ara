//! Typed resource creation and shared command registration for authoring panels.
mod commands;
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
    /// material, vfx, graph or animation.
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
pub(crate) struct AuthoringService;
impl Service for AuthoringService {
    fn name(&self) -> &'static str {
        "editor-authoring"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![Method::new::<CreateParams>(db,"editor.resource.create","Create a typed material, effect, graph or animation asset, with undoable initialization.").action("ara_session","editor.resource.create")]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        _: &str,
        params: Value,
    ) -> Result<Value, Vec<ara_diag::Diagnostic>> {
        let p: CreateParams = parse(params)?;
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
