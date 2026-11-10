//! Fields: one-line text input in a bordered well whose border lights up while it has focus,
//! a search field, a select trigger, switches, checkboxes and segmented controls.
use std::sync::Arc;

use stillkit::el::{Align, El, Justify, Key, col, el, icon, row, text, text_input, token};
use stillkit::icons::Icon;
use stillkit::{Cursor, Role};

use super::{Material, Size, kbd};
use crate::icons;
use crate::look::Look;

/// The well every input shares: the Well material, a step lighter under the pointer, and
/// the focus halo only while focus is inside it.
fn well<S: 'static>(l: &Look, size: Size) -> El<S> {
    let c = &l.c;
    let ring = c.focus_halo.0;
    let hover = l.well_hover();
    let row = row()
        .h(size.height())
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .border(1.0, stillkit::Rgba::TRANSPARENT);
    Material::Well
        .paint(l, row)
        .color(c.muted_foreground.0)
        .cascade_ink()
        .motion(token::MOTION_INTERACTIVE)
        .hovered(move |e| e.fill(hover))
        .focus_within(move |e| e.border(1.0, ring))
}

/// A text field: `value` shown, `placeholder` when empty, optional leading icon. Give the
/// returned field's handlers through `input` (it receives the inner `text_input`):
/// `field(l, Size::Md, "name", Some(icons::USER), "Name", &s.name, |f| f.on_input(..))`.
pub fn field<S: 'static>(
    l: &Look,
    size: Size,
    key: impl Into<Key>,
    lead: Option<Icon>,
    placeholder: &str,
    value: &str,
    input: impl FnOnce(El<S>) -> El<S>,
) -> El<S> {
    let mut w = well(l, size);
    if let Some(i) = lead {
        w = w.child(icon(i).size(14.0));
    }
    let f = text_input(key)
        .grow(1.0)
        .placeholder(placeholder)
        .value(value)
        .text_size(if size == Size::Lg { 13.0 } else { 12.0 })
        .color(l.c.foreground.0)
        // The well's border shows focus; the field inside draws no second ring.
        .focus_ring(false);
    w.child(input(f))
}

/// A search field: magnifier, the query, a clear button while it has text, and a keycap
/// hint (`/`) while empty. `on_clear` runs from the clear button.
pub fn search<S: 'static>(
    l: &Look,
    key: &str,
    placeholder: &str,
    value: &str,
    hint: Option<&str>,
    input: impl FnOnce(El<S>) -> El<S>,
    on_clear: impl Fn(&mut S) + 'static,
) -> El<S> {
    let mut w = field(
        l,
        Size::Md,
        Arc::<str>::from(key),
        Some(icons::SEARCH),
        placeholder,
        value,
        input,
    );
    if !value.is_empty() {
        let c = &l.c;
        let hover = c.foreground.0;
        w = w.child(
            el().key(format!("{key}/clear"))
                .size(18.0)
                .radius(9.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .color(c.muted_foreground.0)
                .label("Clear search")
                .cascade_ink()
                .hovered(move |e| e.color(hover))
                .on_press(on_clear)
                .child(icon(icons::X).size(12.0)),
        );
    } else if let Some(h) = hint {
        w = w.child(kbd(l, h));
    }
    w
}

/// The trigger of a dropdown, a Raised pop-up button: the chosen value and a chevron,
/// opening `open` on press (pair it with an overlay menu anchored to `key`).
pub fn select<S: 'static>(l: &Look, size: Size, key: impl Into<Key>, value: &str) -> El<S> {
    let c = &l.c;
    let row = row()
        .key(key)
        .h(size.height())
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(l.radius.sm);
    Material::Raised
        .paint(l, row)
        .color(c.foreground.0)
        .role(Role::ComboBox)
        .cascade_ink()
        .label(value)
        .motion(token::MOTION_INTERACTIVE)
        .child(text(value).grow(1.0).text_size(12.0))
        .child(
            icon(icons::CHEVRONS_UP_DOWN)
                .size(13.0)
                .color(c.muted_foreground.0),
        )
}

