//! Settings › Keyboard shortcuts: every command the studio runs, by menu, with the binding
//! in force (`commands::Bindings`), the commands sharing its chord, and Record, Clear and
//! Reset. A recorded chord is the person's own binding (`input.shortcuts`), above the
//! keymap preset; every change is an undoable settings edit, as Unity's Shortcuts Manager
//! and Unreal's Keyboard Shortcuts edit theirs.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Map, Value, json};
use stillkit::el::{Align, El, Handled, KeyCode, col, el, row, text};

use super::{Studio, commands};

/// The person's own bindings, by command id.
fn own(s: &Studio) -> Map<String, Value> {
    s.editor
        .state
        .settings
        .get("input.shortcuts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Binds `id` to `keys` (`""` unbinds it), or with `None` gives it back to the preset.
pub(super) fn bind(s: &mut Studio, id: &str, keys: Option<String>) {
    let mut map = own(s);
    match keys {
        Some(keys) => map.insert(id.to_owned(), Value::String(keys)),
        None => map.remove(id),
    };
    let params = match map.is_empty() {
        true => json!({"key": "input.shortcuts", "reset": true}),
        false => json!({"key": "input.shortcuts", "value": map}),
    };
    s.act("editor.settings.set", params);
    commands::refresh_keys(s);
}

/// While a row records: the next chord binds it, Esc stops; a lone modifier waits.
fn record_key(s: &mut Studio, key: KeyCode, mods: stillkit::el::Modifiers) -> Handled {
    let Some(id) = s.local.recording_shortcut else {
        return Handled::No;
    };
    if key == KeyCode::ESCAPE && !mods.ctrl && !mods.shift && !mods.alt {
        s.local.recording_shortcut = None;
        return Handled::Yes;
    }
    if let Some(chord) = crate::keys::Chord::from_key(key.0, mods.ctrl, mods.shift, mods.alt) {
        s.local.recording_shortcut = None;
        bind(s, id, Some(chord.text()));
    }
    Handled::Yes
}

/// The commands matching `query` (title, id, menu or binding), grouped by menu, as rows.
pub(super) fn table(s: &Studio, query: &str) -> Option<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let q = query.trim().to_lowercase();
    let map = &s.local.bindings.map;
    let mine = own(s);
    let mut rows: Vec<(String, &'static crate::catalog::Command)> = crate::catalog::all()
        .iter()
        .filter(|cmd| crate::catalog::later(cmd.id).is_none())
        .map(|cmd| {
            let menu = crate::catalog::menu_path(&s.editor.extensions, cmd.id);
            (
                if menu.is_empty() {
                    "Other".to_owned()
                } else {
                    menu
                },
                cmd,
            )
        })
        .filter(|(menu, cmd)| {
            q.is_empty()
                || cmd.title.to_lowercase().contains(&q)
                || cmd.id.contains(&q)
                || menu.to_lowercase().contains(&q)
                || map
                    .chords(cmd.id)
                    .iter()
                    .any(|k| k.text().to_lowercase().contains(&q))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.title.cmp(b.1.title)));
    let mut body = col().key("shortcuts").gap(1.0);
    let mut last = String::new();
    for (menu, cmd) in rows {
        let group = menu.split(" › ").next().unwrap_or("Other").to_owned();
        if group != last {
            body = body.child(
                text(group.clone())
                    .text_size(11.5)
                    .weight(700)
                    .color(c.muted_foreground.0)
                    .pad_y(8.0),
            );
            last = group;
        }
        body = body.child(shortcut_row(s, cmd, &menu, mine.contains_key(cmd.id)));
    }
    Some(body)
}

/// One command: its title and menu, its chords (or the recording prompt), what else
/// shares them, and Record, Clear and Reset.
fn shortcut_row(
    s: &Studio,
    cmd: &'static crate::catalog::Command,
    menu: &str,
    rebound: bool,
) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let id = cmd.id;
    let map = &s.local.bindings.map;
    let recording = s.local.recording_shortcut == Some(id);
    let mut keys = row().gap(4.0).align(Align::Center);
    if recording {
        keys = keys.child(
            text("Press a shortcut · Esc stops")
                .text_size(11.5)
                .color(c.primary.0),
        );
    } else {
        let chords = map.chords(id);
        if chords.is_empty() {
            keys = keys.child(text("—").text_size(12.0).color(c.muted_foreground.0));
        }
        for chord in chords {
            if let Some(k) = commands::shortcut(&chord.text()) {
                keys = keys.child(kbd(l, &k));
            }
        }
    }
    let conflicts = map.conflicts(id);
    let mut left = col().w(0.0).grow(1.0).min_w(0.0).gap(1.0).child(
        row()
            .gap(6.0)
            .align(Align::Center)
            .child(
                text(cmd.title)
                    .text_size(12.5)
                    .weight(540)
                    .color(c.foreground.0),
            )
            .children(rebound.then(|| el().size(6.0).radius(3.0).fill(c.primary.0))),
    );
    left = left.child(
        text(menu.to_owned())
            .text_size(11.0)
            .color(c.muted_foreground.0),
    );
    if !conflicts.is_empty() {
        let names: Vec<&str> = conflicts
            .iter()
            .map(|other| crate::catalog::get(other).map_or(*other, |o| o.title))
            .collect();
        left = left.child(
            text(format!("Also bound to {}", names.join(", ")))
                .text_size(11.0)
                .color(c.warning_foreground.0),
        );
    }
    let record = icon_button(l, Tone::Ghost, Size::Sm, icons::PENCIL, "Record a shortcut")
        .key(format!("shortcuts/{id}/record"))
        .focusable()
        .on_key(|s: &mut Studio, key, tx| record_key(s, key, tx.modifiers()))
        .on_press_with(move |s: &mut Studio, tx| {
            s.local.recording_shortcut = Some(id);
            tx.focus(format!("shortcuts/{id}/record"));
        });
    let clear = icon_button(l, Tone::Ghost, Size::Sm, icons::X, "No shortcut")
        .key(format!("shortcuts/{id}/clear"))
        .on_press(move |s: &mut Studio| bind(s, id, Some(String::new())));
    let mut right = row()
        .gap(2.0)
        .align(Align::Center)
        .child(keys)
        .child(el().w(6.0));
    right = right.child(record).child(clear);
    if rebound {
        right = right.child(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::UNDO_2,
                "Back to the preset's",
            )
            .key(format!("shortcuts/{id}/reset"))
            .on_press(move |s: &mut Studio| bind(s, id, None)),
        );
    }
    let hover = c.hover_surface.0;
    row()
        .key(format!("shortcuts/{id}"))
        .min_h(40.0)
        .pad_x(6.0)
        .pad_y(4.0)
        .gap(12.0)
        .radius(super::style::RADIUS)
        .align(Align::Center)
        .label(cmd.title)
        .description(cmd.summary)
        .hovered(move |e| e.fill(hover))
        .child(left)
        .child(right)
}
