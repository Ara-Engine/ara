//! Settings: one window for your preferences and the project's policy, generated from the
//! settings table (`crate::settings::DEFS`), so every setting agents can set over
//! `editor.settings.set` has a control here. Scope tabs (You / Project), categories down
//! the side, a search across labels and descriptions, and rows with the setting's control;
//! a changed setting shows a dot and Reset. Every edit is undoable and attributed.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::Rgba;
use stillkit::el::{Align, Axis, El, Justify, Overlay, col, dialog, el, icon, menu, row, text};

use super::Studio;
use crate::settings::{DEFS, Def, Kind, Scope};

/// The open Settings window.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// Whose settings: yours or the project's.
    pub scope: Scope,
    /// The chosen category; `None` shows every category.
    pub category: Option<&'static str>,
    /// The search.
    pub query: String,
}

/// Opens Settings on `scope`.
pub fn open(s: &mut Studio, scope: Scope) {
    s.local.settings = Some(View {
        scope,
        category: None,
        query: String::new(),
    });
    s.local.focus = Some("settings/search");
}

/// The setting's value now, or its default.
fn value(s: &Studio, def: &Def) -> Value {
    s.editor
        .state
        .settings
        .get(def.key)
        .cloned()
        .unwrap_or_else(|| def.default.value())
}

/// Sets (or with `None` resets) setting `key`.
fn set(s: &mut Studio, key: &str, value: Option<Value>) {
    let params = match value {
        Some(v) => json!({"key": key, "value": v}),
        None => json!({"key": key, "reset": true}),
    };
    s.act("editor.settings.set", params);
}

