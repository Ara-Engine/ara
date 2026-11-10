//! The editor's commands as the studio offers them: what each is called and does
//! ([`Command`], [`table::COMMANDS`]), what it acts on ([`Target`]), and where the menu
//! bar and context menus list it ([`menus`]). Menus only list commands in the catalogue,
//! and `studio::commands::run` runs them.
pub(crate) mod menus;
mod table;

use crate::state::EditorState;
use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_every_entry_is_described() {
        let mut seen = std::collections::BTreeSet::new();
        for c in all() {
            assert!(seen.insert(c.id), "{} twice", c.id);
            assert!(!c.title.is_empty() && !c.summary.is_empty(), "{}", c.id);
        }
    }
}
