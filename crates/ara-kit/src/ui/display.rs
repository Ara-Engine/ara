//! What shows rather than acts: keycaps, chips, cards, dots, progress, spinners, dividers,
//! section headers, empty states and monogram avatars.
use std::time::Duration;

use stillkit::el::{Align, El, Justify, arc, col, el, icon, row, text, token};
use stillkit::icons::Icon;
use stillkit::{Rgba, TextRole};

use super::{Size, Tone, button, mono};
use crate::look::Look;

/// A keyboard key or chord as a small keycap: `kbd(l, "⌘K")`.
pub fn kbd<S: 'static>(l: &Look, keys: &str) -> El<S> {
    let c = &l.c;
    row()
        .h(18.0)
        .pad_x(5.0)
        .align(Align::Center)
        .radius(l.radius.xs)
        .fill(c.keycap.0)
        .border(1.0, c.border.0)
        .child(
            text(keys)
                .text_size(10.5)
                .family(mono())
                .color(c.muted_foreground.0),
        )
}

/// A status's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Grey.
    Neutral,
    /// The accent.
    Accent,
    /// Green.
    Success,
    /// Amber.
    Warning,
    /// Red.
    Danger,
    /// Blue information.
    Info,
}

/// (fill, ink, solid) of a status.
pub fn status_paints(l: &Look, s: Status) -> (Rgba, Rgba, Rgba) {
    let c = &l.c;
    match s {
        Status::Neutral => (c.raised.0, c.muted_foreground.0, c.muted_foreground.0),
        Status::Accent => (
            c.primary.0.with_alpha(0.16),
            c.info_foreground.0,
            c.primary.0,
        ),
        Status::Success => (
            c.success.0.with_alpha(0.14),
            c.success_foreground.0,
            c.success.0,
        ),
        Status::Warning => (c.warning_surface.0, c.warning_foreground.0, c.warning.0),
        Status::Danger => (
            c.destructive_surface.0,
            c.destructive_foreground.0,
            c.destructive.0,
        ),
        Status::Info => (c.info.0.with_alpha(0.14), c.info_foreground.0, c.info.0),
    }
}

/// A small pill of text (a version, a count, a state), optionally led by a dot.
pub fn chip<S: 'static>(l: &Look, status: Status, label: &str, dot: bool) -> El<S> {
    let (fill, ink, solid) = status_paints(l, status);
    let mut c = row()
        .h(20.0)
        .pad_x(7.0)
        .gap(5.0)
        .align(Align::Center)
        .radius(10.0)
        .fill(fill)
        .color(ink)
        .cascade_ink();
    if dot {
        c = c.child(el().size(6.0).radius(3.0).fill(solid));
    }
    c.child(text(label).text_size(11.0).weight(520))
}

/// A count badge (sidebar items, tabs): a compact pill.
pub fn count<S: 'static>(l: &Look, n: usize, strong: bool) -> El<S> {
    let c = &l.c;
    let (fill, ink) = if strong {
        (c.primary.0, c.primary_foreground.0)
    } else {
        (c.raised.0, c.muted_foreground.0)
    };
    row()
        .h(18.0)
        .min_w(18.0)
        .pad_x(5.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(9.0)
        .fill(fill)
        .child(text(n.to_string()).text_size(10.5).weight(600).color(ink))
}

/// A status dot (connection, a running process), 8 px.
pub fn dot<S: 'static>(l: &Look, status: Status) -> El<S> {
    el().size(8.0).radius(4.0).fill(status_paints(l, status).2)
}

/// A card: the panel fill, a hairline, Ara's base radius.
pub fn card<S: 'static>(l: &Look) -> El<S> {
    col()
        .fill(l.c.card.0)
        .border(1.0, l.c.border.0)
        .radius(l.radius.base)
}

/// A card that lifts while hovered (project tiles, templates): its border brightens and it
/// casts the card shadow. Make it interactive (`on_press`) for the hover to apply.
pub fn hover_card<S: 'static>(l: &Look) -> El<S> {
    let lift = l.c.separator_strong.0;
    card(l).motion(token::MOTION_INTERACTIVE).hovered(move |e| {
        e.border(1.0, lift)
            .shadow_level(stillkit::ShadowLevel::Raised)
    })
}

/// A horizontal hairline.
pub fn divider<S: 'static>(l: &Look) -> El<S> {
    el().h(1.0).w_full().fill(l.c.border.0)
}

/// A vertical hairline `h` tall (between toolbar groups).
pub fn vdivider<S: 'static>(l: &Look, h: f32) -> El<S> {
    el().w(1.0).h(h).fill(l.c.border.0)
}

/// A determinate progress bar (`fraction` 0..=1) whose fill springs to its value.
pub fn progress<S: 'static>(l: &Look, fraction: f32, status: Status) -> El<S> {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let solid = status_paints(l, status).2;
    el().h(4.0).w_full().radius(2.0).fill(l.c.raised.0).child(
        row()
            .h_full()
            .w_full()
            .child(
                el().h_full()
                    .grow(fraction.max(0.0001))
                    .radius(2.0)
                    .fill(solid)
                    .animate(),
            )
            .child(el().grow((1.0 - fraction).max(0.0001))),
    )
}

