//! Node parameters stay typed and share the graph/DSL transaction path.
use super::*;
impl Shell {
    pub(super) fn material_inspector(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        out.push_str("<scroll id='material-inspector' class='body'><text id='material-inspector-title' class='heading' text='Node properties' />");
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
                    out.push_str(&format!("<text id='material-node-error-{n}' class='destructive' style='white-space:normal' text='{}' />",escape(&diagnostic.message)));
                }
                None
            }
        };
        if editor.state.canvas_selection.is_empty() {
            out.push_str("<text id='material-inspector-hint' class='muted' text='Select a node to inspect its typed inputs and values.' style='white-space:normal' />");
        }
        for (n, id) in editor.state.canvas_selection.iter().enumerate() {
            let Some(node) = graph.nodes.get(id) else {
                continue;
            };
            let name = node
                .payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&node.kind);
            out.push_str(&format!("<text id='node-title-{n}' class='heading' text='{}' /><text id='node-kind-{n}' class='muted' text='{}' /><panel id='node-divider-{n}' class='divider' />",escape(name),escape(&node.kind)));
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
                out.push_str(&format!("<text id='{key}-label' class='field-label' text='Default value' /><field id='{key}' class='number' text='{}' action='Dispatch' label='Typed numeric lanes, boolean or texture value' />",escape(&draft)));
            }
            for (i, (port, socket)) in node.ports.iter().enumerate() {
                out.push_str(&format!("<panel id='node-port-{n}-{i}' class='field-row'><text id='node-port-name-{n}-{i}' class='field-label' text='{}' /><text id='node-port-type-{n}-{i}' class='badge' text='{}' /></panel>",escape(port),escape(&socket.ty)));
            }
        }
        out.push_str("</scroll>");
        Ok(())
    }
}
