//! Command palette v2: human titles first, then the menu path and dimmed id, with
//! keycaps. Prefixes: `@` entities and assets, `#` scenes and assets, `?` help.
//! Engine (content) commands keep their generated parameter form.
use super::commands::{Avail, Target, all, menu_path};
use super::panels::display;
use super::*;

/// One palette row.
pub(super) struct Item {
    pub(super) title: String,
    detail: String,
    id: String,
    icon: &'static str,
    pub(super) binding: Binding,
    keys: Vec<&'static str>,
    reason: Option<String>,
    pub(super) group: &'static str,
    score: usize,
}
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
        let hint = match panel {
            "hierarchy" => "Filter  (t:Light)",
            // The type chips filter by type; `type:` is in the empty state's hint.
            "assets" => "Search assets",
            "console" => "Filter messages",
            "problems" => "Filter problems",
            "settings" => "Search settings",
            "shortcuts" => "Search shortcuts",
            _ => "Search",
        };
        out.push_str(&format!("<panel id='search-wrap-{panel}' class='search'><field id='{id}' class='search-input' text='{}' label='Search {panel}' action='Dispatch' /><image id='search-icon-{panel}' class='search-icon' src='icon-search' tint='true' />{} </panel>",escape(value),if value.is_empty() {format!("<text id='search-placeholder-{panel}' class='placeholder' text='{hint}' />")}else{String::new()}));
    }
    pub(super) fn palette_items(&mut self, editor: &mut Editor, query: &str) -> Vec<Item> {
        let mut items = Vec::new();
        let (mode, term) = match query.chars().next() {
            Some(c @ ('@' | '#' | '?' | '>')) => (c, query[1..].trim().to_lowercase()),
            _ => ('>', query.trim().to_lowercase()),
        };
        let score = |text: &str| fuzzy(&text.to_lowercase(), &term).map(|(s, _)| s);
        if mode == '>' || mode == '?' {
            for spec in all() {
                if matches!(
                    spec.id,
                    "file.open_scene"
                        | "window.focus_window"
                        | "window.show_panel"
                        | "layout.load"
                        | "snap.set"
                        | "edit.select_entity"
                ) {
                    continue;
                }
                let cx = self.ctx(editor, &Target::None);
                let title = spec.title(&cx);
                let path = menu_path(spec.id);
                let haystack = if mode == '?' {
                    format!("{title} {} {}", spec.summary, spec.id)
                } else {
                    format!("{title} {path} {}", spec.id)
                };
                let Some(score) = score(&title).or_else(|| score(&haystack).map(|s| s + 1000))
                else {
                    continue;
                };
                let recent = self.recent_commands.iter().position(|r| *r == spec.id);
                let avail = spec.avail(&cx);
                items.push(Item {
                    title,
                    detail: if mode == '?' {
                        spec.summary.to_owned()
                    } else {
                        path
                    },
                    id: spec.id.to_owned(),
                    icon: if !spec.icon.is_empty() && icons::exists(spec.icon) {
                        spec.icon
                    } else {
                        "command"
                    },
                    binding: Binding::Command(spec.id, Target::None),
                    keys: self
                        .keymap
                        .chords(spec.id)
                        .first()
                        .map(|c| c.caps())
                        .unwrap_or_default(),
                    reason: match avail {
                        Avail::Blocked(r) => Some(r),
                        _ => None,
                    },
                    group: if term.is_empty() && recent.is_some() {
                        "Recent"
                    } else {
                        "Commands"
                    },
                    score: recent.filter(|_| term.is_empty()).unwrap_or(score + 100),
                });
            }
            if mode == '>' {
                let catalog = editor
                    .action("schema.list", json!({"kind":"command"}))
                    .unwrap_or(Value::Null);
                for item in catalog["items"].as_array().into_iter().flatten() {
                    let Some(name) = item["name"].as_str() else {
                        continue;
                    };
                    let Some(s) = score(name) else { continue };
                    items.push(Item {
                        title: name.to_owned(),
                        detail: item["summary"].as_str().unwrap_or("").to_owned(),
                        id: String::new(),
                        icon: "box",
                        binding: Binding::Palette(name.to_owned()),
                        keys: Vec::new(),
                        reason: None,
                        group: "Engine commands",
                        score: s + 5000,
                    });
                }
            }
        }
        if mode == '@'
            && let Ok(resolved) = self.resolved(editor)
        {
            {
                for (id, e) in resolved.iter() {
                    let name = e.record.get("name").and_then(Value::as_str).unwrap_or(id);
                    let Some(s) = score(name).or_else(|| score(id)) else {
                        continue;
                    };
                    items.push(Item {
                        title: name.to_owned(),
                        detail: "Entity".into(),
                        id: id.clone(),
                        icon: "box",
                        binding: Binding::Command("edit.select_entity", Target::Entity(id.clone())),
                        keys: Vec::new(),
                        reason: None,
                        group: "Entities",
                        score: s,
                    });
                }
            }
        }
        if mode == '#' {
            // Every scene in the project, recently opened first: File > Open Scene.
            let workspace = editor.host.store().workspace();
            for (_, doc) in workspace.docs().filter(|(_, d)| d.kind == "scene") {
                let id = doc.id.to_string();
                let path = workspace.path_of(&id).unwrap_or(&id).to_owned();
                let Some(s) = score(&path) else { continue };
                let recent = editor.state.recent_scenes.iter().position(|r| *r == id);
                items.push(Item {
                    title: path.rsplit('/').next().unwrap_or(&path).to_owned(),
                    detail: if id == editor.state.doc {
                        format!("{path} · open")
                    } else {
                        path.clone()
                    },
                    id: String::new(),
                    icon: "clapperboard",
                    binding: Binding::Command("file.open_scene", Target::Doc(id)),
                    keys: Vec::new(),
                    reason: None,
                    group: "Scenes",
                    score: recent.filter(|_| term.is_empty()).unwrap_or(s + 100),
                });
            }
        }
        if mode == '@' || mode == '#' {
            let workspace = editor.host.store().workspace();
            for asset in &self.browser.items {
                let Some(s) = score(&asset.path) else {
                    continue;
                };
                let scene = workspace.find(&asset.path).and_then(|id| {
                    workspace
                        .doc(id.as_str())
                        .filter(|d| d.kind == "scene")
                        .map(|_| id.to_string())
                });
                if mode == '#' && scene.is_some() {
                    continue;
                }
                items.push(Item {
                    title: asset
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&asset.path)
                        .to_owned(),
                    detail: asset.path.clone(),
                    id: String::new(),
                    icon: if scene.is_some() {
                        "clapperboard"
                    } else {
                        "file"
                    },
                    binding: match scene {
                        Some(doc) => Binding::Command("file.open_scene", Target::Doc(doc)),
                        None => Binding::Command("assets.open", Target::Asset(asset.path.clone())),
                    },
                    keys: Vec::new(),
                    reason: None,
                    group: "Assets",
                    score: s + 1000,
                });
            }
        }
        let order = |g: &str| match g {
            "Recent" => 0,
            "Entities" => 1,
            "Commands" => 2,
            "Scenes" => 3,
            "Assets" => 4,
            _ => 5,
        };
        items.sort_by(|a, b| {
            order(a.group)
                .cmp(&order(b.group))
                .then(a.score.cmp(&b.score))
                .then(a.title.cmp(&b.title))
        });
        items.truncate(60);
        items
    }
    pub(super) fn palette(&mut self, editor: &mut Editor, out: &mut String) -> ara_ui::Result<()> {
        let width = (self.ui.size[0] - 32.).min(640.);
        out.push_str(&format!(
            "<panel id='palette' class='palette' style='left:{}px; top:{}px; width:{width}px'>",
            ((self.ui.size[0] - width) * 0.5).max(16.),
            self.ui.size[1] * 0.16
        ));
        self.bindings.insert(
            ara_ui::source::stable_id("palette-search"),
            Binding::Search("palette".into()),
        );
        let query = editor
            .state
            .searches
            .get("palette")
            .cloned()
            .unwrap_or_default();
        out.push_str(&format!("<panel id='palette-search-wrap' style='height:32px;flex-shrink:0'><field id='palette-search' class='search-input' style='height:32px;padding-top:6px;padding-left:30px;font-size:13px' label='Search commands' text='{}' action='Dispatch' /><image id='palette-search-icon' class='search-icon' style='top:8px;left:8px' src='icon-search' tint='true' />{}</panel>",escape(&query),if query.is_empty(){"<text id='palette-placeholder' class='placeholder' style='top:6px;left:30px;font-size:13px' text='Run a command  ·  @ entities  ·  # assets  ·  ? help' />"}else{""}));
        if let Some(name) = self.palette_command.clone() {
            self.palette_form(editor, out, &name)?;
            out.push_str("</panel>");
            return Ok(());
        }
        let items = self.palette_items(editor, &query);
        let term = query
            .trim_start_matches(['@', '#', '?', '>'])
            .trim()
            .to_lowercase();
        self.palette_active = self.palette_active.min(items.len().saturating_sub(1));
        out.push_str("<scroll id='palette-results' style='flex-grow:1; flex-basis:0px'>");
        if items.is_empty() {
            out.push_str("<text id='palette-empty' class='caption' text='No matches. Try @ for entities or # for assets.' style='padding:8px' />");
        }
        let mut group = "";
        for (index, item) in items.into_iter().enumerate() {
            if group != item.group {
                group = item.group;
                out.push_str(&format!(
                    "<text id='palette-group-{index}' class='palette-group' text='{group}' />"
                ));
            }
            let id = format!("command-{index}");
            let active = index == self.palette_active;
            out.push_str(&format!("<button id='{id}' class='palette-result{}' label='{}' description='{}' action='Dispatch'{}>{}<panel id='{id}-copy' class='palette-copy'>",if active{" palette-active"}else{""},escape(&item.title),escape(item.reason.as_deref().unwrap_or(&item.detail)),if item.reason.is_some(){" disabled='true'"}else{""},if active{icons::icon_in(&id,item.icon,"on-selection")}else{icons::icon(&id,item.icon)}));
            highlight(out, &id, &item.title, &term, active);
            if !item.detail.is_empty() {
                out.push_str(&format!(
                    "<text id='{id}-path' class='{}' text='{}' />",
                    if active {
                        "palette-path on-selection-muted"
                    } else {
                        "palette-path"
                    },
                    escape(&item.detail)
                ));
            }
            if !item.id.is_empty() && group != "Entities" {
                out.push_str(&format!(
                    "<text id='{id}-id' class='palette-id{}' text='{}' />",
                    if active { " on-selection-muted" } else { "" },
                    escape(&item.id)
                ));
            }
            out.push_str("</panel>");
            if !item.keys.is_empty() {
                out.push_str(&format!("<panel id='{id}-keys' class='keys'>"));
                for (n, cap) in item.keys.iter().enumerate() {
                    out.push_str(&format!("<panel id='{id}-key-{n}' class='keycap'><text id='{id}-key-text-{n}' text='{}' /></panel>",escape(cap)));
                }
                out.push_str("</panel>");
            }
            out.push_str("</button>");
            self.bindings
                .insert(ara_ui::source::stable_id(&id), item.binding);
        }
        out.push_str("</scroll><panel id='palette-footer' class='row' style='height:22px;padding-left:8px'><text id='palette-footer-copy' class='caption' text='↑↓ choose  ·  Enter run  ·  Esc close  ·  Agents call these commands through editor.command' /></panel></panel>");
        Ok(())
    }
    fn palette_form(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
        name: &str,
    ) -> ara_ui::Result<()> {
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
                .map(|t| (t.name.clone(), t.doc.clone()))
                .collect();
            out.push_str("<text id='palette-group-components' class='palette-group' text='Add component' /><scroll id='palette-results' style='flex-grow:1; flex-basis:0px'>");
            for (index, (name, doc)) in names.iter().enumerate() {
                let calls=editor.state.selection.iter().map(|entity|json!({"cmd":"component.add","args":{"entity":entity,"component":name}})).collect();
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("component-choice-{index}")),
                    Binding::Content(calls, format!("Add {name}")),
                );
                out.push_str(&format!("<button id='component-choice-{index}' class='palette-result' label='{}' description='{}' action='Dispatch'>{}<panel id='component-copy-{index}' class='palette-copy'><text id='component-name-{index}' text='{}' /><text id='component-doc-{index}' class='palette-path' text='{}' /></panel></button>",escape(name),escape(doc),icons::icon(&format!("component-choice-{index}"),"box"),escape(name),escape(doc.lines().next().unwrap_or(""))));
            }
            out.push_str("</scroll>");
            return Ok(());
        }
        out.push_str(&format!(
            "<text id='palette-command' class='accent' text='{}' />",
            escape(name)
        ));
        let schema = editor
            .host
            .catalog()
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.params.clone())
            .or_else(|| editor.command_schemas.get(name).cloned());
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
                out.push_str(&format!("<panel id='param-row-{key}' class='prop-row'><text id='param-label-{key}' class='prop-label' style='width:140px' text='{}' /><panel id='param-value-{key}' class='prop-value'><field id='{id}' text='{}' action='Dispatch' description='{}' style='flex-grow:1' /></panel></panel>",escape(key),escape(&value),escape(prop["description"].as_str().unwrap_or(""))));
            }
        }
        out.push_str(&self.bind_button(
            "palette-run",
            "Run command",
            Binding::PaletteRun,
            "primary",
        ));
        Ok(())
    }
}
/// Title text with matched characters in the accent color.
fn highlight(out: &mut String, id: &str, title: &str, term: &str, active: bool) {
    let matched = fuzzy(&title.to_lowercase(), term)
        .map(|(_, m)| m)
        .unwrap_or_default();
    let mut run = String::new();
    let mut highlighted = false;
    let mut n = 0;
    let flush = |out: &mut String, run: &mut String, highlighted: bool, n: &mut usize| {
        if !run.is_empty() {
            out.push_str(&format!(
                "<text id='{id}-match-{n}' class='{}' text='{}' style='font-size:13px' />",
                if highlighted && !active {
                    "accent"
                } else if active {
                    "on-selection"
                } else {
                    "fg"
                },
                escape(run)
            ));
            run.clear();
            *n += 1;
        }
    };
    out.push_str(&format!(
        "<panel id='{id}-title' style='flex-direction:row;flex-shrink:0'>"
    ));
    for (i, c) in title.chars().enumerate() {
        let h = matched.contains(&i);
        if h != highlighted {
            flush(out, &mut run, highlighted, &mut n);
        }
        highlighted = h;
        run.push(c);
    }
    flush(out, &mut run, highlighted, &mut n);
    out.push_str("</panel>");
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
    let spread = positions.last().copied().unwrap_or(0) - positions.first().copied().unwrap_or(0);
    let score = spread + positions.first().copied().unwrap_or(0);
    Some((score, positions))
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
