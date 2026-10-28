//! Hierarchy actions share the drag framework's root and world-pose validation.
mod model;
pub(super) use model::{preserved_transform, selection_roots};

use super::{Shell, commands::Target};
use crate::Editor;
use ara_doc::prefab::Resolved;
use serde_json::json;

impl Shell {
    /// Group the clicked row, or the complete selection when that row is selected.
    pub(crate) fn group_selection(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let selected = target.entities(&editor.state);
        let resolved = self.resolved(editor)?;
        group(editor, &resolved, &selected)?;
        Ok(())
    }

    /// Hide/show exactly the target set, optionally including all its descendants.
    /// A missing value toggles the complete set on unless every target is already on.
    pub(crate) fn set_hierarchy_hidden(
        &mut self,
        editor: &mut Editor,
        target: &Target,
        descendants: bool,
        enabled: Option<bool>,
    ) -> ara_ui::Result<()> {
        let resolved = self.resolved(editor)?;
        set_view_flag(editor, &resolved, target, descendants, enabled, false)
    }

    /// Lock/unlock exactly the target set, optionally including all its descendants.
    pub(crate) fn set_hierarchy_locked(
        &mut self,
        editor: &mut Editor,
        target: &Target,
        descendants: bool,
        enabled: Option<bool>,
    ) -> ara_ui::Result<()> {
        let resolved = self.resolved(editor)?;
        set_view_flag(editor, &resolved, target, descendants, enabled, true)
    }
}

fn group(editor: &mut Editor, resolved: &Resolved, selected: &[String]) -> ara_ui::Result<String> {
    let revision = editor.host.store().revision();
    let commands = model::group_commands(resolved, selected, &editor.state.locked)?;
    let result = editor.apply(commands, "Group selection", Some(revision))?;
    let id = result["outputs"][0]["id"]
        .as_str()
        .ok_or("The group was created but its entity ID was not returned")?
        .to_owned();
    editor.action("editor.select", json!({"entities":[id]}))?;
    editor.action("editor.collapse", json!({"entity":id,"enabled":false}))?;
    Ok(id)
}

fn set_view_flag(
    editor: &mut Editor,
    resolved: &Resolved,
    target: &Target,
    descendants: bool,
    enabled: Option<bool>,
    lock: bool,
) -> ara_ui::Result<()> {
    // Validate the whole set before the first navigation action. A deleted row or
    // malformed subtree cannot leave only part of a requested set hidden/locked.
    let entities = model::view_targets(resolved, &target.entities(&editor.state), descendants)?;
    let current = if lock {
        &editor.state.locked
    } else {
        &editor.state.hidden
    };
    let enabled = enabled.unwrap_or_else(|| !entities.iter().all(|id| current.contains(id)));
    let method = if lock {
        "editor.lock"
    } else {
        "editor.visibility"
    };
    for entity in entities {
        editor.action(method, json!({"entity":entity,"enabled":enabled}))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
