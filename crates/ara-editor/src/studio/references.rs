//! Unity's object fields in the Inspector: an asset or entity reference shows what it points
//! at, opens a searchable picker (None first, then every asset that fits the field, or every
//! entity in the scene), and sets the pick as one undo step. A cleared reference is `null`.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, Axis, El, col, icon, popover, row, text};

use super::Studio;
use crate::inspector::{Field, Widget};

/// The longest list a picker shows; the search narrows the rest.
const SHOWN: usize = 200;

/// What a reference field shows: the asset's file stem or the entity's name; "None" unset.
fn shown(s: &Studio, f: &Field) -> String {
    if f.mixed {
        return "—".into();
    }
    match (&f.widget, f.value.as_str()) {
        (_, None) | (_, Some("")) => "None".into(),
        (Widget::Entity, Some(id)) => s.name(id),
        (_, Some(reference)) => {
            let source = reference.trim_start_matches('@');
            let source = source.split('#').next().unwrap_or(source);
            crate::shell::names::stem(source.rsplit('/').next().unwrap_or(source)).to_owned()
        }
    }
}

/// The candidates for `f` matching `query`: (value, title, detail), None first.
fn candidates(s: &Studio, f: &Field, query: &str) -> Vec<(Value, String, String)> {
    let query = query.trim().to_lowercase();
    let matches = |title: &str| query.is_empty() || title.to_lowercase().contains(&query);
    let mut out = vec![(Value::Null, "None".to_owned(), String::new())];
    match &f.widget {
        Widget::Entity => {
            let mut entities: Vec<_> = s
                .resolved
                .keys()
                .map(|id| (s.name(id), id.clone()))
                .filter(|(name, _)| matches(name))
                .collect();
            entities.sort();
            out.extend(
                entities
                    .into_iter()
                    .map(|(name, id)| (json!(id), name, String::new())),
            );
        }
        Widget::Asset { kind } => {
            let workspace = s.editor.host.store().workspace();
            let mut assets: Vec<_> = s
                .assets
                .iter()
                .flat_map(|b| b.items.iter())
                .filter(|a| a.fits(kind.as_deref(), &f.path))
                .filter_map(|a| {
                    let reference = a.current_reference(workspace)?;
                    let title = crate::shell::names::stem(a.name()).to_owned();
                    let folder = a.path.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
                    matches(&title).then_some((json!(reference), title, folder))
                })
                .collect();
            assets.sort_by(|a, b| a.1.cmp(&b.1));
            out.extend(assets);
        }
        _ => {}
    }
    out.truncate(SHOWN + 1);
    out
}

/// The reference field for `f`: its value, a chevron, and (while open) its picker.
pub(super) fn field(
    s: &Studio,
    f: &Field,
    commit: fn(&mut Studio, &str, Value, &str),
) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let key = format!("field/{}", f.path);
    let open = s
        .local
        .reference
        .as_ref()
        .is_some_and(|(path, _)| *path == f.path);
    let entity = matches!(f.widget, Widget::Entity);
    let path = f.path.clone();
    let mut trigger = row()
        .key(key.clone())
        .h(24.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(l.radius.xs)
        .fill(l.well())
        .role(stillkit::Role::ComboBox)
        .label(format!("{} reference", f.label))
        .expanded(open)
        .on_press(move |s: &mut Studio| {
            s.local.reference = match &s.local.reference {
                Some((p, _)) if *p == path => None,
                _ => Some((path.clone(), String::new())),
            };
            s.local.focus = Some("reference/search");
        })
        .child(
            icon(if entity { icons::BOX } else { icons::PACKAGE })
                .size(13.0)
                .color(c.muted_foreground.0),
        )
        .child(
            text(shown(s, f))
                .text_size(12.0)
                .color(if f.value.is_null() {
                    c.muted_foreground.0
                } else {
                    c.foreground.0
                })
                .wrap(1)
                .w(0.0)
                .grow(1.0),
        )
        .child(
            icon(icons::CHEVRONS_UP_DOWN)
                .size(13.0)
                .color(c.muted_foreground.0),
        );
    if let Some((_, query)) = s.local.reference.as_ref().filter(|_| open) {
        trigger = trigger.overlay(
            popover(picker(s, f, query, commit))
                .side(stillkit::el::Side::Bottom)
                .align(Align::Start)
                .offset(4.0)
                .on_dismiss(|s: &mut Studio| s.local.reference = None),
        );
    }
    trigger
}

/// The picker: a search, then None and what fits; the current one is ticked.
fn picker(
    s: &Studio,
    f: &Field,
    query: &str,
    commit: fn(&mut Studio, &str, Value, &str),
) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let found = candidates(s, f, query);
    let mut list = col()
        .key("reference/list")
        .max_h(320.0)
        .pad(4.0)
        .gap(1.0)
        .scroll(Axis::Vertical);
    if found.len() == 1 {
        let what = match &f.widget {
            Widget::Asset { kind } => {
                crate::shell::assets::field_accepts(kind.as_deref(), &f.path).1
            }
            _ => "an entity",
        };
        list = list.child(
            row().pad(8.0).child(
                text(format!("Nothing here is {what}."))
                    .text_size(11.5)
                    .color(c.muted_foreground.0),
            ),
        );
    }
    for (index, (value, title, detail)) in found.into_iter().enumerate() {
        let current = value == f.value;
        let (path, label, hover) = (f.path.clone(), f.label.clone(), c.hover_surface.0);
        list = list.child(
            row()
                .key(format!("reference/item/{index}"))
                .h(26.0)
                .pad_x(8.0)
                .gap(8.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(title.clone())
                .hovered(move |e| e.fill(hover))
                .on_press(move |s: &mut Studio| {
                    s.local.reference = None;
                    commit(s, &path, value.clone(), &format!("Set {label}"));
                })
                .child(
                    icon(if current { icons::CHECK } else { icons::DOT })
                        .size(12.0)
                        .color(if current {
                            c.foreground.0
                        } else {
                            stillkit::Rgba::TRANSPARENT
                        }),
                )
                .child(
                    text(title)
                        .text_size(12.0)
                        .color(c.foreground.0)
                        .wrap(1)
                        .w(0.0)
                        .grow(1.0),
                )
                .child(
                    text(detail)
                        .text_size(10.5)
                        .color(c.muted_foreground.0)
                        .wrap(1),
                ),
        );
    }
    super::style::float(l, col())
        .key("reference/panel")
        .w(300.0)
        .radius(l.radius.sm + 2.0)
        .child(
            row().pad(6.0).child(
                search(
                    l,
                    "reference/search",
                    "Search",
                    query,
                    None,
                    |f| {
                        f.on_input(|s: &mut Studio, t| {
                            if let Some((_, query)) = s.local.reference.as_mut() {
                                *query = t.to_owned();
                            }
                        })
                    },
                    |s: &mut Studio| {
                        if let Some((_, query)) = s.local.reference.as_mut() {
                            query.clear();
                        }
                    },
                )
                .grow(1.0),
            ),
        )
        .child(list)
}
