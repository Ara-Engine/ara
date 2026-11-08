//! Buttons: a label with an optional leading icon, in four tones and three sizes, and square
//! icon buttons. Callers add the key, handler and tooltip:
//! `button(l, Tone::Primary, Size::Md, Some(icons::PLUS), "New project").key("new").on_press(..)`.
use stillkit::el::{Align, El, Justify, icon, row, text, token};
use stillkit::icons::Icon;

use super::mix;
use crate::look::Look;

/// How loud a button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The page's one main action: the accent fill.
    Primary,
    /// Other actions: a raised fill with a hairline.
    Secondary,
    /// Quiet actions in toolbars and rows: no fill until hovered.
    Ghost,
    /// Destructive actions: an error tint.
    Danger,
}

/// A control's height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// 24 px: dense toolbars, rows, inspectors.
    Sm,
    /// 28 px: the default.
    Md,
    /// 36 px: page headers, dialogs, empty states.
    Lg,
}

impl Size {
    /// Height in design px.
    pub fn height(self) -> f32 {
        match self {
            Size::Sm => 24.0,
            Size::Md => 28.0,
            Size::Lg => 36.0,
        }
    }

    fn pad(self) -> f32 {
        match self {
            Size::Sm => 8.0,
            Size::Md => 10.0,
            Size::Lg => 14.0,
        }
    }

    fn icon(self) -> f32 {
        match self {
            Size::Sm => 14.0,
            Size::Md => 15.0,
            Size::Lg => 16.0,
        }
    }

    fn text(self) -> f32 {
        match self {
            Size::Lg => 13.0,
            _ => 12.0,
        }
    }
}

/// Fill, ink and border of a tone at rest, and its hovered ink and border.
struct Paints {
    fill: stillkit::Rgba,
    ink: stillkit::Rgba,
    border: Option<stillkit::Rgba>,
    hover_ink: stillkit::Rgba,
    hover_fill: Option<stillkit::Rgba>,
    hover_border: Option<stillkit::Rgba>,
}

fn paints(l: &Look, tone: Tone) -> Paints {
    let c = &l.c;
    match tone {
        Tone::Primary => Paints {
            fill: c.primary.0,
            ink: c.primary_foreground.0,
            border: None,
            hover_ink: c.primary_foreground.0,
            hover_fill: Some(mix(
                c.primary.0,
                stillkit::Rgba::new(1.0, 1.0, 1.0, 1.0),
                0.1,
            )),
            hover_border: None,
        },
        Tone::Secondary => Paints {
            fill: c.raised.0,
            ink: c.foreground.0,
            border: Some(c.border.0),
            hover_ink: c.foreground.0,
            hover_fill: Some(c.hover_surface.0),
            hover_border: Some(c.separator_strong.0),
        },
        Tone::Ghost => Paints {
            fill: stillkit::Rgba::TRANSPARENT,
            ink: c.muted_foreground.0,
            border: None,
            hover_ink: c.foreground.0,
            hover_fill: Some(c.hover_surface.0),
            hover_border: None,
        },
        Tone::Danger => Paints {
            fill: c.destructive_surface.0,
            ink: c.destructive_foreground.0,
            border: Some(c.destructive.0.with_alpha(0.25)),
            hover_ink: c.destructive_foreground.0,
            hover_fill: Some(mix(c.destructive_surface.0, c.destructive.0, 0.25)),
            hover_border: Some(c.destructive.0.with_alpha(0.5)),
        },
    }
}

/// A button: `label` with an optional leading `icon`, sized to its content.
pub fn button<S: 'static>(
    l: &Look,
    tone: Tone,
    size: Size,
    lead: Option<Icon>,
    label: &str,
) -> El<S> {
    let p = paints(l, tone);
    let mut b = row()
        .h(size.height())
        .pad_x(size.pad())
        .gap(6.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(l.radius.sm)
        .fill(p.fill)
        .color(p.ink)
        .cascade_ink()
        .label(label)
        .motion(token::MOTION_INTERACTIVE);
    if let Some(border) = p.border {
        b = b.border(1.0, border);
    }
    let hover = (p.hover_ink, p.hover_fill, p.hover_border);
    b = b.hovered(move |e| {
        let mut e = e.color(hover.0);
        if let Some(f) = hover.1 {
            e = e.fill(f);
        }
        if let Some(border) = hover.2 {
            e = e.border(1.0, border);
        }
        e
    });
    if let Some(i) = lead {
        b = b.child(icon(i).size(size.icon()));
    }
    b.child(
        text(label)
            .text_size(size.text())
            .weight(if tone == Tone::Primary { 560 } else { 500 }),
    )
}

/// A square icon-only button (`size` tall and wide). Give it a tooltip: it has no text.
pub fn icon_button<S: 'static>(l: &Look, tone: Tone, size: Size, i: Icon, label: &str) -> El<S> {
    let p = paints(l, tone);
    let mut b = row()
        .size(size.height())
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(l.radius.sm)
        .fill(p.fill)
        .color(p.ink)
        .cascade_ink()
        .label(label)
        .tooltip(label)
        .motion(token::MOTION_INTERACTIVE);
    if let Some(border) = p.border {
        b = b.border(1.0, border);
    }
    let hover = (p.hover_ink, p.hover_fill);
    b.hovered(move |e| {
        let e = e.color(hover.0);
        match hover.1 {
            Some(f) => e.fill(f),
            None => e,
        }
    })
    .child(icon(i).size(size.icon()))
}

/// Dims a control and makes it ignore input while `off`.
pub fn enabled<S>(el: El<S>, on: bool) -> El<S> {
    if on {
        el
    } else {
        el.disabled(true).opacity(0.45)
    }
}

/// A selectable icon button in a toolbar group (a view toggle, a tool): the accent ink and a
/// raised fill while `on`.
pub fn toggle_button<S: 'static>(l: &Look, size: Size, i: Icon, label: &str, on: bool) -> El<S> {
    let c = &l.c;
    let b = icon_button(l, Tone::Ghost, size, i, label).checked(on);
    if on {
        b.fill(c.raised.0).color(c.foreground.0).hovered(|e| e)
    } else {
        b
    }
}
