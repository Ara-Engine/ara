//! The quiet stand-in for what the renderer does not provide yet (asset previews, the Game
//! view outside Play): a muted tile with an icon and one line, in the space the thing would
//! fill. Never a blank and never a red error: nothing is broken, it
//! is not there yet.
//!
//! - A panel, page or preview area that has nothing to draw: [`tile`].
//! - A control with nothing behind it does not belong here: remove it (and add a row to the
//!   gap table in `crates/ara-renderlab/AGENTS.md`), never disable and keep it.
use ara_kit::icons;
use ara_kit::look::Look;
use stillkit::el::{Align, El, Justify, col, el, icon, row, text};

use super::Studio;

/// A muted tile filling its parent: an outlined square and `line` (one sentence, what is not
/// available). `key` names it for tests and captures.
#[allow(dead_code, reason = "first user: the Scene view's placeholders")]
pub(super) fn tile(l: &Look, key: &str, line: &str) -> El<Studio> {
    let c = &l.c;
    el().key(key.to_owned())
        .grow(1.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .child(
            col()
                .gap(8.0)
                .align(Align::Center)
                .color(c.muted_foreground.0)
                .cascade_ink()
                .child(icon(icons::SQUARE_DASHED).size(22.0))
                .child(text(line.to_owned()).text_size(12.0).wrap(3)),
        )
}

/// One muted line with the outlined square, for a panel that has room for a sentence but not a
/// tile (a page's side panel, a strip): what is not available, in `line`.
pub(super) fn note(l: &Look, line: &str) -> El<Studio> {
    row()
        .pad(10.0)
        .gap(8.0)
        .align(Align::Center)
        .color(l.c.muted_foreground.0)
        .cascade_ink()
        .child(icon(icons::SQUARE_DASHED).size(14.0))
        .child(text(line.to_owned()).text_size(11.5).wrap(3).grow(1.0))
}

/// Whether the Game view can show the scene through its gameplay camera outside Play (a second
/// camera on the Scene's renderer). Play itself runs in the player process and is unaffected.
pub(super) fn game_preview_available() -> bool {
    false
}
