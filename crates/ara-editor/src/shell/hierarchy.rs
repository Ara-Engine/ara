use super::*;
impl Shell {
    pub(super) fn hierarchy_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        out.push_str("<panel id='hierarchy-toolbar' class='toolbar'>");
        out.push_str(&self.bind_button("entity-create","+",Binding::Content(vec![json!({"cmd":"entity.create","args":{"name":"Entity","components":{"Transform":{}}}})],"Create entity".into()),"small"));
        self.search(out, "hierarchy", editor);
        out.push_str(&self.bind_button("hierarchy-menu", "...", Binding::Context, "small"));
        out.push_str("</panel>");
        if let Some(id) = editor.state.selection.first() {
            out.push_str("<panel id='hierarchy-actions' class='toolbar'>");
            out.push_str(&self.bind_button(
                "hierarchy-collapse",
                if editor.state.collapsed.contains(id) {
                    "Expand"
                } else {
                    "Collapse"
                },
                Binding::Protocol(
                    "editor.collapse",
                    json!({"entity":id,"enabled":!editor.state.collapsed.contains(id)}),
                ),
                "small quiet",
            ));
            out.push_str(&self.bind_button(
                "hierarchy-lock",
                if editor.state.locked.contains(id) {
                    "Unlock"
                } else {
                    "Lock"
                },
                Binding::Protocol(
                    "editor.lock",
                    json!({"entity":id,"enabled":!editor.state.locked.contains(id)}),
                ),
                "small quiet",
            ));
            out.push_str(&self.bind_button(
                "hierarchy-hide",
                if editor.state.hidden.contains(id) {
                    "Show"
                } else {
                    "Hide"
                },
                Binding::Protocol(
                    "editor.visibility",
                    json!({"entity":id,"enabled":!editor.state.hidden.contains(id)}),
                ),
                "small quiet",
            ));
            out.push_str("</panel>");
        }
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?;
        let changed = editor.agent_fields();
        let changed_entities: std::collections::BTreeSet<_> =
            changed.keys().map(|(id, _)| id.as_str()).collect();
        self.hierarchy_tints = editor.review_tints();
        self.hierarchy_ghosts.clear();
        let mut children: BTreeMap<Option<&str>, Vec<&String>> = BTreeMap::new();
        for (id, e) in &resolved {
            children.entry(e.parent.as_deref()).or_default().push(id);
        }
        for kids in children.values_mut() {
            kids.sort_by(|a, b| resolved[*a].order.cmp(&resolved[*b].order).then(a.cmp(b)));
        }
        let mut stack: Vec<_> = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .map(|id| (*id, 0usize))
            .collect();
        self.hierarchy.items.clear();
        self.hierarchy_entities.clear();
        let query = editor
            .state
            .searches
            .get("hierarchy")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        while let Some((id, depth)) = stack.pop() {
            let e = &resolved[id];
            let name = e.record.get("name").and_then(Value::as_str).unwrap_or(id);
            let component_filter = query.strip_prefix("type:");
            let matches = component_filter.map_or_else(
                || query.is_empty() || name.to_lowercase().contains(&query) || id.contains(&query),
                |kind| e.record.keys().any(|k| k.to_lowercase().contains(kind)),
            );
            if matches {
                let key = crate::world::render_id(id);
                let badge = if e.source.0.as_str() != editor.state.doc {
                    " P"
                } else {
                    ""
                };
                let modified = if changed_entities.contains(id.as_str()) {
                    " *"
                } else {
                    ""
                };
                let selected = if editor.state.selection.contains(id) {
                    "> "
                } else {
                    "  "
                };
                let text = format!(
                    "{}{}{}{}{}{}",
                    "  ".repeat(depth.min(32)),
                    selected,
                    name,
                    badge,
                    modified,
                    if editor.state.locked.contains(id) {
                        " [L]"
                    } else if editor.state.hidden.contains(id) {
                        " [H]"
                    } else {
                        ""
                    }
                );
                self.hierarchy.items.push((key, text));
                self.hierarchy_entities.insert(key, id.clone());
            }
            if (!editor.state.collapsed.contains(id) || !query.is_empty())
                && let Some(kids) = children.get(&Some(id.as_str())) {
                    stack.extend(kids.iter().rev().map(|kid| (*kid, depth + 1)));
                }
        }
        for (id, record) in editor.view()?.ghosts() {
            let key = crate::world::render_id(id);
            self.hierarchy.items.push((
                key,
                format!(
                    "[deleted] {}",
                    record.get("name").and_then(Value::as_str).unwrap_or(id)
                ),
            ));
            self.hierarchy_entities.insert(key, id.clone());
            self.hierarchy_ghosts.insert(key);
            self.hierarchy_tints.insert(key, "destructive-foreground");
        }
        out.push_str("<scroll id='hierarchy-rows' style='flex-grow:1; min-height:0px' />");
        Ok(())
    }
    pub(super) fn decorate_hierarchy(&mut self) {
        let Some(view) = &self.hierarchy_view else {
            return;
        };
        for index in view.visible_nodes() {
            let key = self.ui.nodes[*index]
                .element
                .id
                .rsplit_once("key-")
                .and_then(|(_, s)| s.parse::<u64>().ok());
            let class = key
                .and_then(|k| self.hierarchy_tints.get(&k))
                .map(|t| match *t {
                    "success-foreground" => "success",
                    "destructive-foreground" => "destructive",
                    _ => "warning",
                })
                .unwrap_or("");
            for position in 0..self.ui.nodes[*index].element.children.len() {
                let child = self.ui.nodes[*index].element.children[position];
                let node = &mut self.ui.nodes[child];
                if node.element.attrs.get("class").map(String::as_str) != Some(class) {
                    node.element.attrs.insert("class".into(), class.into());
                    node.dirty.style = true;
                }
            }
        }
    }
}
