//! Buttons: a label with an optional leading icon, in four tones and three sizes, and square
//! icon buttons. Callers add the key, handler and tooltip:
//! `button(l, Tone::Primary, Size::Md, Some(icons::PLUS), "New project").key("new").on_press(..)`.
use stillkit::el::{Align, El, Justify, icon, row, text, token};
use stillkit::icons::Icon;

use super::{Material, mix};
use crate::look::Look;

/// How loud a button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The page's one main action: the Primary material.
    Primary,
    /// Other actions: the Raised material.
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

/// A tone's surface on `b`. Primary and Secondary are the Primary and Raised materials (Raised
/// brings its own hover step and press-in); Ghost and Danger are flat with a hover step.
fn surface<S: 'static>(l: &Look, tone: Tone, b: El<S>) -> El<S> {
    let c = &l.c;
    match tone {
        Tone::Primary => {
            let hover = mix(c.primary.0, stillkit::Rgba::new(1.0, 1.0, 1.0, 1.0), 0.1);
            Material::Primary
                .paint(l, b)
                .hovered(move |e| e.fill(hover))
        }
        Tone::Secondary => Material::Raised.paint(l, b).color(c.foreground.0),
        Tone::Ghost => {
            let (ink, fill) = (c.foreground.0, c.hover_surface.0);
            b.color(c.muted_foreground.0)
                .hovered(move |e| e.color(ink).fill(fill))
        }
        Tone::Danger => {
            let ink = c.destructive_foreground.0;
            let fill = mix(c.destructive_surface.0, c.destructive.0, 0.25);
            b.fill(c.destructive_surface.0)
                .color(ink)
                .hovered(move |e| e.fill(fill))
        }
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
    let b = row()
        .h(size.height())
        .pad_x(size.pad())
        .gap(6.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(l.radius.sm)
        .cascade_ink()
        .label(label)
        .motion(token::MOTION_INTERACTIVE);
    let mut b = surface(l, tone, b);
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
    let b = row()
        .size(size.height())
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(l.radius.sm)
        .cascade_ink()
        .label(label)
        .tooltip(label)
        .motion(token::MOTION_INTERACTIVE);
    surface(l, tone, b).child(icon(i).size(size.icon()))
}

/// Dims a control and makes it ignore input while `off`.
pub fn enabled<S>(el: El<S>, on: bool) -> El<S> {
    if on {
        el
    } else {
        el.disabled(true).opacity(0.45)
    }
}

/// A selectable icon button in a toolbar group (a view toggle, a tool): pressed in, in the
/// full ink, while `on`.
pub fn toggle_button<S: 'static>(l: &Look, size: Size, i: Icon, label: &str, on: bool) -> El<S> {
    let b = icon_button(l, Tone::Ghost, size, i, label).checked(on);
    if on {
        Material::Pressed
            .paint(l, b)
            .color(l.c.foreground.0)
            .hovered(|e| e)
    } else {
        b
    }
}
