//! Comments/groups are normal graph-layout commands, including controller projections.
use super::*;
impl Shell {
    pub(super) fn annotation_inspector(&mut self, editor: &Editor, out: &mut String) -> bool {
        let Some(shown) = self.graphs.get(self.graph_page) else {
            return false;
        };
        let (canvas, doc) = (&shown.canvas, &shown.doc);
        let selected = editor.state.canvas_selection.iter().find_map(|id| {
            canvas
                .graph()
                .editor
                .comments
                .get(id)
                .map(|c| (id.clone(), c.text.clone(), "Comment"))
                .or_else(|| {
                    canvas
                        .graph()
                        .editor
                        .groups
                        .get(id)
                        .map(|g| (id.clone(), g.label.clone(), "Group"))
                })
        });
        let Some((key, label, kind)) = selected else {
            return false;
        };
        let value = self
            .drafts
            .get(&format!("@graph-label|{doc}|{key}"))
            .unwrap_or(&label);
        out.push_str(&format!("<text id='annotation-kind' class='heading' text='{kind}' /><text id='annotation-label-title' class='field-label' text='Label' /><field id='annotation-label' text='{}' action='Dispatch' label='{kind} label' />",escape(value)));
        self.bindings.insert(
            ara_ui::source::stable_id("annotation-label"),
            Binding::GraphLabel(doc.clone(), key),
        );
        true
    }
    pub(super) fn commit_graph_labels(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let keys = self
            .drafts
            .keys()
            .filter(|k| k.starts_with("@graph-label|"))
            .cloned()
            .collect::<Vec<_>>();
        for key in &keys {
            let parts = key.split('|').collect::<Vec<_>>();
            if let [_, doc, item] = parts.as_slice() {
                let document = editor
                    .host
                    .store()
                    .workspace()
                    .doc(doc)
                    .ok_or("annotation document missing")?;
                let animation = document.kind == "animation";
                let mut layout = if animation {
                    let path = crate::authoring::animation::layout_path(doc);
                    editor
                        .host
                        .store()
                        .workspace()
                        .find(&path)
                        .and_then(|id| editor.host.store().workspace().doc(id.as_str()))
                        .and_then(|d| ara_graph::Graph::from_document(d).ok())
                        .map(|g| g.editor)
                        .unwrap_or_default()
                } else {
                    ara_graph::Graph::from_document(document)
                        .map_err(|d| d.to_string())?
                        .editor
                };
                let label = self.drafts.get(key).ok_or("annotation draft missing")?;
                if let Some(comment) = layout.comments.get_mut(*item) {
                    comment.text.clone_from(label);
                } else if let Some(group) = layout.groups.get_mut(*item) {
                    group.label.clone_from(label);
                } else {
                    return Err("annotation no longer exists".into());
                }
                if animation {
                    editor.action(
                        "editor.animation.layout",
                        json!({"doc":doc,"editor":layout}),
                    )?;
                } else {
                    editor.apply_to(
                        doc,
                        vec![json!({"cmd":"graph.editor.set","args":{"editor":layout}})],
                        "Edit graph annotation",
                    )?;
                }
            }
        }
        let changed = !keys.is_empty();
        for key in keys {
            self.drafts.remove(&key);
        }
        Ok(changed)
    }
}
