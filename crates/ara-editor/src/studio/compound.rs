//! Compound Inspector fields (lists, keyed lists, maps, nested structs), edited in place as
//! Unity's Inspector edits arrays: the field's row folds its elements open and adds one;
//! each element gets the editor its schema type asks for (checkbox, number, text, vector,
//! enum, asset or entity reference), structs nest their fields, and an element moves or
//! is removed from its own row. Every edit is one `editor.property.nested.edit` per
//! selected entity: a JSON pointer into the field, resolved against each one's current value.
use ara_kit::icons;
use ara_kit::ui::*;
use ara_schema::{SchemaDb, Shape, ValueType};
use serde_json::{Value, json};
use stillkit::el::{Align, El, icon, menu, row, text};

use super::Studio;
use crate::inspector::{Field, Provenance, Widget};

/// How deep nested structs open; deeper values show a summary.
const DEPTH: usize = 3;
/// The most elements a list shows open (the rest stay in the document, counted).
const SHOWN: usize = 64;

fn inner(ty: &ValueType) -> &ValueType {
    match ty {
        ValueType::Option(t) => inner(t),
        t => t,
    }
}

/// A JSON-pointer token for `key`.
fn token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// A new element of `ty`: a struct with every field at its default, else the zero.
fn element_default(db: &SchemaDb, ty: &ValueType) -> Value {
    if let ValueType::Named(name) = inner(ty)
        && let Some(Shape::Struct { fields, .. }) = db.get(name).map(|t| &t.shape)
    {
        return Value::Object(
            fields
                .iter()
                .map(|f| (f.name.clone(), db.field_default(f)))
                .collect(),
        );
    }
    db.zero(ty)
}

/// Edits `path` at `pointer` on every selected entity as one undo step.
fn nested(s: &mut Studio, path: &str, pointer: &str, operation: &str, value: Value, why: &str) {
    let commands = s
        .editor
        .state
        .selection
        .iter()
        .map(|entity| {
            json!({"cmd": "editor.property.nested.edit", "args": {
                "entity": entity, "path": path, "pointer": pointer,
                "operation": operation, "value": value,
            }})
        })
        .collect();
    s.apply(commands, why);
}

/// Sets a field target: `path`, or `path#pointer` inside a compound field.
pub(super) fn commit_target(s: &mut Studio, target: &str, value: Value, why: &str) {
    match target.split_once('#') {
        Some((path, pointer)) => nested(s, path, pointer, "set", value, why),
        None => {
            let commands =
                crate::inspector::edit_commands(&s.editor.state.selection, target, &value);
            s.apply(commands, why);
        }
    }
}

