//! Synthetic bus records exercise real Console/Problems without damaging project sources.
use crate::Editor;
use ara_diag::{Diagnostic, FixAction, codes};
use serde_json::{Value, json};

pub(super) const PREFIX: &str = "J7 ";
pub(super) const FILE_A: &str = "Assets/Gameplay/J7SourceA.cs";
pub(super) const FILE_B: &str = "Assets/Gameplay/J7SourceB.cs";
pub(super) const SAME: &str = "J7 same failure";
pub(super) const REPAIR: &str = "J7 pinned engine installation needs repair";
pub(super) const ENGINE: &str = "preview4 fixture's engine";

fn diagnostic(message: &str, file: &str, line: u32, severity: ara_diag::Severity) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(codes::INVALID_PARAMS, message);
    diagnostic.severity = severity;
    diagnostic.location.file = Some(file.into());
    diagnostic.location.line = Some(line);
    diagnostic.location.column = Some(4);
    diagnostic
}

pub(super) fn seed(editor: &mut Editor) -> ara_ui::Result<()> {
    use ara_diag::Severity::{Error, Info, Warning};
    let repeat = diagnostic(SAME, FILE_A, 12, Error);
    let repair = diagnostic(REPAIR, FILE_B, 20, Warning).with_fix(
        "Review engine repair command",
        FixAction::Cli {
            args: vec!["hub".into(), "repair".into(), ENGINE.into()],
        },
    );
    let diagnostics = [
        repeat.clone(),
        repeat.clone(),
        repeat,
        diagnostic(SAME, FILE_B, 12, Error),
        diagnostic(SAME, FILE_A, 13, Error),
        diagnostic("J7 different failure", FILE_A, 12, Error),
        diagnostic("J7 warning only", FILE_A, 14, Warning),
        diagnostic("J7 info only", FILE_A, 15, Info),
        repair,
    ];
    let reply = editor.action(
        "diag.report",
        json!({"engine":true,"diagnostics":diagnostics}),
    )?;
    if reply["recorded"] != 9
        || reply["skipped"]
            .as_array()
            .is_none_or(|items| !items.is_empty())
    {
        return Err(format!("J7 diagnostics fixture was rejected: {reply}").into());
    }
    Ok(())
}

pub(super) fn after_clear(editor: &mut Editor) -> ara_ui::Result<()> {
    editor.action("diag.report",json!({"engine":true,"diagnostics":[Diagnostic::error(codes::INVALID_PARAMS,"J7 message after clear")]}))?;
    Ok(())
}

pub(super) fn records(editor: &mut Editor) -> ara_ui::Result<Vec<Value>> {
    let logs = editor.action("logs", json!({"group":false,"severity":"info","limit":500}))?;
    Ok(logs["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry["diagnostic"]["message"]
                .as_str()
                .is_some_and(|text| text.starts_with(PREFIX))
        })
        .cloned()
        .collect())
}

pub(super) fn seq(records: &[Value], message: &str, file: &str, line: u32) -> ara_ui::Result<u64> {
    records
        .iter()
        .find(|entry| {
            entry["diagnostic"]["message"] == message
                && entry["diagnostic"]["location"]["file"] == file
                && entry["diagnostic"]["location"]["line"] == line
        })
        .and_then(|entry| entry["seq"].as_u64())
        .ok_or_else(|| {
            format!("J7 diagnostic is not on the real bus: {message} ({file}:{line})").into()
        })
}

pub(super) fn command() -> &'static str {
    if cfg!(windows) {
        "ara hub repair 'preview4 fixture''s engine'"
    } else {
        "ara hub repair 'preview4 fixture'\\''s engine'"
    }
}
