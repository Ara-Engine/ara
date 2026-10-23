use super::panels::display;
use super::*;
impl Shell {
    pub(super) fn search(&mut self, out: &mut String, panel: &str, editor: &Editor) {
        let id = format!("search-{panel}");
        self.bindings.insert(
            ara_ui::source::stable_id(&id),
            Binding::Search(panel.into()),
        );
        out.push_str(&format!("<field id='{id}' style='flex-grow:1' text='{}' label='Search {panel}' action='Dispatch' />",escape(editor.state.searches.get(panel).map(String::as_str).unwrap_or(""))));
    }
    pub(super) fn context_menu(&mut self, editor: &Editor, out: &mut String) {
        out.push_str(&format!(
            "<panel id='context-menu' class='context' style='left:{}px; top:{}px'>",
            self.pointer[0].clamp(0., (self.ui.size[0] - 230.).max(0.)),
            self.pointer[1].clamp(0., (self.ui.size[1] - 240.).max(0.))
        ));
        if let Some(id) = editor.state.selection.first() {
            for (name, label, command, args) in [
                (
                    "duplicate",
                    "Duplicate",
                    "editor.entity.duplicate",
                    json!({"entity":id}),
                ),
                (
                    "delete",
                    "Delete",
                    "entity.delete",
                    json!({"entities":editor.state.selection}),
                ),
                (
                    "child",
                    "Create empty child",
                    "entity.create",
                    json!({"parent":id,"name":"Entity","components":{"Transform":{}}}),
                ),
                (
                    "unpack",
                    "Unpack prefab",
                    "editor.prefab.unpack",
                    json!({"entity":id}),
                ),
            ] {
                out.push_str(&self.bind_button(
                    &format!("context-{name}"),
                    label,
                    Binding::Content(vec![json!({"cmd":command,"args":args})], label.into()),
                    "quiet",
                ));
            }
            out.push_str(&self.bind_button("context-rename", "Rename", Binding::Rename, "quiet"));
        }
        out.push_str("</panel>");
    }
    pub(super) fn palette(&mut self, editor: &mut Editor, out: &mut String) -> ara_ui::Result<()> {
        out.push_str(&format!("<panel id='palette' class='palette' style='left:{}px; top:{}px'><text id='palette-title' text='Command palette' />",self.ui.size[0]*0.24,self.ui.size[1]*0.12));
        self.bindings.insert(
            ara_ui::source::stable_id("palette-search"),
            Binding::Search("palette".into()),
        );
        out.push_str(&format!(
            "<field id='palette-search' label='Search commands' text='{}' action='Dispatch' />",
            escape(
                editor
                    .state
                    .searches
                    .get("palette")
                    .map(String::as_str)
                    .unwrap_or("")
            )
        ));
        if let Some(name) = self.palette_command.clone() {
            if name == "@components" {
                let query = editor
                    .state
                    .searches
                    .get("palette")
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                let names: Vec<_> = editor
                    .host
                    .store()
                    .schema()
                    .components()
                    .filter(|t| t.name.to_lowercase().contains(&query))
                    .map(|t| t.name.clone())
                    .collect();
                for (index, name) in names.iter().enumerate() {
                    let calls=editor.state.selection.iter().map(|entity|json!({"cmd":"component.add","args":{"entity":entity,"component":name}})).collect();
                    out.push_str(&self.bind_button(
                        &format!("component-choice-{index}"),
                        name,
                        Binding::Content(calls, format!("Add {name}")),
                        "quiet",
                    ));
                }
                out.push_str("</panel>");
                return Ok(());
            }
            out.push_str(&format!(
                "<text id='palette-command' class='accent' text='{}' />",
                escape(&name)
            ));
            let schema = editor
                .host
                .catalog()
                .iter()
                .find(|m| m.name == name)
                .map(|m| m.params.clone())
                .or_else(|| editor.command_schemas.get(&name).cloned());
            if let Some(schema) = schema
                && let Some(properties) = schema["properties"].as_object()
            {
                for (key, prop) in properties {
                    if key == "session" {
                        continue;
                    }
                    let id = format!("param-{key}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&id),
                        Binding::PaletteParam(key.clone()),
                    );
                    let value = self
                        .palette_params
                        .get(key)
                        .map(display)
                        .unwrap_or_default();
                    out.push_str(&format!("<text id='param-label-{key}' class='muted' text='{}' /><field id='{id}' text='{}' action='Dispatch' description='{}' />",escape(key),escape(&value),escape(prop["description"].as_str().unwrap_or(""))));
                }
            }
            out.push_str(&self.bind_button(
                "palette-run",
                "Run command",
                Binding::PaletteRun,
                "selected",
            ));
        } else {
            let query = editor
                .state
                .searches
                .get("palette")
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let catalog = editor
                .action("schema.list", json!({"kind":"command"}))
                .unwrap_or(Value::Null);
            let mut commands: Vec<(String, String)> = Vec::new();
            if let Some(items) = catalog["items"].as_array() {
                for item in items {
                    if let Some(name) = item["name"].as_str() {
                        commands.push((name.into(), item["summary"].as_str().unwrap_or("").into()));
                    }
                }
            }
            commands.extend(
                editor
                    .host
                    .catalog()
                    .iter()
                    .filter(|m| m.name.starts_with("editor."))
                    .map(|m| (m.name.clone(), m.summary.clone())),
            );
            commands.sort_by(|a, b| a.0.cmp(&b.0));
            out.push_str("<scroll id='palette-results' style='flex-grow:1'>");
            for (index, (name, summary)) in commands
                .into_iter()
                .filter(|(n, s)| format!("{n} {s}").to_lowercase().contains(&query))
                .take(30)
                .enumerate()
            {
                out.push_str(&self.bind_button(
                    &format!("command-{index}"),
                    &format!("{name}   {summary}"),
                    Binding::Palette(name),
                    "quiet",
                ));
            }
            out.push_str("</scroll>");
        }
        out.push_str("</panel>");
        Ok(())
    }
}
