//! The selected material node in the Inspector: a header with its operation and doc
//! line, its editable value, then its inputs (with what feeds them) and its output.
//! Value edits share the graph/DSL transaction path (`editor.material.value.set`).
use super::*;
use crate::graph::style;
impl Shell {
    pub(super) fn material_inspector(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        out.push_str("<scroll id='material-inspector' style='flex-grow:1;flex-basis:0px;min-height:0px;padding-bottom:12px'>");
        if self.annotation_inspector(editor, out) {
            out.push_str("</scroll>");
            return Ok(());
        }
        let Some(doc) = self.resource_for(editor, "material") else {
            out.push_str("</scroll>");
            return Ok(());
        };
        let graph = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(&doc)
                .ok_or("material missing")?,
        )
        .map_err(|d| d.to_string())?;
        let material = match ara_material::Material::from_graph(&graph) {
            Ok(material) => Some(material),
            Err(diagnostics) => {
                for (n, diagnostic) in diagnostics.iter().take(3).enumerate() {
                    out.push_str(&format!("<panel id='material-node-error-row-{n}' style='flex-shrink:0;padding:6px;padding-left:12px'><text id='material-node-error-{n}' class='destructive' style='white-space:normal' text='{}' /></panel>",escape(&diagnostic.message)));
                }
                None
            }
        };
        // The material page's own selection, also while another graph page is active.
        let selection = self.graph_selection(editor, "material").clone();
        if selection.is_empty() {
            self.empty(
                out,
                "material-node-empty",
                "contrast",
                "No node selected",
                "Select a node to edit its value and see what feeds its inputs.",
                &[],
            );
        }
        let width = self
            .ui
            .find("material-inspector")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(300.);
        let label = self.label_px(width);
        for (n, id) in selection.iter().enumerate() {
            let Some(node) = graph.nodes.get(id) else {
                continue;
            };
            let op = node.kind.strip_prefix("material.").unwrap_or(&node.kind);
            let ty = node.payload.get("ty").and_then(Value::as_str).unwrap_or("");
            let name = node
                .payload
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| style::op_title(op));
            let kind = match op {
                "parameter" => format!("Parameter · {}", style::short_type(ty)),
                "literal" => format!("Constant · {}", style::short_type(ty)),
                "output" => "Material output".into(),
                _ => style::op_title(op),
            };
            let doc_line = match op {
                "output" => "The material's typed outputs for its domain.",
                "parameter" => "A value material instances and scripts can change.",
                "literal" => "A typed constant, folded into the shader.",
                _ => ara_material::library::get(op).map_or("", |b| b.doc),
            };
            out.push_str(&format!("<panel id='node-header-{n}' class='component-header'>{}<text id='node-title-{n}' class='component-title' text='{}' /><panel id='node-header-fill-{n}' class='fill' /><text id='node-kind-{n}' class='caption' style='padding-right:6px' text='{}' /></panel>",icons::icon(&format!("node-header-{n}"),"contrast"),escape(&name),escape(&kind)));
            if !doc_line.is_empty() {
                out.push_str(&format!("<panel id='node-doc-row-{n}' style='flex-shrink:0;padding:6px;padding-left:12px'><text id='node-doc-{n}' class='dialog-text' text='{}' /></panel>",escape(doc_line)));
            }
            let value = if let Some(p) = material.as_ref().and_then(|m| m.parameters.get(id)) {
                Some(ara_json::to_value(&p.default)?)
            } else if let Some(b) = material.as_ref().and_then(|m| m.bindings.get(id)) {
                if let ara_material::Expr::Literal { values } = &b.expr {
                    Some(json!(values))
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(value) = value {
                let key = format!("material-value-{n}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&key),
                    Binding::MaterialValue(doc.clone(), id.clone()),
                );
                let draft = self
                    .drafts
                    .get(&format!("@material|{doc}|{id}"))
                    .cloned()
                    .unwrap_or_else(|| {
                        panels::display(
                            value
                                .as_array()
                                .filter(|v| v.len() == 1)
                                .map_or(&value, |v| &v[0]),
                        )
                    });
                let title = if op == "parameter" {
                    "Default"
                } else {
                    "Value"
                };
                out.push_str(&format!("<panel id='{key}-row' class='prop-row'><panel id='{key}-label-cell' class='prop-label' style='width:{label}px'><text id='{key}-label' text='{title}' /></panel><panel id='{key}-cell' class='prop-value'><field id='{key}' class='number' style='flex-grow:1;flex-basis:0px' text='{}' action='Dispatch' label='Typed numeric lanes, boolean or texture value. Enter applies.' /></panel></panel>",escape(&draft)));
            }
            let inputs = node
                .ports
                .iter()
                .filter(|(_, p)| p.direction == ara_graph::Direction::Input)
                .collect::<Vec<_>>();
            if !inputs.is_empty() {
                out.push_str(&format!("<panel id='node-inputs-{n}' class='side-header'><text id='node-inputs-title-{n}' class='caption' text='Inputs' /></panel>"));
            }
            for (i, (port, socket)) in inputs.into_iter().enumerate() {
                let source = graph
                    .edges
                    .values()
                    .find(|e| e.to.node == *id && e.to.port == *port)
                    .and_then(|e| graph.nodes.get(&e.from.node))
                    .map(|from| {
                        from.payload
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| {
                                style::op_title(from.kind.trim_start_matches("material."))
                            })
                    });
                let row = format!("node-input-{n}-{i}");
                let pin = style::pin_label(node, port, socket.direction, &socket.ty);
                out.push_str(&format!("<panel id='{row}' class='prop-row'><panel id='{row}-label-cell' class='prop-label' style='width:{label}px'><text id='{row}-label' text='{}' /></panel><panel id='{row}-cell' class='prop-value'><text id='{row}-source' class='{}' text='{}' /><panel id='{row}-fill' class='fill' /><text id='{row}-type' class='caption' text='{}' /></panel></panel>",escape(&pin),if source.is_some(){"fg"}else{"muted"},escape(source.as_deref().unwrap_or("Not connected")),escape(&style::short_type(&socket.ty))));
            }
            for (i, (port, socket)) in node
                .ports
                .iter()
                .filter(|(_, p)| p.direction == ara_graph::Direction::Output)
                .enumerate()
            {
                if i == 0 {
                    out.push_str(&format!("<panel id='node-output-header-{n}' class='side-header'><text id='node-output-title-{n}' class='caption' text='Output' /></panel>"));
                }
                let users = graph
                    .edges
                    .values()
                    .filter(|e| e.from.node == *id && e.from.port == *port)
                    .count();
                let uses = match users {
                    0 => "Unused".to_owned(),
                    1 => "Feeds 1 input".to_owned(),
                    n => format!("Feeds {n} inputs"),
                };
                let row = format!("node-output-{n}-{i}");
                out.push_str(&format!("<panel id='{row}' class='prop-row'><panel id='{row}-label-cell' class='prop-label' style='width:{label}px'><text id='{row}-label' text='{}' /></panel><panel id='{row}-cell' class='prop-value'><text id='{row}-uses' class='{}' text='{uses}' /></panel></panel>",escape(&style::short_type(&socket.ty)),if users>0{"fg"}else{"muted"}));
            }
        }
        out.push_str("</scroll>");
        Ok(())
    }
}
