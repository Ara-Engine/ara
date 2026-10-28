//! Resource lifecycle and initial source share the batch's attributed transaction.
use super::*;

/// Create one resource with its initialized domain source in the same transaction.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorResourceCreateCommandParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Params {
    /// New authored-relative document path.
    path: String,
    /// scene, prefab, material, vfx, graph or animation.
    domain: String,
    /// Optional complete material or VFX source.
    source: Option<String>,
}

pub(super) struct Create;
impl Command for Create {
    type Params = Params;
    type Output = Value;
    const NAME: &'static str = "editor.resource.create.atomic";
    const SUMMARY: &'static str =
        "Create and initialize an authoring resource as one undoable transaction.";

    fn examples() -> Vec<Value> {
        vec![json!({"path":"materials/paint.graph","domain":"material"})]
    }

    fn run(cx: &mut EditCtx<'_, '_>, p: Params) -> Result<Value, Diagnostic> {
        let kind = match p.domain.as_str() {
            "scene" => "scene",
            "prefab" => "prefab",
            "animation" => "animation",
            "material" | "vfx" | "graph" => "graph",
            _ => return Err(crate::invalid("unknown resource domain")),
        };
        let doc = cx.create_document(&p.path, kind, None)?;
        if p.domain == "animation" {
            cx.in_document(&doc, |cx| {
                AnimationSet::run(
                    cx,
                    AnimationParams {
                        record: "an_controller".into(),
                        controller: ara_anim::graph::ControllerData::default(),
                    },
                )
            })?;
        } else if kind == "graph" {
            // Parsing and semantic validation occur after the staged lifecycle op;
            // any failure rolls back that op with every initializer record.
            cx.in_document(&doc, |cx| {
                Initialize::run(
                    cx,
                    InitializeParams {
                        domain: p.domain,
                        source: p.source,
                    },
                )
            })?;
        }
        Ok(json!({"doc":doc,"path":p.path}))
    }
}
