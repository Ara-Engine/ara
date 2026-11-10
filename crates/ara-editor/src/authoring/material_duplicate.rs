//! Duplicating material nodes (the graph's ⌘D): each copy takes a free name and an offset
//! position, gets its own copies of the input defaults drawn inline on it, and keeps
//! sharing every other input, as Unreal's duplicate keeps a node's settings.
use ara_cmd::{Command, EditCtx};
use ara_diag::Diagnostic;
use ara_material::{Expr, Material};
use ara_schema::Schema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorMaterialDuplicateParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Duplicate material nodes with their inline input defaults.
pub(super) struct Params {
    /// Stable IDs of the nodes to copy; the output node and unknown IDs are skipped.
    nodes: Vec<String>,
    /// Graph-space offset of each copy from its original (32, 32 by default).
    #[serde(default)]
    offset: Option<[f32; 2]>,
}

pub(super) struct Duplicate;
impl Command for Duplicate {
    type Params = Params;
    type Output = Value;
    const NAME: &'static str = "editor.material.node.duplicate";
    const SUMMARY: &'static str =
        "Duplicate material nodes, each with its own copies of its inline input defaults.";
    fn examples() -> Vec<Value> {
        vec![json!({"nodes":["n_tint"]})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: Params) -> Result<Value, Diagnostic> {
        let graph = ara_graph::Graph::from_document(cx.read().doc())?;
        let mut material = Material::from_graph(&graph).map_err(super::commands::first)?;
        let mut layout = graph.editor.clone();
        let mut order = material
            .bindings
            .values()
            .map(|b| b.order)
            .chain(material.parameters.values().map(|p| p.order))
            .max()
            .unwrap_or(0);
        let offset = p.offset.unwrap_or([32.0, 32.0]);
        let mut copies = Vec::new();
        for source in &p.nodes {
            let id = cx.new_id(None, "n")?.to_string();
            let at = layout
                .positions
                .get(source)
                .map(|[x, y]| [x + offset[0], y + offset[1]]);
            if let Some(mut parameter) = material.parameters.get(source).cloned() {
                parameter.name = free_name(&material, &parameter.name);
                order += 1;
                parameter.order = order;
                material.parameters.insert(id.clone(), parameter);
            } else if let Some(mut binding) = material.bindings.get(source).cloned() {
                binding.name = free_name(&material, &binding.name);
                order += 1;
                binding.order = order;
                if let Expr::Call { args, .. } = &mut binding.expr {
                    for (index, arg) in args.iter_mut().enumerate() {
                        // Its own copy of each default drawn on it; shared inputs stay shared.
                        let Some(default) = material
                            .bindings
                            .get(arg.as_str())
                            .filter(|_| crate::graph::is_default(&graph, arg))
                            .cloned()
                        else {
                            continue;
                        };
                        let copy = cx.new_id(None, "n")?.to_string();
                        let position = layout
                            .positions
                            .get(arg.as_str())
                            .map(|[x, y]| [x + offset[0], y + offset[1]]);
                        if let Some(position) = position {
                            layout.positions.insert(copy.clone(), position);
                        }
                        material.bindings.insert(
                            copy.clone(),
                            ara_material::Binding {
                                name: format!("{}Input{index}", binding.name),
                                ..default
                            },
                        );
                        *arg = copy;
                    }
                }
                material.bindings.insert(id.clone(), binding);
            } else {
                continue;
            }
            if let Some(at) = at {
                layout.positions.insert(id.clone(), at);
            }
            copies.push(id);
        }
        let graph = material.to_graph(layout)?;
        ara_graph::commands::apply_snapshot(cx, &graph)?;
        Ok(json!({"ids": copies}))
    }
}

/// The first free `name_copy`, `name_copy2`, … for a copy of `name`.
fn free_name(material: &Material, name: &str) -> String {
    let base = format!("{name}_copy");
    std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base}{n}")))
        .find(|candidate| material.node_id(candidate).is_none())
        .unwrap_or(base)
}
