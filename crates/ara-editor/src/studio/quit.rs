//! Closing the editor: File › Exit and the window's close button ask first when work could be
//! lost (documents still being written, a document that diverged on disk, a number being
//! dragged, an Inspector value not yet committed). A tool's `daemon.shutdown` closes without
//! asking (`closing` in `mod.rs`): nobody is there to answer.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::json;
use stillkit::el::{Overlay, text};

use super::Studio;

/// What a close would lose or cut short, one line each; empty when nothing is at risk.
fn at_risk(s: &mut Studio) -> Vec<&'static str> {
    let mut why = Vec::new();
    let files = s
        .editor
        .action("session.status", json!({}))
        .map(|status| status["files"].clone())
        .unwrap_or_default();
    let listed = |key: &str| {
        files[key].as_array().is_some_and(|v| !v.is_empty())
            || files[key].as_object().is_some_and(|v| !v.is_empty())
    };
    if s.editor.host.unsaved() || listed("unsaved") {
        why.push("Some documents are still being written.");
    }
    if listed("diverged") {
        why.push("A document changed on disk and no longer matches the editor's copy.");
    }
    if s.local.scrub.is_some() || s.local.draft.is_some() {
        why.push("A value is being edited and has not been committed.");
    }
    why
}

/// File › Exit and the window's close button: closes at once when nothing is at risk, otherwise
/// opens the question (a second request while it is open leaves it as it is).
pub(super) fn request(s: &mut Studio) {
    if s.local.quit_ask.is_some() {
        return;
    }
    let why = at_risk(s);
    if why.is_empty() {
        s.local.quit = true;
    } else {
        s.local.menu = None;
        s.local.quit_ask = Some(why);
    }
}

/// Save and close: commits a value being dragged, writes the documents, then closes.
fn save_and_close(s: &mut Studio) {
    if let Some((path, axis, _, now)) = s.local.scrub.take() {
        super::inspector::commit_number(s, &path, axis, now);
    }
    s.local.draft = None;
    s.editor.host.flush();
    s.local.quit_ask = None;
    s.local.quit = true;
}

/// Close without committing the value being edited. Documents already changed are still written
/// as the editor closes.
fn discard_and_close(s: &mut Studio) {
    s.local.scrub = None;
    s.local.draft = None;
    s.local.quit_ask = None;
    s.local.quit = true;
}

/// The question, over everything but the import card.
pub(super) fn overlay(s: &Studio) -> Option<Overlay<Studio>> {
    let why = s.local.quit_ask.clone()?;
    let l = &s.look;
    let c = &l.c;
    let cancel = |s: &mut Studio| s.local.quit_ask = None;
    let body = stillkit::el::col()
        .gap(6.0)
        .children(why.iter().map(|line| {
            text(*line)
                .text_size(12.0)
                .color(c.muted_foreground.0)
                .wrap(3)
        }));
    let panel = dialog_panel(
        l,
        Some((Status::Warning, icons::TRIANGLE_ALERT)),
        "Close the editor?",
        body,
        vec![
            button(l, Tone::Secondary, Size::Md, None, "Cancel")
                .key("quit/cancel")
                .on_press(cancel),
            button(l, Tone::Ghost, Size::Md, None, "Discard and close")
                .key("quit/discard")
                .on_press(discard_and_close),
            button(l, Tone::Primary, Size::Md, None, "Save and close")
                .key("quit/save")
                .on_press(save_and_close),
        ],
        cancel,
    )
    .w(400.0);
    Some(stillkit::el::dialog(panel).on_dismiss(cancel))
}