/// The value cell of compound field `f`: a fold chevron, its count, and Add for a list or
/// map (or Set for an unset struct).
pub(super) fn head(s: &Studio, f: &Field, ty: &ValueType) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let open = s.local.unfolded.contains(&f.path);
    let ty = inner(ty);
    let summary = if f.mixed {
        "—".to_owned()
    } else {
        match &f.value {
            Value::Array(a) if a.is_empty() => "Empty".to_owned(),
            Value::Array(a) => format!("{} item{}", a.len(), if a.len() == 1 { "" } else { "s" }),
            Value::Object(o) if matches!(ty, ValueType::Map(_)) => format!("{} entries", o.len()),
            Value::Object(_) => "Set".to_owned(),
            Value::Null => "None".to_owned(),
            other => other.to_string(),
        }
    };
    let path = f.path.clone();
    let mut out = row()
        .key(format!("field/{}", f.path))
        .h(24.0)
        .gap(4.0)
        .align(Align::Center)
        .child(
            row()
                .key(format!("compound/{}/fold", f.path))
                .h(24.0)
                .pad_x(6.0)
                .gap(6.0)
                .grow(1.0)
                .align(Align::Center)
                .radius(l.radius.xs)
                .fill(l.well())
                .role(stillkit::Role::Button)
                .label(if open { "Fold" } else { "Unfold" })
                .expanded(open)
                .on_press(move |s: &mut Studio| {
                    if !s.local.unfolded.remove(&path) {
                        s.local.unfolded.insert(path.clone());
                    }
                })
                .child(
                    icon(if open {
                        icons::CHEVRON_DOWN
                    } else {
                        icons::CHEVRON_RIGHT
                    })
                    .size(12.0)
                    .color(c.muted_foreground.0),
                )
                .child(text(summary).text_size(12.0).color(c.foreground.0)),
        );
    if f.read_only || f.mixed {
        return out;
    }
    let db = s.editor.host.store().schema();
    let added = match ty {
        ValueType::List(element) | ValueType::Keyed(element) => {
            Some(("append", element_default(db, element)))
        }
        ValueType::Map(element) => Some((
            "append",
            json!({"key": "entry", "value": element_default(db, element)}),
        )),
        ValueType::Named(_) if f.value.is_null() => Some(("set", element_default(db, ty))),
        _ => None,
    };
    if let Some((operation, value)) = added {
        let (path, label) = (f.path.clone(), f.label.clone());
        let empty = f.value.is_null();
        let whole =
            matches!(ty, ValueType::List(_) | ValueType::Keyed(_)).then(|| json!([value.clone()]));
        out = out.child(
            icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, "Add an element")
                .size(22.0)
                .key(format!("compound/{}/add", f.path))
                .on_press(move |s: &mut Studio| {
                    s.local.unfolded.insert(path.clone());
                    // An unset list starts as one with the element.
                    match whole.clone().filter(|_| empty) {
                        Some(whole) => commit_target(s, &path, whole, &format!("Add to {label}")),
                        None => nested(
                            s,
                            &path,
                            "",
                            operation,
                            value.clone(),
                            &format!("Add to {label}"),
                        ),
                    }
                }),
        );
    }
    out
}

/// The rows under an unfolded compound field: its elements (or a struct's fields).
pub(super) fn rows(s: &Studio, f: &Field, ty: &ValueType) -> Vec<El<Studio>> {
    if !s.local.unfolded.contains(&f.path) || f.mixed {
        return Vec::new();
    }
    let mut out = Vec::new();
    value_rows(s, f, "", ty, &f.value, 1, &mut out);
    out
}

/// The rows of `value` (of type `ty`) at `pointer` inside `f`, indented by `depth`.
fn value_rows(
    s: &Studio,
    f: &Field,
    pointer: &str,
    ty: &ValueType,
    value: &Value,
    depth: usize,
    out: &mut Vec<El<Studio>>,
) {
    let db = s.editor.host.store().schema();
    match inner(ty) {
        ValueType::List(element) | ValueType::Keyed(element) => {
            let items = value.as_array().map(Vec::as_slice).unwrap_or_default();
            let count = items.len();
            for (index, item) in items.iter().enumerate().take(SHOWN) {
                let at = format!("{pointer}/{index}");
                let title = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("{index}"), str::to_owned);
                let tools = tools(s, f, &at, index, count);
                element_rows(s, f, &at, element, item, &title, tools, depth, out);
            }
            if count > SHOWN {
                out.push(note(s, &format!("{} more", count - SHOWN), depth));
            }
        }
        ValueType::Map(element) => {
            let entries = value.as_object().into_iter().flatten();
            for (key, item) in entries.take(SHOWN) {
                let at = format!("{pointer}/{}", token(key));
                let tools = vec![remove(s, f, &at)];
                element_rows(s, f, &at, element, item, key, tools, depth, out);
            }
        }
        ValueType::Named(name) => {
            if let Some(Shape::Struct { fields, .. }) = db.get(name).map(|t| &t.shape) {
                for field in fields
                    .iter()
                    .filter(|d| crate::inspector::shows(db, fields, d, Some(value)))
                {
                    let at = format!("{pointer}/{}", token(&field.name));
                    let item = value
                        .get(&field.name)
                        .cloned()
                        .unwrap_or_else(|| db.field_default(field));
                    element_rows(
                        s,
                        f,
                        &at,
                        &field.ty,
                        &item,
                        &crate::shell::inspector_panel::sentence(&field.name),
                        Vec::new(),
                        depth,
                        out,
                    );
                }
            }
        }
        _ => {}
    }
}

