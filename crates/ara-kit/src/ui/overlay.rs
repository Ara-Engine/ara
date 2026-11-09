//! Overlay content: menu panels and items, dialog panels and toasts. Open them with
//! stillkit's overlays (`.overlay(menu(menu_panel(l, items)).open(s.menu).on_dismiss(..))`),
//! which place, focus, trap and dismiss them.
use stillkit::el::{Align, El, Justify, col, el, icon, row, text};
use stillkit::icons::Icon;
use stillkit::{Role, ShadowLevel, TextRole};

use super::{Size, Status, Tone, button, icon_button, status_paints};
use crate::icons;
use crate::look::Look;

/// A menu's panel: the menu surface, a hairline and the menu shadow, holding `items`.
pub fn menu_panel<S: 'static>(l: &Look, items: Vec<El<S>>) -> El<S> {
    col()
        .min_w(200.0)
        .pad(4.0)
        .gap(1.0)
        .radius(l.radius.md)
        .fill(l.c.menu_surface.0)
        .border(1.0, l.c.separator_strong.0)
        .shadow_level(ShadowLevel::Menu)
        .role(Role::Menu)
        .children(items)
}

/// One menu item: optional icon, label, optional shortcut. `danger` reads in the error ink.
pub fn menu_item<S: 'static>(
    l: &Look,
    key: &str,
    lead: Option<Icon>,
    label: &str,
    shortcut: Option<&str>,
    danger: bool,
) -> El<S> {
    let c = &l.c;
    let ink = if danger {
        c.destructive_foreground.0
    } else {
        c.foreground.0
    };
    let mut r = row()
        .key(key)
        .h(26.0)
        .pad_x(8.0)
        .gap(8.0)
        .align(Align::Center)
        .radius(l.radius.xs)
        .role(Role::MenuItem)
        .label(label)
        .color(ink)
        .cascade_ink()
        .child(
            el().size(15.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .children(lead.map(|i| {
                    icon(i)
                        .size(14.0)
                        .color(if danger { ink } else { c.muted_foreground.0 })
                })),
        )
        .child(text(label).text_size(12.0).grow(1.0));
    if let Some(keys) = shortcut {
        r = r.child(text(keys).text_size(11.0).color(c.muted_foreground.0));
    }
    r
}

/// A checkable menu item: a check mark while `on`.
pub fn menu_check<S: 'static>(l: &Look, key: &str, label: &str, on: bool) -> El<S> {
    menu_item(l, key, on.then_some(icons::CHECK), label, None, false)
        .role(Role::MenuItemCheckBox)
        .checked(on)
}

/// Room between menu groups: spacing, not a rule.
pub fn menu_separator<S: 'static>(_: &Look) -> El<S> {
    el().h(7.0)
}

/// A muted heading inside a menu.
pub fn menu_heading<S: 'static>(l: &Look, label: &str) -> El<S> {
    row().h(24.0).pad_x(8.0).align(Align::Center).child(
        text(label)
            .text_size(11.0)
            .weight(600)
            .color(l.c.muted_foreground.0),
    )
}

/// A dialog's panel: an optional status icon, title, body (any content) and actions at the
/// bottom right (the last is the default; destructive dialogs focus Cancel first by putting
/// it first and keying it).
pub fn dialog_panel<S: 'static>(
    l: &Look,
    status: Option<(Status, Icon)>,
    title: &str,
    body: El<S>,
    actions: Vec<El<S>>,
    close: impl Fn(&mut S) + 'static,
) -> El<S> {
    let c = &l.c;
    let mut head = row().gap(10.0).align(Align::Center);
    if let Some((s, glyph)) = status {
        let (fill, ink, _) = status_paints(l, s);
        head = head.child(
            el().size(28.0)
                .radius(8.0)
                .fill(fill)
                .align(Align::Center)
                .justify(Justify::Center)
                .child(icon(glyph).size(16.0).color(ink)),
        );
    }
    head = head
        .child(
            text(title)
                .font(TextRole::Title3)
                .color(c.foreground.0)
                .grow(1.0),
        )
        .child(
            icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Close")
                .key("dialog/close")
                .on_press(close),
        );
    col()
        .w(440.0)
        .max_w(560.0)
        .radius(l.radius.base + 2.0)
        .fill(c.popover.0)
        .border(1.0, c.separator_strong.0)
        .shadow_level(ShadowLevel::Dialog)
        .role(Role::Dialog)
        .label(title)
        .child(col().pad(20.0).gap(14.0).child(head).child(body))
        .child(
            col().child(el().h(1.0).w_full().fill(c.border.0)).child(
                row()
                    .pad_x(20.0)
                    .pad_y(14.0)
                    .gap(8.0)
                    .justify(Justify::End)
                    .children(actions),
            ),
        )
}

/// Body text for a dialog.
pub fn dialog_text<S: 'static>(l: &Look, body: &str) -> El<S> {
    text(body)
        .text_size(12.5)
        .wrap(12)
        .color(l.c.muted_foreground.0)
}

/// A toast: status icon, message, an optional action and a dismiss button.
pub fn toast<S: 'static>(
    l: &Look,
    key: &str,
    status: Status,
    message: &str,
    action: Option<El<S>>,
    dismiss: impl Fn(&mut S) + 'static,
) -> El<S> {
    let c = &l.c;
    let (_, ink, _) = status_paints(l, status);
    let glyph = match status {
        Status::Success => icons::CIRCLE_CHECK,
        Status::Danger => icons::CIRCLE_ALERT,
        Status::Warning => icons::TRIANGLE_ALERT,
        _ => icons::INFO,
    };
    let mut t = row()
        .key(key)
        .w(360.0)
        .pad_x(12.0)
        .pad_y(10.0)
        .gap(10.0)
        .align(Align::Center)
        .radius(l.radius.md)
        .fill(c.popover.0)
        .border(1.0, c.separator_strong.0)
        .shadow_level(ShadowLevel::Toast)
        .role(Role::Status)
        .transition(stillkit::el::Transition::slide(0.0, 12.0))
        .child(icon(glyph).size(16.0).color(ink))
        .child(
            text(message)
                .text_size(12.0)
                .wrap(3)
                .grow(1.0)
                .color(c.foreground.0),
        );
    if let Some(a) = action {
        t = t.child(a);
    }
    t.child(
        icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Dismiss")
            .key(format!("{key}/dismiss"))
            .on_press(dismiss),
    )
}

/// A dialog's default action button.
pub fn primary<S: 'static>(l: &Look, label: &str) -> El<S> {
    button(l, Tone::Primary, Size::Md, None, label)
}

/// A dialog's secondary action button.
pub fn secondary<S: 'static>(l: &Look, label: &str) -> El<S> {
    button(l, Tone::Secondary, Size::Md, None, label)
}