/// The window, while open.
pub fn overlay(s: &Studio) -> Option<Overlay<Studio>> {
    let v = s.local.settings.as_ref()?;
    let l = &s.look;
    let c = &l.c;
    let shown: Vec<&Def> = DEFS
        .iter()
        .filter(|d| d.scope == v.scope)
        .filter(|d| {
            let q = v.query.trim().to_lowercase();
            if q.is_empty() {
                v.category.is_none_or(|c| d.category == c)
            } else {
                d.label.to_lowercase().contains(&q)
                    || d.description.to_lowercase().contains(&q)
                    || d.category.to_lowercase().contains(&q)
            }
        })
        .collect();
    let mut categories: Vec<&'static str> = Vec::new();
    for d in DEFS.iter().filter(|d| d.scope == v.scope) {
        if !categories.contains(&d.category) {
            categories.push(d.category);
        }
    }
    // Scope tabs and categories.
    let scope = segmented(
        l,
        &[
            ("settings/scope/user", None, "You"),
            ("settings/scope/project", None, "Project"),
        ],
        usize::from(v.scope == Scope::Project),
        |i| {
            move |s: &mut Studio| {
                if let Some(v) = s.local.settings.as_mut() {
                    v.scope = if i == 0 { Scope::User } else { Scope::Project };
                    v.category = None;
                }
            }
        },
    );
    let mut side = col()
        .w(200.0)
        .min_w(200.0)
        .pad(10.0)
        .gap(2.0)
        .child(scope)
        .child(el().h(8.0));
    let mut cat = |key: String, title: &'static str, chosen: Option<&'static str>, on: bool| {
        let hover = c.hover_surface.0;
        side = std::mem::replace(&mut side, col()).child(
            row()
                .key(key)
                .h(28.0)
                .pad_x(10.0)
                .align(Align::Center)
                .radius(l.radius.sm)
                .fill(if on {
                    c.selection_surface_inactive.0
                } else {
                    Rgba::TRANSPARENT
                })
                .color(if on {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                })
                .cascade_ink()
                .label(title)
                .selected(on)
                .hovered(move |e| if on { e } else { e.fill(hover) })
                .on_press(move |s: &mut Studio| {
                    if let Some(v) = s.local.settings.as_mut() {
                        v.category = chosen;
                        v.query.clear();
                    }
                })
                .child(
                    text(title)
                        .text_size(12.5)
                        .weight(if on { 600 } else { 500 }),
                ),
        );
    };
    let searching = !v.query.trim().is_empty();
    cat(
        "settings/cat/all".into(),
        "All",
        None,
        !searching && v.category.is_none(),
    );
    for category in &categories {
        cat(
            format!("settings/cat/{category}"),
            category,
            Some(category),
            !searching && v.category == Some(category),
        );
    }
    // Rows, grouped by category.
    let mut body = col().pad_x(20.0).pad_y(12.0).gap(2.0);
    let mut last = "";
    for def in &shown {
        if def.category != last {
            last = def.category;
            body = body.child(
                text(def.category)
                    .text_size(11.5)
                    .weight(700)
                    .color(c.muted_foreground.0)
                    .pad_y(8.0),
            );
        }
        body = body.child(setting_row(s, def));
    }
    // Keyboard shortcuts: the commands themselves, under the preset; a search finds
    // commands too.
    let keyboard = v.scope == Scope::User
        && (v.category == Some("Keyboard shortcuts") || !v.query.trim().is_empty());
    let commands = keyboard
        .then(|| super::shortcuts::table(s, &v.query))
        .flatten();
    let nothing = shown.is_empty() && commands.is_none();
    body = body.children(commands);
    if nothing {
        body = body.child(empty(
            l,
            icons::SEARCH,
            "No settings match",
            "Search looks at names and descriptions in this scope.",
            None,
        ));
    }
    let search = row().pad(10.0).child(
        search(
            l,
            "settings/search",
            "Search settings",
            &v.query,
            None,
            |f| {
                f.on_input(|s: &mut Studio, t| {
                    if let Some(v) = s.local.settings.as_mut() {
                        v.query = t.to_owned();
                    }
                })
            },
            |s: &mut Studio| {
                if let Some(v) = s.local.settings.as_mut() {
                    v.query.clear();
                }
            },
        )
        .grow(1.0),
    );
    let head = row()
        .h(44.0)
        .pad_x(16.0)
        .align(Align::Center)
        .child(
            text(if v.scope == Scope::User {
                "Preferences"
            } else {
                "Project Settings"
            })
            .text_size(15.0)
            .weight(650)
            .color(c.foreground.0)
            .grow(1.0),
        )
        .child(
            text(if v.scope == Scope::User {
                "Yours, on every project"
            } else {
                "Shared through settings/project.json"
            })
            .text_size(11.5)
            .color(c.muted_foreground.0),
        )
        .child(el().w(12.0))
        .child(
            icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Close")
                .key("settings/close")
                .on_press(|s: &mut Studio| s.local.settings = None),
        );
    let right = col()
        .w(0.0)
        .grow(1.0)
        .min_h(0.0)
        .child(search)
        .child(col().h(0.0).grow(1.0).scroll(Axis::Vertical).child(body));
    let window = super::style::float(l, col())
        .key("settings")
        .w(860.0)
        .h(600.0)
        .radius(l.radius.base)
        .label("Settings")
        .child(head)
        .child(
            row()
                .h(0.0)
                .grow(1.0)
                .child(side.fill(c.muted.0.with_alpha(0.5)))
                .child(right),
        );
    Some(dialog(window).on_dismiss(|s: &mut Studio| s.local.settings = None))
}

/// One setting: its label and what it does on the left, its control on the right; a dot
/// and Reset when it differs from its default.
fn setting_row(s: &Studio, def: &Def) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let now = value(s, def);
    let changed = now != def.default.value();
    let key = def.key;
    let mut left = col().w(0.0).grow(1.0).min_w(0.0).gap(2.0).child(
        row()
            .gap(6.0)
            .align(Align::Center)
            .child(
                text(def.label)
                    .text_size(13.0)
                    .weight(560)
                    .color(c.foreground.0),
            )
            .children(changed.then(|| el().size(6.0).radius(3.0).fill(c.primary.0))),
    );
    left = left.child(
        text(def.description)
            .text_size(11.5)
            .wrap(2)
            .color(c.muted_foreground.0),
    );
    let mut right = row()
        .gap(6.0)
        .align(Align::Center)
        .justify(Justify::End)
        .w(260.0)
        .min_w(260.0);
    if changed {
        right = right.child(
            icon_button(l, Tone::Ghost, Size::Sm, icons::UNDO_2, "Reset to default")
                .key(format!("settings/{key}/reset"))
                .on_press(move |s: &mut Studio| set(s, key, None)),
        );
    }
    right = right.child(control(s, def, &now));
    row()
        .key(format!("settings/{key}"))
        .min_h(48.0)
        .pad_y(8.0)
        .gap(16.0)
        .align(Align::Center)
        .child(left)
        .child(right)
}