/// A spinner `size` px across: a quarter arc turning once a second while visible.
pub fn spinner<S: 'static>(l: &Look, size: f32) -> El<S> {
    arc(0.28)
        .size(size)
        .stroke(2.0)
        .color(l.c.muted_foreground.0)
        .track(l.c.raised.0)
        .spin(Duration::from_millis(900))
}

/// A section header in a page or panel: the title, an optional detail, actions at the end.
pub fn section<S: 'static>(
    l: &Look,
    title: &str,
    detail: Option<&str>,
    actions: Vec<El<S>>,
) -> El<S> {
    let c = &l.c;
    let mut head = row()
        .gap(8.0)
        .align(Align::Center)
        .child(text(title).font(TextRole::Title3).color(c.foreground.0));
    if let Some(d) = detail {
        head = head.child(text(d).text_size(12.0).color(c.muted_foreground.0));
    }
    head.child(el().grow(1.0)).children(actions)
}

/// An empty state: an icon in a soft disc, a title, a line of guidance and an action.
pub fn empty<S: 'static>(
    l: &Look,
    glyph: Icon,
    title: &str,
    body: &str,
    action: Option<El<S>>,
) -> El<S> {
    let c = &l.c;
    let mut e = col()
        .gap(10.0)
        .align(Align::Center)
        .pad(32.0)
        .child(
            el().size(44.0)
                .radius(22.0)
                .fill(c.raised.0)
                .border(1.0, c.border.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .child(icon(glyph).size(20.0).color(c.muted_foreground.0)),
        )
        .child(text(title).font(TextRole::Headline).color(c.foreground.0))
        .child(
            text(body)
                .text_size(12.0)
                .wrap(3)
                .max_w(360.0)
                .text_align(stillkit::el::TextAlign::Center)
                .color(c.muted_foreground.0),
        );
    if let Some(a) = action {
        e = e.child(el().h(4.0)).child(a);
    }
    e
}

/// Initials for a name: the first letters of its first two words ("Night Market" -> "NM").
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|w| !w.is_empty())
        .collect();
    let pick = |w: &str| {
        w.chars()
            .next()
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_default()
    };
    match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect::<String>().to_uppercase(),
        [a, b, ..] => pick(a) + &pick(b),
    }
}

/// A rounded square monogram for a project or a person, tinted by `seed`'s chart colour.
pub fn avatar<S: 'static>(l: &Look, name: &str, seed: u64, size: f32) -> El<S> {
    let tint = chart(l, seed);
    el().size(size)
        .radius((size * 0.28).round())
        .fill(tint.with_alpha(0.16))
        .border(1.0, tint.with_alpha(0.28))
        .align(Align::Center)
        .justify(Justify::Center)
        .child(
            text(initials(name))
                .text_size((size * 0.38).round())
                .weight(650)
                .color(tint),
        )
}

/// One of the six chart colours, picked by `seed`.
pub fn chart(l: &Look, seed: u64) -> Rgba {
    let c = &l.c;
    [
        c.chart_1, c.chart_2, c.chart_3, c.chart_4, c.chart_5, c.chart_6,
    ][(seed % 6) as usize]
        .0
}

/// A small heading over a group of fields or rows.
pub fn label<S: 'static>(l: &Look, s: &str) -> El<S> {
    text(s)
        .text_size(11.0)
        .weight(600)
        .color(l.c.muted_foreground.0)
}

/// A link-styled action: accent ink, underline-free, brightening on hover.
pub fn link<S: 'static>(l: &Look, s: &str) -> El<S> {
    let hover = l.c.foreground.0;
    row()
        .color(l.c.info_foreground.0)
        .hovered(move |e| e.color(hover))
        .cascade_ink()
        .cursor(stillkit::Cursor::Hand)
        .child(text(s).text_size(12.0))
}

/// A banner across the top of a page: icon, message, and actions, in a status tint.
pub fn banner<S: 'static>(
    l: &Look,
    status: Status,
    glyph: Icon,
    message: &str,
    actions: Vec<El<S>>,
) -> El<S> {
    let (fill, ink, solid) = status_paints(l, status);
    row()
        .pad_x(12.0)
        .pad_y(8.0)
        .gap(10.0)
        .align(Align::Center)
        .radius(l.radius.md)
        .fill(fill)
        .border(1.0, solid.with_alpha(0.3))
        .child(icon(glyph).size(15.0).color(ink))
        .child(
            text(message)
                .text_size(12.0)
                .wrap(2)
                .grow(1.0)
                .color(l.c.foreground.0),
        )
        .children(actions)
}

/// The "nothing to show yet" button used by empty states.
pub fn empty_action<S: 'static>(l: &Look, glyph: Icon, label: &str) -> El<S> {
    button(l, Tone::Primary, Size::Md, Some(glyph), label)
}

/// A chip that sits on a picture (a cover, a thumbnail): a dark translucent pill with white
/// text and a coloured dot, legible over any image.
pub fn photo_chip<S: 'static>(l: &Look, status: Status, label: &str) -> El<S> {
    let solid = status_paints(l, status).2;
    row()
        .h(22.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(11.0)
        .fill(Rgba::new(0.0, 0.0, 0.0, 0.62))
        .border(1.0, Rgba::new(1.0, 1.0, 1.0, 0.12))
        .child(el().size(6.0).radius(3.0).fill(solid))
        .child(
            text(label)
                .text_size(11.0)
                .weight(560)
                .color(Rgba::new(1.0, 1.0, 1.0, 0.95)),
        )
}