/// One element: a row with its title, editor and tools; a struct (or a nested collection)
/// opens its own rows beneath, one level in.
#[allow(clippy::too_many_arguments)]
fn element_rows(
    s: &Studio,
    f: &Field,
    pointer: &str,
    ty: &ValueType,
    value: &Value,
    title: &str,
    tools: Vec<El<Studio>>,
    depth: usize,
    out: &mut Vec<El<Studio>>,
) {
    let db = s.editor.host.store().schema();
    let nests = match inner(ty) {
        ValueType::Named(name) => {
            matches!(db.get(name).map(|t| &t.shape), Some(Shape::Struct { .. }))
        }
        ValueType::List(_) | ValueType::Keyed(_) | ValueType::Map(_) => true,
        _ => false,
    };
    let c = &s.look.c;
    let editor = if nests {
        text(match value {
            Value::Array(a) => format!("{} items", a.len()),
            Value::Null => "None".to_owned(),
            _ => String::new(),
        })
        .text_size(11.5)
        .color(c.muted_foreground.0)
    } else {
        leaf(s, f, pointer, ty, value)
    };
    out.push(
        row()
            .key(format!("compound/{}#{pointer}", f.path))
            .min_h(26.0)
            .gap(6.0)
            .align(Align::Center)
            .child(stillkit::el::el().w(12.0 * depth as f32))
            .child(
                text(title.to_owned())
                    .text_size(11.5)
                    .weight(if nests { 600 } else { 400 })
                    .color(c.muted_foreground.0)
                    .wrap(1)
                    .w(0.0)
                    .grow(0.4),
            )
            .child(editor.w(0.0).grow(0.6))
            .children(tools),
    );
    if nests && depth < DEPTH && !value.is_null() {
        value_rows(s, f, pointer, ty, value, depth + 1, out);
    }
}

/// A muted line inside the element list.
fn note(s: &Studio, message: &str, depth: usize) -> El<Studio> {
    row()
        .min_h(22.0)
        .child(stillkit::el::el().w(12.0 * depth as f32))
        .child(
            text(message.to_owned())
                .text_size(11.0)
                .color(s.look.c.muted_foreground.0),
        )
}

/// A list element's tools: move up, move down, remove.
fn tools(s: &Studio, f: &Field, pointer: &str, index: usize, count: usize) -> Vec<El<Studio>> {
    if f.read_only {
        return Vec::new();
    }
    let l = &s.look;
    let mut out = Vec::new();
    for (operation, glyph, label, shown) in [
        ("moveUp", icons::CHEVRON_UP, "Move up", index > 0),
        (
            "moveDown",
            icons::CHEVRON_DOWN,
            "Move down",
            index + 1 < count,
        ),
    ] {
        let (path, at) = (f.path.clone(), pointer.to_owned());
        out.push(enabled(
            icon_button(l, Tone::Ghost, Size::Sm, glyph, label)
                .size(20.0)
                .key(format!("compound/{}#{pointer}/{operation}", f.path))
                .on_press(move |s: &mut Studio| {
                    nested(s, &path, &at, operation, Value::Null, label)
                }),
            shown,
        ));
    }
    out.push(remove(s, f, pointer));
    out
}

fn remove(s: &Studio, f: &Field, pointer: &str) -> El<Studio> {
    let (path, at) = (f.path.clone(), pointer.to_owned());
    icon_button(&s.look, Tone::Ghost, Size::Sm, icons::X, "Remove")
        .size(20.0)
        .key(format!("compound/{}#{pointer}/remove", f.path))
        .on_press(move |s: &mut Studio| {
            nested(s, &path, &at, "remove", Value::Null, "Remove element")
        })
}