/// The control for a setting's kind.
fn control(s: &Studio, def: &Def, now: &Value) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let key = def.key;
    match def.kind {
        Kind::Toggle => {
            let on = now.as_bool().unwrap_or(false);
            switch(l, format!("settings/{key}/switch"), def.label, on)
                .on_press(move |s: &mut Studio| set(s, key, Some(json!(!on))))
        }
        Kind::Choice(options) if options.len() <= 3 => {
            let chosen = options
                .iter()
                .position(|(v, _)| now.as_str() == Some(*v))
                .unwrap_or(0);
            let keys: Vec<String> = options
                .iter()
                .map(|(v, _)| format!("settings/{key}/{v}"))
                .collect();
            let items: Vec<(&str, Option<stillkit::icons::Icon>, &str)> = options
                .iter()
                .zip(&keys)
                .map(|((_, label), k)| (k.as_str(), None, *label))
                .collect();
            segmented(l, &items, chosen, move |i| {
                move |s: &mut Studio| set(s, key, Some(json!(options[i].0)))
            })
        }
        Kind::Choice(options) => {
            let current = options
                .iter()
                .find(|(v, _)| now.as_str() == Some(*v))
                .map_or("—", |(_, label)| *label);
            let trigger_key = format!("settings/{key}/select");
            let open = s.local.select.as_deref() == Some(trigger_key.as_str());
            let k = trigger_key.clone();
            let mut trigger = select(l, Size::Sm, trigger_key.clone(), current)
                .w(200.0)
                .expanded(open)
                .on_press(move |s: &mut Studio| {
                    s.local.select = if s.local.select.as_deref() == Some(k.as_str()) {
                        None
                    } else {
                        Some(k.clone())
                    };
                });
            if open {
                let items = options
                    .iter()
                    .map(|(v, label)| {
                        menu_check(
                            l,
                            &format!("{trigger_key}/{v}"),
                            label,
                            now.as_str() == Some(*v),
                        )
                        .on_press(move |s: &mut Studio| {
                            s.local.select = None;
                            set(s, key, Some(json!(v)));
                        })
                    })
                    .collect();
                trigger = trigger.overlay(
                    menu(menu_panel(l, items))
                        .match_width()
                        .on_dismiss(|s: &mut Studio| s.local.select = None),
                );
            }
            trigger
        }
        Kind::Number { min, max, unit, .. } => {
            let shown = now
                .as_f64()
                .map(|v| {
                    if v.fract() == 0.0 {
                        format!("{v:.0}")
                    } else {
                        format!("{v}")
                    }
                })
                .unwrap_or_default();
            row()
                .w(140.0)
                .gap(6.0)
                .align(Align::Center)
                .child(
                    field(
                        l,
                        Size::Sm,
                        format!("settings/{key}/number"),
                        None,
                        "",
                        &shown,
                        move |f| {
                            f.on_submit(move |s: &mut Studio, t| {
                                if let Ok(v) = t.trim().parse::<f64>() {
                                    set(s, key, Some(json!(v.clamp(min, max))));
                                }
                            })
                        },
                    )
                    .grow(1.0),
                )
                .children(
                    (!unit.is_empty())
                        .then(|| text(unit).text_size(11.5).color(c.muted_foreground.0)),
                )
        }
        Kind::Text => {
            let shown = now.as_str().unwrap_or("").to_owned();
            field(
                l,
                Size::Sm,
                format!("settings/{key}/text"),
                None,
                // Empty means the default behaviour the description names.
                "Default",
                &shown,
                move |f| f.on_submit(move |s: &mut Studio, t| set(s, key, Some(json!(t)))),
            )
            .w(240.0)
        }
        // Kept elsewhere: what it holds now (the row's Reset clears it).
        Kind::List => {
            let names: Vec<&str> = now
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            text(match names.is_empty() {
                true => "Default".to_owned(),
                false => names.join(", "),
            })
            .text_size(12.0)
            .color(c.muted_foreground.0)
            .wrap(1)
            .tooltip(def.description)
        }
        Kind::Map | Kind::Colors { .. } => {
            let count = match now {
                Value::Object(o) => o.len(),
                Value::Array(a) => a.len(),
                _ => 0,
            };
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(
                    icon(icons::LIST_TREE)
                        .size(13.0)
                        .color(c.muted_foreground.0),
                )
                .child(
                    text(format!("{count} entries"))
                        .text_size(12.0)
                        .color(c.muted_foreground.0),
                )
        }
    }
}
