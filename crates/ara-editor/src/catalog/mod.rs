//! The editor's commands as the studio offers them: what each is called and does
//! ([`Command`], [`table::COMMANDS`]), what it acts on ([`Target`]), and where the menu
//! bar and context menus list it ([`menus`]). Menus only list commands in the catalogue,
//! and `studio::commands::run` runs them.
pub(crate) mod menus;
mod table;

use crate::extensions::Catalog;
use crate::keys::{Keymap, Preset};
use crate::state::EditorState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One command: its stable id, title, Lucide icon name, default shortcut list
/// (`Ctrl+Y / Ctrl+Shift+Z`, or empty) and one sentence for the palette, tooltips and
/// MCP.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    pub keys: &'static str,
    pub summary: &'static str,
}

/// What a command acts on: the right-clicked row, field, tab or nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    /// The current selection or focused panel.
    #[default]
    None,
    /// A scene entity address.
    Entity(String),
    /// A project-relative asset path.
    Asset(String),
    /// A project-relative folder.
    Folder(String),
    /// An inspector field path (`Transform.position`).
    Field(String),
    /// An inspector component name.
    Component(String),
    /// A dock panel id.
    Panel(String),
    /// A graph node id in the active resource.
    Node(String),
    /// A scene document id (Open Recent).
    Doc(String),
    /// A floating window id.
    Window(String),
    /// Plain text (console rows, profiler spans).
    Text(String),
}
impl Target {
    /// The entity this command applies to: the target, else the primary selection.
    pub fn entity<'a>(&'a self, state: &'a EditorState) -> Option<&'a str> {
        match self {
            Self::Entity(e) => Some(e),
            _ => state.selection.first().map(String::as_str),
        }
    }
    /// Entities this command applies to: the target if it is outside the selection.
    pub fn entities(&self, state: &EditorState) -> Vec<String> {
        match self {
            Self::Entity(e) if !state.selection.contains(e) => vec![e.clone()],
            _ => state.selection.clone(),
        }
    }
}

/// Every command, in table order.
pub(crate) fn all() -> &'static [Command] {
    table::COMMANDS
}

/// The command `id`, if the editor has it.
pub(crate) fn get(id: &str) -> Option<&'static Command> {
    table::COMMANDS.iter().find(|c| c.id == id)
}

/// Why `id` cannot run yet, when it cannot: its feature is not built (the summary says
/// what is missing), or only the legacy editor runs it so far.
pub(crate) fn later(id: &str) -> Option<&'static str> {
    if table::UNBUILT.contains(&id) {
        return get(id).map(|c| c.summary);
    }
    table::UNPORTED
        .contains(&id)
        .then_some("Not in the studio yet; the legacy editor (--legacy-ui) runs it")
}

/// The bindings in force: the catalogue's chords, under the `input.keymap` preset and
/// the user's `input.shortcuts` rebinds; C# extension commands bind only by rebinding.
pub(crate) fn keymap(state: &EditorState, extensions: &Catalog) -> Keymap {
    let preset = Preset::parse(
        state
            .settings
            .get("input.keymap")
            .and_then(Value::as_str)
            .unwrap_or("ara"),
    );
    let overrides = state
        .settings
        .get("input.shortcuts")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let builtin = all().iter().map(|c| (c.id, c.keys));
    let extension = extensions.commands.iter().map(|c| (c.id.as_str(), ""));
    Keymap::new(builtin.chain(extension), preset, &overrides)
}

/// Where the menu bar lists `id`, as `Edit › Snapping`, or empty; a C# extension's
/// registered menu wins for its own commands.
pub(crate) fn menu_path(extensions: &Catalog, id: &str) -> String {
    fn walk(items: &[menus::Item], id: &str, path: &mut Vec<&'static str>) -> bool {
        for item in items {
            match item {
                menus::Item::Command(c) if *c == id => return true,
                menus::Item::Dynamic(d) if d.command() == id => return true,
                menus::Item::Sub(title, children) => {
                    path.push(title);
                    if walk(children, id, path) {
                        return true;
                    }
                    path.pop();
                }
                _ => {}
            }
        }
        false
    }
    if let Some(menu) = extensions
        .menus
        .iter()
        .filter(|m| m.command == id)
        .min_by(|a, b| a.path.cmp(&b.path))
    {
        return menu
            .path
            .rsplit_once('/')
            .map_or(menu.path.as_str(), |(p, _)| p)
            .replace('/', " › ");
    }
    for menu in menus::MENUS {
        let mut path = vec![menu.title];
        if walk(menu.items, id, &mut path) {
            return path.join(" › ");
        }
    }
    String::new()
}

/// The catalogue as agents see it (`editor.commands`): each command's id, title, menu
/// path, summary and bindings in force; C# commands say when they are disabled and why.
pub(crate) fn export(state: &EditorState, extensions: &Catalog) -> Value {
    let keymap = keymap(state, extensions);
    let row = |id: &str, title: &str, summary: &str, enabled: bool, reason: Option<&str>| {
        json!({
            "id": id,
            "title": title,
            "menu": menu_path(extensions, id),
            "summary": summary,
            "shortcuts": keymap.chords(id).iter().map(|k| k.text()).collect::<Vec<_>>(),
            "enabled": enabled,
            "disabledReason": reason,
        })
    };
    let builtin = all().iter().map(|c| {
        let reason = later(c.id);
        row(c.id, c.title, c.summary, reason.is_none(), reason)
    });
    let extension = extensions.commands.iter().map(|c| {
        let reason = (!c.enabled).then(|| {
            c.disabled_reason
                .as_deref()
                .unwrap_or("C# editor command is disabled until reload")
        });
        row(&c.id, &c.title, &c.summary, c.enabled, reason)
    });
    Value::Array(builtin.chain(extension).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_commands_are_in_the_catalogue_once() {
        for id in table::UNBUILT.iter().chain(table::UNPORTED) {
            assert!(get(id).is_some(), "{id} has no entry");
        }
        let both: Vec<_> = table::UNBUILT
            .iter()
            .filter(|id| table::UNPORTED.contains(id))
            .collect();
        assert!(both.is_empty(), "{both:?}");
    }

    #[test]
    fn ids_are_unique_and_every_entry_is_described() {
        let mut seen = std::collections::BTreeSet::new();
        for c in all() {
            assert!(seen.insert(c.id), "{} twice", c.id);
            assert!(!c.title.is_empty() && !c.summary.is_empty(), "{}", c.id);
        }
    }
}
