//! Hierarchy edits that need no window: grouping and the pure planning behind it and
//! behind drag reparenting. Both validate roots and world poses through `model`.
use crate::Editor;
use ara_doc::prefab::Resolved;
use serde_json::json;

pub(crate) mod model;
pub(crate) mod reparent;

/// Groups `selected` under a new parent in one transaction; the parent's id.
pub(crate) fn group(
    editor: &mut Editor,
    resolved: &Resolved,
    selected: &[String],
) -> ara_ui::Result<String> {
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
