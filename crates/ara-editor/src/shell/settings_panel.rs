//! One Settings surface for Preferences (user) and Project Settings (Ctrl+, and
//! Ctrl+Alt+,): search with per-category counts, a modified-only filter, a 2 px
//! modified marker with ↺ reset, and the Keyboard Shortcuts page with presets.
//! Every edit is an undoable `editor.settings.set`.
use super::commands::keys::Preset;
use super::*;
use crate::settings::{DEFS, Def, Kind, Scope, categories};

const SHORTCUTS: &str = "Keyboard shortcuts";
impl Shell {
    pub(super) fn settings_panel(&mut self, editor: &mut Editor, out: &mut String) {
        let user = editor.state.settings_scope != "project";
        let scope = if user { Scope::User } else { Scope::Project };
        let query = editor
            .state
            .searches
            .get("settings")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let modified_only = editor.state.settings_modified;
        let category = editor.state.settings_category.clone();
        out.push_str("<panel id='settings-toolbar' class='toolbar' style='height:36px;padding-left:8px;gap:8px'><panel id='settings-scope' class='segment'>");
        for (id, label, value) in [
            ("settings-user", "Preferences", "user"),
            ("settings-project", "Project", "project"),
        ] {
            let on = (value == "user") == user;
            self.bindings.insert(
                ara_ui::source::stable_id(id),
                Binding::SettingChoice("@view".into(), json!({"scope": value, "category": ""})),
            );
            out.push_str(&format!("<button id='{id}' class='segment-button{}' label='{label}' description='{}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",if on{" selected"}else{""},if value=="user"{"Your editor preferences, on every project"}else{"Settings shared by everyone on this project"}));
        }
        out.push_str("</panel>");
        self.search(out, "settings", editor);
        self.bindings.insert(
            ara_ui::source::stable_id("settings-modified"),
            Binding::SettingChoice("@view".into(), json!({"modified": !modified_only})),
        );
        out.push_str(&format!("<button id='settings-modified' class='{}' label='Modified only' description='Show only settings that differ from their defaults' action='Dispatch'>{}<text id='settings-modified-label' text='Modified only' /></button></panel>",if modified_only{"selected"}else{""},icons::icon("settings-modified",if modified_only{"check"}else{"circle"})));
        let banner = if user {
            "Preferences are yours: stored in your user profile (editor-settings.json), never in the project."
        } else {
            "Project settings are shared: stored in ara.project.json and versioned with the project."
        };
        out.push_str(&format!("<panel id='settings-banner' class='banner'>{}<text id='settings-banner-text' text='{}' /></panel>",icons::icon("settings-banner","info"),escape(banner)));
        let matches = |d: &Def| {
            d.scope == scope
                && (!modified_only || editor.state.settings.get(d.key) != Some(&default_of(d)))
                && (query.is_empty()
                    || format!("{} {} {} {}", d.label, d.description, d.key, d.category)
                        .to_lowercase()
                        .contains(&query))
        };
        out.push_str("<panel id='settings-content' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px'><scroll id='settings-side' class='settings-side'>");
        let total = DEFS.iter().filter(|d| matches(d)).count();
        let mut cats = vec![("", "All".to_owned(), total)];
        for c in categories(scope) {
            let count = DEFS
                .iter()
                .filter(|d| d.category == c && matches(d))
                .count();
            cats.push((c, c.to_owned(), count));
        }
        for (key, label, count) in cats {
            let id = format!("settings-cat-{}", key.replace(' ', "-").replace('&', "and"));
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::SettingChoice("@view".into(), json!({"category": key})),
            );
            let on = category == key;
            out.push_str(&format!("<button id='{id}' class='settings-cat{}' label='{}' action='Dispatch'><text id='{id}-label' class='fill' text='{}' /><text id='{id}-count' class='caption' text='{}' /></button>",if on{" settings-cat-on"}else{""},escape(&label),escape(&label),if query.is_empty() && !modified_only {String::new()} else {count.to_string()}));
        }
        out.push_str("</scroll><scroll id='settings-list' class='body' style='gap:2px'>");
        if user && category == SHORTCUTS {
            self.shortcuts_page(editor, out);
        } else {
            let mut shown = 0;
            let mut heading = "";
            for d in DEFS
                .iter()
                .filter(|d| matches(d) && (category.is_empty() || d.category == category))
            {
                if matches!(d.kind, Kind::Map)
                    && !matches!(d.key, "play.networkProfiles" | "hierarchy.filters")
                {
                    continue;
                }
                if heading != d.category {
                    heading = d.category;
                    out.push_str(&format!("<text id='settings-heading-{}' class='heading' text='{}' style='padding:6px;padding-top:10px' />",d.key,escape(heading)));
                }
                self.setting_row(editor, d, out);
                shown += 1;
            }
            if shown == 0 {
                self.empty(
                    out,
                    "settings-none",
                    "search",
                    "No matching settings",
                    "Clear the search or the Modified only filter.",
                    &[],
                );
            }
        }
        out.push_str("</scroll></panel>");
    }
    fn setting_row(&mut self, editor: &Editor, d: &Def, out: &mut String) {
        let key = d.key;
        let id = key.replace('.', "-");
        let value = editor
            .state
            .settings
            .get(key)
            .cloned()
            .unwrap_or(Value::Null);
        let modified = value != default_of(d);
        out.push_str(&format!(
            "<panel id='setting-{id}' class='setting' description='{}'>",
            escape(d.description)
        ));
        if modified {
            // In the row's left padding, clear of the label (as in the Inspector).
            out.push_str(&format!("<panel id='setting-mark-{id}' class='prop-marker' style='left:4px;top:11px;height:14px' />"));
        }
        out.push_str(&format!(
            "<panel id='setting-head-{id}' class='setting-head'>"
        ));
        // The key lives in the label's tooltip; the row keeps one description line.
        out.push_str(&format!(
            "<text id='setting-label-{id}' class='setting-label' text='{}' description='{key}' />",
            escape(d.label)
        ));
        match d.kind {
            Kind::Toggle => {
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("set-{id}")),
                    Binding::Setting(key.into()),
                );
                out.push_str(&format!(
                    "<toggle id='set-{id}' checked='{}' label='{}' action='Dispatch' />",
                    value.as_bool().unwrap_or(false),
                    escape(d.label)
                ));
            }
            Kind::Choice(options) => {
                self.choice_button(
                    &format!("set-{id}"),
                    d.label,
                    choices::Select {
                        destination: choices::Destination::Setting(key.into()),
                        options: options
                            .iter()
                            .map(|(value, label)| (json!(value), (*label).to_owned()))
                            .collect(),
                        current: value.clone(),
                    },
                    out,
                );
            }
            Kind::Number {
                min,
                max,
                step,
                unit,
            } => {
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("set-{id}")),
                    Binding::Setting(key.into()),
                );
                out.push_str(&format!("<field id='set-{id}' class='number' min='{min}' max='{max}' step='{step}' style='width:120px' text='{}' label='{}' description='Type and press Enter, or drag the grip to adjust' action='Dispatch' /><text id='set-{id}-unit' class='caption' text='{}' />",escape(&panels::display(&value)),escape(d.label),escape(unit)));
            }
            Kind::Colors { .. } => {
                // Saved and edited from the colour picker; reset clears them.
                let count = value.as_array().map_or(0, Vec::len);
                let text = match count {
                    0 => "None yet: save colours from the colour picker".to_owned(),
                    1 => "1 colour, in the colour picker".to_owned(),
                    n => format!("{n} colours, in the colour picker"),
                };
                out.push_str(&format!(
                    "<text id='set-{id}' class='caption' text='{}' />",
                    escape(&text)
                ));
            }
            Kind::Map if key == "play.networkProfiles" => {
                self.bind_command(&format!("set-{id}"), "play.options", commands::Target::None);
                out.push_str(&format!("<button id='set-{id}' text='Play options…' label='Client network profiles' action='Dispatch' />"));
            }
            Kind::Map if key == "hierarchy.filters" => {
                let count = value.as_object().map_or(0, serde_json::Map::len);
                self.bind_command(
                    &format!("set-{id}"),
                    "hierarchy.filter.apply",
                    commands::Target::None,
                );
                out.push_str(&format!("<text id='set-{id}-count' class='caption' text='{count} saved' /><button id='set-{id}' text='Manage saved filters…' label='Apply a saved hierarchy filter' description='Choose a named query; save and delete filters from the Hierarchy filter menu' action='Dispatch' />"));
            }
            Kind::Text | Kind::Map => {
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("set-{id}")),
                    Binding::Setting(key.into()),
                );
                out.push_str(&format!("<field id='set-{id}' style='width:260px' text='{}' label='{}' description='Enter to apply' action='Dispatch' />",escape(value.as_str().unwrap_or("")),escape(d.label)));
            }
        }
        if modified {
            self.bindings.insert(
                ara_ui::source::stable_id(&format!("reset-{id}")),
                Binding::SettingChoice(key.into(), Value::Null),
            );
            out.push_str(&format!("<button id='reset-{id}' class='ghost icon-button' label='Reset {}' description='Reset to {}' action='Dispatch'>{}</button>",escape(d.label),escape(&panels::display(&default_of(d))),icons::icon(&format!("reset-{id}"),"undo-2")));
        }
        out.push_str(&format!("<panel id='setting-fill-{id}' class='fill' /></panel><text id='setting-desc-{id}' class='setting-desc' text='{}' /></panel>",escape(d.description)));
    }
    fn shortcuts_page(&mut self, editor: &Editor, out: &mut String) {
        let preset = Preset::parse(
            editor
                .state
                .settings
                .get("input.keymap")
                .and_then(Value::as_str)
                .unwrap_or("ara"),
        );
        out.push_str("<panel id='keymap-row' class='setting-head' style='padding:6px'><text id='keymap-label' class='setting-label' text='Keymap preset' />");
        self.choice_button(
            "keymap-presets",
            "Keymap preset",
            choices::Select {
                destination: choices::Destination::Setting("input.keymap".into()),
                options: Preset::ALL
                    .iter()
                    .map(|p| (json!(p.name()), p.title().to_owned()))
                    .collect(),
                current: json!(preset.name()),
            },
            out,
        );
        out.push_str(
            "</panel><panel id='shortcut-tools' class='toolbar' style='padding-left:6px'>",
        );
        self.search(out, "shortcuts", editor);
        self.bindings.insert(
            ara_ui::source::stable_id("shortcut-keys"),
            Binding::KeySearch,
        );
        out.push_str(&format!("<button id='shortcut-keys' class='{}' label='Search by keys' description='Press a shortcut to find what it runs' action='Dispatch'>{}<text id='shortcut-keys-label' text='{}' /></button></panel>",if self.key_search{"selected"}else{""},icons::icon("shortcut-keys","keyboard"),if self.key_search{"Press keys…"}else{"Record keys"}));
        let query = editor
            .state
            .searches
            .get("shortcuts")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let overrides = editor
            .state
            .settings
            .get("input.shortcuts")
            .and_then(Value::as_object);
        let keymap = &self.keymap;
        // Matching commands under one header per menu, in menu-bar order; commands
        // outside the menus come last. Rows keep only a deeper sub-path.
        let mut groups: Vec<(String, Vec<_>)> = Vec::new();
        for spec in commands::entries(&editor.extensions) {
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
            let chords = keymap.chords(spec.id);
            let keys_text = chords
                .iter()
                .map(|c| c.text().to_lowercase())
                .collect::<Vec<_>>();
            let path = commands::registered_menu_path(&editor.extensions, spec.id);
            if !query.is_empty()
                && !format!("{} {} {}", spec.title, path, spec.id)
                    .to_lowercase()
                    .contains(&query)
                && !keys_text.contains(&query)
            {
                continue;
            }
            let (top, sub) = path.split_once(" › ").unwrap_or((path.as_str(), ""));
            let top = if top.is_empty() { "Other" } else { top }.to_owned();
            let sub = sub.to_owned();
            match groups.iter_mut().find(|(t, _)| *t == top) {
                Some((_, rows)) => rows.push((spec, sub)),
                None => groups.push((top, vec![(spec, sub)])),
            }
        }
        groups.sort_by_key(|(top, _)| *top == "Other");
        let shown = groups.iter().map(|(_, rows)| rows.len()).sum::<usize>();
        for (top, rows) in groups {
            out.push_str(&format!(
                "<text id='shortcut-menu-{}' class='shortcut-menu caption' text='{}' />",
                top.to_lowercase(),
                escape(&top.to_uppercase())
            ));
            for (spec, sub) in rows {
                let id = shortcut_row_id(spec.id, spec.builtin.is_some());
                let chords = keymap.chords(spec.id);
                out.push_str(&format!("<panel id='shortcut-{id}' class='shortcut-row' description='{}'><text id='shortcut-title-{id}' text='{}' /><text id='shortcut-path-{id}' class='caption' text='{}' /><panel id='shortcut-fill-{id}' class='fill' />",escape(spec.summary),escape(spec.title),escape(&sub)));
                let conflicts = keymap.conflicts(spec.id);
                if let Some(other) = conflicts
                    .first()
                    .and_then(|c| commands::lookup(&editor.extensions, c))
                {
                    out.push_str(&format!(
                        "<text id='shortcut-conflict-{id}' class='conflict' text='Also {}' />",
                        escape(other.title)
                    ));
                }
                if self.recording.as_deref() == Some(spec.id) {
                    out.push_str(&format!("<text id='shortcut-recording-{id}' class='accent' text='Press keys… (Esc cancels)' />"));
                } else if chords.is_empty() {
                    out.push_str(&format!(
                        "<text id='shortcut-none-{id}' class='caption' text='—' />"
                    ));
                } else {
                    out.push_str(&format!("<panel id='shortcut-keys-{id}' class='keys'>"));
                    for (n, cap) in chords[0].caps().iter().enumerate() {
                        out.push_str(&format!("<panel id='shortcut-key-{id}-{n}' class='keycap'><text id='shortcut-key-text-{id}-{n}' text='{}' /></panel>",escape(cap)));
                    }
                    out.push_str("</panel>");
                }
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("rebind-{id}")),
                    Binding::ShortcutRecord(spec.id.to_owned()),
                );
                out.push_str(&format!("<button id='rebind-{id}' class='ghost icon-button' label='Rebind {}' description='Press a new shortcut' action='Dispatch'>{}</button>",escape(spec.title),icons::icon(&format!("rebind-{id}"),"pencil")));
                if overrides.is_some_and(|map| map.contains_key(spec.id)) {
                    self.bindings.insert(
                        ara_ui::source::stable_id(&format!("unbind-{id}")),
                        Binding::ShortcutReset(spec.id.to_owned()),
                    );
                    out.push_str(&format!("<button id='unbind-{id}' class='ghost icon-button' label='Reset {}' description='Back to the preset binding' action='Dispatch'>{}</button>",escape(spec.title),icons::icon(&format!("unbind-{id}"),"undo-2")));
                }
                out.push_str("</panel>");
            }
        }
        if shown == 0 {
            self.empty(
                out,
                "shortcuts-none",
                "keyboard",
                "No matching shortcuts",
                "Search by name, or Record keys to find a binding.",
                &[],
            );
        }
    }
}
fn shortcut_row_id(command: &str, builtin: bool) -> String {
    if builtin {
        command.replace('.', "-")
    } else {
        format!("extension-{:x}", ara_ui::source::stable_id(command))
    }
}

#[cfg(test)]
mod tests;

fn default_of(d: &Def) -> Value {
    match d.default {
        crate::settings::Lit::B(v) => json!(v),
        crate::settings::Lit::S(v) => json!(v),
        crate::settings::Lit::N(v) => json!(v),
        crate::settings::Lit::Empty => json!({}),
        crate::settings::Lit::None => json!([]),
    }
}
