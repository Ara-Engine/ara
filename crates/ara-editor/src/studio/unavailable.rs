//! The quiet stand-in for what the renderer does not provide yet (view modes, rendering debug
//! switches, asset previews, the Scene view itself): a muted tile with an icon and one line, in
//! the space the thing would fill. Never a blank and never a red error: nothing is broken, it
//! is not there yet.
//!
//! - A panel, page or preview area that has nothing to draw: [`tile`].
//! - A menu entry or button with nothing behind it: keep the entry, `enabled(item, false)`, and
//!   give it the tooltip [`SOON`], so the menu keeps its shape.
use ara_kit::icons;
use ara_kit::look::Look;
use stillkit::el::{Align, El, Justify, col, el, icon, text};

use super::Studio;

/// The one line a disabled entry's tooltip says.
#[allow(dead_code, reason = "first user: the Scene view's placeholders")]
pub(super) const SOON: &str = "Not available yet";

/// A muted tile filling its parent: an outlined square and `line` (one sentence, what is not
/// available; [`SOON`] when nothing more is useful). `key` names it for tests and captures.
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
