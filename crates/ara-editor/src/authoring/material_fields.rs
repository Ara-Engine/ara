//! Typed parameter/literal edits shared by the node inspector and protocol.
use ara_cmd::{Command, EditCtx};
use ara_diag::Diagnostic;
use ara_schema::Schema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorMaterialValueParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Set a material parameter default or literal while preserving its declared type.
pub(super) struct Params {
    /// Stable graph node ID.
    node: String,
    /// Numeric lanes, boolean, or symbolic texture reference/default.
    value: Value,
}
pub(super) struct Set;
impl Command for Set {
    type Params = Params;
    type Output = Value;
    const NAME: &'static str = "editor.material.value.set";
    const SUMMARY: &'static str = "Edit a typed material node value and its synchronized DSL.";
    fn examples() -> Vec<Value> {
        vec![json!({"node":"n_roughness","value":[0.4]})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Params) -> Result<Value, Diagnostic> {
        let graph = ara_graph::Graph::from_document(cx.read().doc())?;
        let mut material =
            ara_material::Material::from_graph(&graph).map_err(super::commands::first)?;
        if let Some(parameter) = material.parameters.get_mut(&p.node) {
            parameter.default =
                serde_json::from_value(p.value).map_err(|e| crate::invalid(e.to_string()))?;
        } else if let Some(binding) = material.bindings.get_mut(&p.node) {
            if !matches!(binding.expr, ara_material::Expr::Literal { .. }) {
                return Err(crate::invalid(
                    "Operation values are edited through their input nodes",
                ));
            }
            let values =
                serde_json::from_value(p.value).map_err(|e| crate::invalid(e.to_string()))?;
            binding.expr = ara_material::Expr::Literal { values };
        } else {
            return Err(crate::invalid("Material value node not found"));
        }
        ara_material::check(&material).map_err(super::commands::first)?;
        ara_graph::commands::apply_snapshot(cx, &material.to_graph(graph.editor)?)?;
        Ok(Value::Null)
    }
}