/// A switch: a 28x16 track (a Well, Primary while on) whose Raised knob springs across;
/// `on` is the model's value.
pub fn switch<S: 'static>(l: &Look, key: impl Into<Key>, label: &str, on: bool) -> El<S> {
    let c = &l.c;
    let track = el()
        .key(key)
        .w(28.0)
        .h(16.0)
        .radius(8.0)
        .role(Role::Switch)
        .label(label)
        .checked(on);
    let knob = el()
        .size(12.0)
        .radius(6.0)
        .absolute(if on { 14.0 } else { 2.0 }, 2.0);
    let knob = if on {
        knob.fill(c.primary_foreground.0)
    } else {
        Material::Raised.paint(l, knob).fill(c.muted_foreground.0)
    };
    (if on {
        Material::Primary
    } else {
        Material::Well
    })
    .paint(l, track)
    .motion(token::MOTION_INTERACTIVE)
    .cursor(Cursor::Hand)
    .child(knob.animate())
}

/// A checkbox: a 14 px Well, Primary with a check while `on`.
pub fn checkbox<S: 'static>(l: &Look, key: impl Into<Key>, label: &str, on: bool) -> El<S> {
    let c = &l.c;
    let b = el()
        .key(key)
        .size(14.0)
        .radius(l.radius.xs)
        .role(Role::CheckBox)
        .label(label)
        .checked(on)
        .align(Align::Center)
        .justify(Justify::Center)
        .motion(token::MOTION_INTERACTIVE);
    if on {
        Material::Primary
            .paint(l, b)
            .child(icon(icons::CHECK).size(11.0).color(c.primary_foreground.0))
    } else {
        Material::Well.paint(l, b)
    }
}

/// A segmented control: options side by side in a Well; the chosen one Raised. Each option
/// is `(key, icon, label)`; `on_pick(i)` makes the handler for option `i`.
pub fn segmented<S: 'static, F: Fn(&mut S) + 'static>(
    l: &Look,
    options: &[(&str, Option<Icon>, &str)],
    chosen: usize,
    on_pick: impl Fn(usize) -> F,
) -> El<S> {
    let c = &l.c;
    let hover = c.foreground.0;
    let track = row().h(28.0).pad(2.0).gap(2.0).radius(l.radius.sm);
    Material::Well
        .paint(l, track)
        .role(Role::RadioGroup)
        .focus_group(stillkit::el::Orientation::Horizontal)
        .select_follows_focus()
        .children(options.iter().enumerate().map(|(i, (key, lead, label))| {
            let on = i == chosen;
            let mut o: El<S> = row()
                .key(*key)
                .h_full()
                .pad_x(10.0)
                .gap(6.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .radius(l.radius.xs)
                .role(Role::RadioButton)
                .label(*label)
                .selected(on)
                .checked(on);
            if on {
                o = Material::Raised.paint(l, o);
            }
            o = o
                .color(if on {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                })
                .cascade_ink()
                .motion(token::MOTION_INTERACTIVE)
                .hovered(move |e| e.color(hover))
                .on_press(on_pick(i));
            if let Some(i) = lead {
                o = o.child(icon(*i).size(14.0));
            }
            if !label.is_empty() && lead.is_none() {
                o = o.child(
                    text(*label)
                        .text_size(12.0)
                        .weight(if on { 560 } else { 500 }),
                );
            }
            o
        }))
}

/// A labelled form row: the label above the control, a hint or error below.
pub fn form_row<S: 'static>(
    l: &Look,
    label: &str,
    control: El<S>,
    note: Option<(&str, bool)>,
) -> El<S> {
    let c = &l.c;
    let mut r = col()
        .gap(6.0)
        .child(
            text(label)
                .text_size(12.0)
                .weight(560)
                .color(c.foreground.0),
        )
        .child(control);
    if let Some((note, error)) = note {
        let ink = if error {
            c.destructive_foreground.0
        } else {
            c.muted_foreground.0
        };
        let mut line = row().gap(6.0).align(Align::Center).color(ink);
        if error {
            line = line.child(icon(icons::CIRCLE_ALERT).size(13.0));
        }
        r = r.child(line.child(text(note).text_size(11.0).wrap(3).grow(1.0)));
    }
    r
}
