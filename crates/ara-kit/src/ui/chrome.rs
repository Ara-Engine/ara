//! Window chrome shared by the Hub and the editor: the title bar, sidebar navigation items and
//! page headers.
use stillkit::el::{Align, El, Justify, col, el, icon, row, text};
use stillkit::icons::Icon;
use stillkit::{Role, TextRole};

use super::{Material, count};
use crate::icons;
use crate::look::Look;

/// Keys of the drawn window controls, for the host's native chrome parts
/// (`Surface::set_chrome`: Windows' snap layouts appear over the maximize button).
pub const MINIMIZE: &str = "window/minimize";
/// The maximize button's key.
pub const MAXIMIZE: &str = "window/maximize";
/// The close button's key.
pub const CLOSE: &str = "window/close";
/// The title bar's key: its free space drags the window.
pub const TITLE_BAR: &str = "window/title";

/// Title bar height in design px (the macOS compact caption's).
pub const TITLE_HEIGHT: f32 = 40.0;

/// A title bar: `lead` after the system window controls (`inset` design px, the macOS
/// traffic lights' right edge; 0 elsewhere), `middle` centred, `trail` at the end, then
/// drawn window controls where the system draws none (`controls`). Its free space is the
/// window's caption: the host marks it `Chrome::Caption`.
pub fn title_bar<S: 'static>(
    l: &Look,
    inset: f32,
    lead: Vec<El<S>>,
    middle: Vec<El<S>>,
    trail: Vec<El<S>>,
    controls: Option<bool>,
) -> El<S> {
    title_bar_ruled(l, inset, lead, middle, trail, controls, true)
}

/// [`title_bar`] without the hairline under it, for a window whose panels sit on the same
/// surface as the bar (the studio): the top reads as one piece.
pub fn title_bar_flush<S: 'static>(
    l: &Look,
    inset: f32,
    lead: Vec<El<S>>,
    middle: Vec<El<S>>,
    trail: Vec<El<S>>,
    controls: Option<bool>,
) -> El<S> {
    title_bar_ruled(l, inset, lead, middle, trail, controls, false)
}

fn title_bar_ruled<S: 'static>(
    l: &Look,
    inset: f32,
    lead: Vec<El<S>>,
    middle: Vec<El<S>>,
    trail: Vec<El<S>>,
    controls: Option<bool>,
    rule: bool,
) -> El<S> {
    let c = &l.c;
    let mut bar = row()
        .key(TITLE_BAR)
        .h(TITLE_HEIGHT)
        .w_full()
        .align(Align::Center)
        .fill(c.chrome.0)
        .child({
            let edge = if inset > 0.0 { inset + 8.0 } else { 10.0 };
            el().w(edge).min_w(edge)
        })
        .child(row().gap(4.0).align(Align::Center).children(lead))
        .child(
            row()
                .grow(1.0)
                .justify(Justify::Center)
                .align(Align::Center)
                .gap(8.0)
                .children(middle),
        )
        .child(
            // The trail (project name, search) shrinks when the window is narrow; the lead,
            // the middle and the window buttons never do.
            row()
                .min_w(0.0)
                .gap(4.0)
                .align(Align::Center)
                .children(trail)
                .child(el().w(8.0).min_w(8.0)),
        );
    if let Some(maximized) = controls {
        bar = bar.child(window_controls(l, maximized));
    }
    col()
        .w_full()
        .child(bar)
        .children(rule.then(|| el().h(1.0).w_full().fill(c.border.0)))
}

/// Minimize, maximize (restore when `maximized`) and close, Windows-sized (46 x 40); close
/// turns red on hover.
fn window_controls<S: 'static>(l: &Look, maximized: bool) -> El<S> {
    let c = &l.c;
    let hover = c.hover_surface.0;
    let ink = c.foreground.0;
    let part = |key: &str, glyph: Icon, label: &str, danger: bool| {
        let red = stillkit::Rgba::rgb8([196, 43, 28], 1.0);
        el().key(key)
            .w(46.0)
            .h(TITLE_HEIGHT)
            .align(Align::Center)
            .justify(Justify::Center)
            .label(label)
            .focusable()
            .color(c.muted_foreground.0)
            .cascade_ink()
            .hovered(move |e| {
                if danger {
                    e.fill(red).color(stillkit::Rgba::new(1.0, 1.0, 1.0, 1.0))
                } else {
                    e.fill(hover).color(ink)
                }
            })
            .child(icon(glyph).size(14.0))
    };
    row()
        .child(part(MINIMIZE, icons::MINUS, "Minimize", false))
        .child(part(
            MAXIMIZE,
            if maximized {
                icons::COPY
            } else {
                icons::SQUARE
            },
            if maximized { "Restore" } else { "Maximize" },
            false,
        ))
        .child(part(CLOSE, icons::X, "Close", true))
}

/// A sidebar destination: icon, label and an optional count; the current one is raised.
pub fn nav_item<S: 'static>(
    l: &Look,
    key: &str,
    glyph: Icon,
    label: &str,
    current: bool,
    badge: Option<(usize, bool)>,
) -> El<S> {
    let c = &l.c;
    let hover = c.foreground.0;
    let mut item: El<S> = row()
        .key(key)
        .h(32.0)
        .pad_x(10.0)
        .gap(10.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .role(Role::Tab)
        .label(label)
        .selected(current);
    if current {
        item = Material::Raised.paint(l, item);
    }
    item = item
        .color(if current {
            c.foreground.0
        } else {
            c.muted_foreground.0
        })
        .cascade_ink()
        .motion(stillkit::el::token::MOTION_INTERACTIVE)
        .hovered(move |e| e.color(hover))
        .child(icon(glyph).size(16.0))
        .child(
            text(label)
                .text_size(12.5)
                .weight(if current { 580 } else { 500 })
                .grow(1.0),
        );
    if let Some((n, strong)) = badge.filter(|(n, _)| *n > 0) {
        item = item.child(count(l, n, strong));
    }
    item
}

/// A page's header: a large title, a muted subtitle, and actions at the end.
pub fn page_header<S: 'static>(
    l: &Look,
    title: &str,
    subtitle: Option<&str>,
    actions: Vec<El<S>>,
) -> El<S> {
    let c = &l.c;
    let mut titles = col()
        .gap(4.0)
        .grow(1.0)
        .child(text(title).font(TextRole::Title1).color(c.foreground.0));
    if let Some(s) = subtitle {
        titles = titles.child(text(s).text_size(12.5).color(c.muted_foreground.0));
    }
    row()
        .gap(8.0)
        .align(Align::End)
        .child(titles)
        .children(actions)
}
