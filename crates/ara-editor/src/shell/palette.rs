use super::panels::display;
use super::*;
impl Shell {
    pub(super) fn search(&mut self, out: &mut String, panel: &str, editor: &Editor) {
        let id = format!("search-{panel}");
        self.bindings.insert(
            ara_ui::source::stable_id(&id),
            Binding::Search(panel.into()),
        );
        let value = editor
            .state
            .searches
            .get(panel)
            .map(String::as_str)
            .unwrap_or("");
        out.push_str(&format!("<panel id='search-wrap-{panel}' class='search'><field id='{id}' class='search-input' text='{}' label='Search {panel}' action='Dispatch' /><image id='search-icon-{panel}' class='search-icon' src='icon-search' />{} </panel>",escape(value),if value.is_empty() {format!("<text id='search-placeholder-{panel}' class='placeholder' text='Search {panel}…' />")}else{String::new()}));
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
        out.push_str(&format!("<panel id='palette' class='palette' style='left:{}px; top:{}px; width:{}px'><panel id='palette-heading' class='row'>{}<text id='palette-title' text='Run a command' /><panel id='palette-title-fill' style='flex-grow:1' /><text id='palette-escape' class='palette-shortcut' text='Esc' /></panel>",(self.ui.size[0]-640.).max(16.)*0.5,self.ui.size[1]*0.18,(self.ui.size[0]-32.).min(640.),icons::icon("palette-heading","command")));
        self.bindings.insert(
            ara_ui::source::stable_id("palette-search"),
            Binding::Search("palette".into()),
        );
        let query = editor
            .state
            .searches
            .get("palette")
            .map(String::as_str)
            .unwrap_or("");
        out.push_str(&format!("<panel id='palette-search-wrap' style='height:36px'><field id='palette-search' class='search-input' style='height:36px' label='Search commands' text='{}' action='Dispatch' /><image id='palette-search-icon' class='search-icon' style='top:10px' src='icon-search' />{}</panel>",escape(query),if query.is_empty(){"<text id='palette-placeholder' class='placeholder' style='top:8px' text='Search commands...' />"}else{""}));
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
            let mut results: Vec<_> = commands
                .into_iter()
                .filter_map(|(name, summary)| {
                    let matched = fuzzy(&name, &query).or_else(|| fuzzy(&summary, &query));
                    matched.map(|(score, _)| (name, summary, score))
                })
                .collect();
            results.sort_by(|a, b| {
                b.0.starts_with("editor.")
                    .cmp(&a.0.starts_with("editor."))
                    .then(a.2.cmp(&b.2))
                    .then(a.0.cmp(&b.0))
            });
            out.push_str("<panel id='palette-divider' class='divider' /><scroll id='palette-results' style='flex-grow:1; flex-basis:0px'>");
            let mut group = "";
            for (index, (name, summary, _)) in results.into_iter().take(30).enumerate() {
                let next = if name.starts_with("editor.") {
                    "Editor"
                } else {
                    "Scene commands"
                };
                if group != next {
                    group = next;
                    out.push_str(&format!(
                        "<text id='palette-group-{index}' class='palette-group' text='{group}' />"
                    ));
                }
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("command-{index}")),
                    Binding::Palette(name.clone()),
                );
                out.push_str(&format!("<button id='command-{index}' class='palette-result' label='{}' action='Dispatch'>{}<panel id='command-copy-{index}' class='palette-copy'><panel id='command-name-{index}' style='flex-direction:row'>",escape(&name),icons::icon(&format!("command-{index}"),if next=="Editor"{"command"}else{"box"})));
                let matched = fuzzy(&name, &query).map(|(_, m)| m).unwrap_or_default();
                // Runs preserve exact text and give matched characters the accent token.
                let mut run = String::new();
                let mut highlighted = false;
                let mut n = 0;
                for (i, c) in name.chars().enumerate() {
                    let h = matched.contains(&i);
                    if h != highlighted && !run.is_empty() {
                        out.push_str(&format!("<text id='match-{index}-{n}' class='{}' text='{}' style='font-size:12px' />",if highlighted{"accent"}else{""},escape(&run)));
                        run.clear();
                        n += 1;
                    }
                    highlighted = h;
                    run.push(c);
                }
                if !run.is_empty() {
                    out.push_str(&format!("<text id='match-{index}-{n}' class='{}' text='{}' style='font-size:12px' />",if highlighted{"accent"}else{""},escape(&run)));
                }
                out.push_str(&format!("</panel><text id='command-summary-{index}' class='badge' text='{}' /></panel><text id='command-key-{index}' class='palette-shortcut' text='{}' /></button>",escape(&summary),shortcut(&name)));
            }
            out.push_str("</scroll>");
        }
        out.push_str("<panel id='palette-footer' class='toolbar'><text id='palette-footer-copy' class='badge' text='Tab to navigate    Enter to choose' /></panel></panel>");
        Ok(())
    }
}

/// Subsequence matching with a compactness score; byte offsets never split UTF-8.
pub(super) fn fuzzy(value: &str, query: &str) -> Option<(usize, Vec<usize>)> {
    let mut wanted = query.chars().filter(|c| !c.is_whitespace()).peekable();
    let mut positions = Vec::new();
    for (i, c) in value.chars().enumerate() {
        if wanted.peek().is_some_and(|w| c.eq_ignore_ascii_case(w)) {
            positions.push(i);
            wanted.next();
        }
    }
    if wanted.peek().is_some() {
        return None;
    }
    let score = positions.last().copied().unwrap_or(0) + positions.first().copied().unwrap_or(0);
    Some((score, positions))
}
fn shortcut(name: &str) -> &'static str {
    match name {
        "editor.frame" => "F",
        "editor.palette" => "Ctrl K",
        "editor.tool" => "W / E / R",
        "history.undo" => "Ctrl Z",
        "history.redo" => "Ctrl Y",
        _ => "Enter",
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn fuzzy_is_ordered_and_unicode_safe() {
        assert!(super::fuzzy("transform.set", "trset").is_some());
        assert!(super::fuzzy("frame", "fm").is_some());
        assert!(super::fuzzy("frame", "mf").is_none());
        assert_eq!(
            super::fuzzy("\u{e9}cho", "\u{e9}o").map(|(_, p)| p),
            Some(vec![0, 3])
        );
    }
}