/// The editor for one scalar element of type `ty`.
fn leaf(s: &Studio, f: &Field, pointer: &str, ty: &ValueType, value: &Value) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let target = format!("{}#{pointer}", f.path);
    let key = format!("compound/{target}/value");
    if f.read_only {
        return text(crate::shell::vfx_panel::short(value))
            .text_size(12.0)
            .color(c.muted_foreground.0);
    }
    let db = s.editor.host.store().schema();
    match inner(ty) {
        ValueType::Bool => {
            let on = value.as_bool().unwrap_or(false);
            row().child(checkbox(l, key, "", on).on_press(move |s: &mut Studio| {
                commit_target(s, &target, json!(!on), "Toggle element")
            }))
        }
        ValueType::Entity | ValueType::Asset(_) => {
            let widget = match inner(ty) {
                ValueType::Asset(kind) => Widget::Asset { kind: kind.clone() },
                _ => Widget::Entity,
            };
            let field = Field {
                path: target,
                label: f.label.clone(),
                doc: f.doc.clone(),
                value: value.clone(),
                mixed: false,
                channels: Vec::new(),
                provenance: Provenance::Authored,
                was: None,
                unit: None,
                widget,
                read_only: false,
                optional: false,
                fallback: Value::Null,
            };
            super::references::field(s, &field, commit_target)
        }
        ValueType::Named(name) => match db.get(name).map(|t| &t.shape) {
            Some(Shape::Enum { variants }) => {
                let current = value.as_str().unwrap_or_default().to_owned();
                let open = s.local.select.as_deref() == Some(key.as_str());
                let toggle = key.clone();
                let mut trigger = select(l, Size::Sm, key.clone(), &current)
                    .expanded(open)
                    .on_press(move |s: &mut Studio| {
                        s.local.select = match s.local.select.as_deref() == Some(toggle.as_str()) {
                            true => None,
                            false => Some(toggle.clone()),
                        }
                    });
                if open {
                    let items = variants
                        .iter()
                        .map(|v| {
                            let (target, name) = (target.clone(), v.name.clone());
                            menu_check(l, &format!("{key}/{}", v.name), &v.name, v.name == current)
                                .on_press(move |s: &mut Studio| {
                                    s.local.select = None;
                                    commit_target(s, &target, json!(name), "Choose value");
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
            _ => text(crate::shell::vfx_panel::short(value))
                .text_size(12.0)
                .color(c.muted_foreground.0),
        },
        ValueType::Json => text(value.to_string())
            .text_size(12.0)
            .color(c.muted_foreground.0)
            .wrap(1),
        kind => {
            // Numbers, text, vectors (typed `x, y, z`) and colours (their channels).
            let shown = match value {
                Value::String(t) => t.clone(),
                Value::Array(items) => items
                    .iter()
                    .map(|v| v.as_f64().map_or_else(|| v.to_string(), short_number))
                    .collect::<Vec<_>>()
                    .join(", "),
                Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), short_number),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            let kind = kind.clone();
            field(l, Size::Sm, key, None, "", &shown, move |x| {
                x.select_all_on_focus()
                    .on_submit(move |s: &mut Studio, t| match parse(&kind, t) {
                        Some(value) => commit_target(s, &target, value, "Edit element"),
                        None => s
                            .editor
                            .report_failure(&*anyhow::anyhow!("`{t}` is not a {kind:?} value")),
                    })
            })
        }
    }
}

/// A number without float noise: six decimals at most, trailing zeros gone.
fn short_number(v: f64) -> String {
    let text = format!("{v:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Typed text as a value of `ty`: a number, text, or comma-separated lanes.
fn parse(ty: &ValueType, text: &str) -> Option<Value> {
    let text = text.trim();
    let lanes = |n: usize| {
        let values: Option<Vec<f64>> = text
            .trim_matches(['[', ']'])
            .split(',')
            .map(|p| p.trim().parse().ok())
            .collect();
        values.filter(|v| v.len() == n).map(|v| json!(v))
    };
    match ty {
        ValueType::String => Some(json!(text)),
        ValueType::F32 | ValueType::F64 => text
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| json!(v)),
        ValueType::I32 | ValueType::I64 => text.parse::<i64>().ok().map(|v| json!(v)),
        ValueType::U8 | ValueType::U16 | ValueType::U32 | ValueType::U64 => {
            text.parse::<u64>().ok().map(|v| json!(v))
        }
        ValueType::Vec2 => lanes(2),
        ValueType::Vec3 => lanes(3),
        ValueType::Vec4 | ValueType::Quat | ValueType::Color => lanes(4),
        _ => serde_json::from_str(text).ok(),
    }
}
