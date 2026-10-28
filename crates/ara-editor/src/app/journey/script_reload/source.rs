//! Source edits use the attributed preparation/job/typed-op pipeline exclusively.
use crate::Editor;
use serde_json::{Value, json};

pub(super) fn replace(
    editor: &mut Editor,
    doc: &str,
    path: &str,
    before: &str,
    after: &str,
    reason: &str,
) -> ara_ui::Result<(String, Value)> {
    let response = editor.action("cmd.apply", json!({"doc":doc,"why":reason,"commands":[{
        "cmd":"source.replace","args":{"path":path,"baseHash":ara_doc::file_state::FileState::bytes(before.as_bytes()).hash,"text":after}
    }]}))?;
    let job = response["job"]
        .as_str()
        .ok_or("source.replace returned no preparation/commit job")?
        .to_owned();
    Ok((
        job.clone(),
        json!({"job":job,"path":path,"command":"source.replace","why":reason,
        "beforeHash":ara_doc::file_state::FileState::bytes(before.as_bytes()).hash,
        "afterHash":ara_doc::file_state::FileState::bytes(after.as_bytes()).hash,"ingress":"typed command, not native source editing"}),
    ))
}
