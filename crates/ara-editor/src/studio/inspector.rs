//! Inspector: the selection's components as folding sections, every field from the schema
//! (`crate::inspector::fields`: widget kind, unit, provenance, mixed values across a
//! multi-selection). Numbers scrub by dragging their label (8 px a step; Shift fine, Alt coarse) and
//! commit one undo step on release; typing commits on Enter. Vectors label their axes in the
//! gizmo's colours. Every edit is `property.set` through `cmd.apply`, as agents edit.
use std::collections::BTreeMap;

use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::Cursor;
use stillkit::el::{
    Align, Axis, El, Justify, PointerPhase, col, el, icon, menu, row, text, text_input,
};

use super::Studio;
use crate::inspector::{Field, Provenance, Widget};

const LABEL: f32 = 0.38;

pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let selection = &s.editor.state.selection;
    if selection.is_empty() {
        return empty(
            l,
            icons::MOUSE_POINTER_2,
            "Nothing selected",
            "Select an entity in the Scene or the Hierarchy to edit its components.",
            None,
        );
    }
    let Some(doc) = s.editor.host.store().workspace().doc(&s.editor.state.doc) else {
        return empty(
            l,
            icons::FILE,
            "Scene unavailable",
            "Open a scene to inspect its entities.",
            None,
        );
    };
    let changes = s.editor.agent_fields();
    let fields = crate::inspector::fields(
        s.editor.host.store().schema(),
        doc,
        &s.resolved,
        selection,
        &changes,
    );
    let mut by_component: BTreeMap<String, Vec<&Field>> = BTreeMap::new();
    let mut order = Vec::new();
    for f in &fields {
        let component = f.path.split('.').next().unwrap_or("").to_owned();
        if !by_component.contains_key(&component) {
            order.push(component.clone());
        }
        by_component.entry(component).or_default().push(f);
    }
    let mut page = col().pad(10.0).gap(8.0).child(header(s));
    // Transform leads, as in every engine; the rest keep their record order.
    order.sort_by_key(|c| c != "Transform");
    for component in order {
        page = page.child(section(s, &component, &by_component[&component]));
    }
    col()
        .grow(1.0)
        .min_h(0.0)
        .scroll(Axis::Vertical)
        .child(page.child(el().h(12.0)))
}

/// The selection's name (editable for one entity) and how many are selected.
fn header(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let selection = &s.editor.state.selection;
    let title = if selection.len() == 1 {
        s.name(&selection[0])
    } else {
        format!("{} entities", selection.len())
    };
    let mut head = row().gap(8.0).align(Align::Center).pad_y(2.0).child(
        el().size(28.0)
            .radius(7.0)
            .fill(c.raised.0)
            .border(1.0, c.border.0)
            .align(Align::Center)
            .justify(Justify::Center)
            .child(icon(icons::BOX).size(15.0).color(c.muted_foreground.0)),
    );
    if selection.len() == 1 {
        let id = selection[0].clone();
        head = head.child(
            field(l, Size::Md, "inspector/name", None, "Name", &title, move |f| {
                f.on_submit(move |s: &mut Studio, t| {
                    let name = t.trim().to_owned();
                    if !name.is_empty() {
                        s.apply(vec![json!({"cmd":"property.set","args":{"entity":id,"path":"name","value":name}})], "Rename entity");
                    }
                })
            })
            .grow(1.0),
        );
    } else {
        head = head.child(
            text(title)
                .text_size(13.0)
                .weight(600)
                .color(c.foreground.0)
                .grow(1.0),
        );
    }
    let id = if selection.len() == 1 {
        selection[0].clone()
    } else {
        String::new()
    };
    col().gap(4.0).child(head).child(
        text(id)
            .text_size(10.5)
            .family(mono())
            .color(c.muted_foreground.0.with_alpha(0.75)),
    )
}

/// A component's section: a header (fold, icon, name) over its fields.
fn section(s: &Studio, component: &str, fields: &[&Field]) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let folded = s.local.folded.contains(component);
    let name = component.to_owned();
    let head = row()
        .key(format!("component/{component}"))
        .h(30.0)
        .pad_x(8.0)
        .gap(8.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .color(c.foreground.0)
        .cascade_ink()
        .label(component.to_owned())
        .expanded(!folded)
        .on_press(move |s: &mut Studio| {
            if !s.local.folded.remove(&name) {
                s.local.folded.insert(name.clone());
            }
        })
        .child(
            icon(if folded {
                icons::CHEVRON_RIGHT
            } else {
                icons::CHEVRON_DOWN
            })
            .size(12.0)
            .color(c.muted_foreground.0),
        )
        .child(
            icon(component_icon(component))
                .size(14.0)
                .color(c.info_foreground.0),
        )
        .child(
            text(component.to_owned())
                .text_size(12.5)
                .weight(600)
                .grow(1.0),
        );
    let mut out = card(l).fill(c.muted.0).child(head);
    if !folded {
        let mut body = col().pad_x(8.0).pad_y(6.0).gap(4.0);
        for f in fields {
            body = body.child(field_row(s, f));
        }
        out = out.child(el().h(1.0).w_full().fill(c.border.0)).child(body);
    }
    out
}

fn component_icon(component: &str) -> stillkit::icons::Icon {
    match component {
        "Transform" => icons::MOVE_3D,
        "MeshRenderer" => icons::BOX,
        "Light" => icons::LIGHTBULB,
        "Camera" => icons::VIDEO,
        "Rigidbody" | "Collider" => icons::ATOM,
        "AudioSource" => icons::VOLUME_2,
        "Terrain" => icons::MOUNTAIN,
        "Animator" => icons::PERSON_STANDING,
        _ => icons::COMPONENT,
    }
}

/// One field: its label (scrubs numbers; a tick marks authored values) and its editor.
fn field_row(s: &Studio, f: &Field) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let mark = match f.provenance {
        Provenance::Default => stillkit::Rgba::TRANSPARENT,
        Provenance::AgentChanged => c.warning.0,
        Provenance::Override => c.info.0,
        _ => c.primary.0.with_alpha(0.7),
    };
    let width = (s.size.0 * 0.22).clamp(220.0, 420.0);
    let mut label = row()
        .w(width * LABEL)
        .min_w(width * LABEL)
        .h(24.0)
        .gap(6.0)
        .align(Align::Center)
        .child(el().w(2.0).h(14.0).radius(1.0).fill(mark))
        .child(
            text(sentence(&f.label))
                .text_size(12.0)
                .color(c.muted_foreground.0),
        )
        .tooltip(if f.doc.is_empty() {
            f.label.clone()
        } else {
            f.doc.clone()
        });
    if let Widget::Number { step, min, max, .. } = &f.widget
        && !f.read_only
    {
        label = scrubbable(
            label,
            f.path.clone(),
            None,
            f.value.as_f64().unwrap_or(0.0),
            *step,
            *min,
            *max,
        );
    }
    row()
        .gap(6.0)
        .align(Align::Center)
        .min_h(24.0)
        .child(label)
        .child(editor(s, f).grow(1.0).min_w(0.0))
}

/// A label that scrubs a number: 8 px per step to the right, Shift for a tenth, Alt for ten.
#[allow(clippy::too_many_arguments)]
fn scrubbable(
    label: El<Studio>,
    path: String,
    axis: Option<usize>,
    value: f64,
    step: f64,
    min: Option<f64>,
    max: Option<f64>,
) -> El<Studio> {
    let step = if step > 0.0 { step } else { 0.01 };
    label
        .cursor(Cursor::ResizeHorizontal)
        .on_pointer(move |s: &mut Studio, e, _| {
            match e.phase {
                PointerPhase::Down => {
                    for id in s.editor.state.selection.clone() {
                        let _ = s.editor.field_gesture(&id, &path, true);
                    }
                    s.local.scrub = Some((path.clone(), axis, value, value));
                }
                PointerPhase::Move => {
                    let k = if e.mods.shift {
                        0.1
                    } else if e.mods.alt {
                        10.0
                    } else {
                        1.0
                    };
                    if let Some((_, _, _, now)) = s.local.scrub.as_mut() {
                        // 8 logical px per schema step (I02).
                        *now = (*now + f64::from(e.delta.x) / 8.0 * step * k)
                            .clamp(min.unwrap_or(f64::MIN), max.unwrap_or(f64::MAX));
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    if let Some((path, axis, base, now)) = s.local.scrub.take() {
                        let _ = s.editor.end_field_gestures();
                        if e.phase == PointerPhase::Up && (now - base).abs() > f64::EPSILON {
                            commit_number(s, &path, axis, now);
                        }
                    }
                }
                PointerPhase::Hover => return false,
            }
            true
        })
}

/// Commits a number (one channel of a vector when `axis` is set) to every selected entity.
fn commit_number(s: &mut Studio, path: &str, axis: Option<usize>, value: f64) {
    let target = match axis {
        Some(a) => format!("{path}.{}", ["x", "y", "z", "w"][a.min(3)]),
        None => path.to_owned(),
    };
    commit(s, &target, json!(round(value)), "Set value");
}

/// Commits `value` at `path` to every selected entity (one undo step).
fn commit(s: &mut Studio, path: &str, value: Value, why: &str) {
    let commands = crate::inspector::edit_commands(&s.editor.state.selection, path, &value);
    s.apply(commands, why);
}

/// Six significant decimals, so scrubbing never writes float noise.
fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// A number shown compactly.
fn show(v: f64) -> String {
    let r = round(v);
    if r.fract() == 0.0 && r.abs() < 1e9 {
        format!("{r:.0}")
    } else {
        format!("{r}").trim_end_matches('0').to_owned()
    }
}

/// The field's editor by widget kind.
fn editor(s: &Studio, f: &Field) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let path = f.path.clone();
    if f.mixed {
        return text("—").text_size(12.0).color(c.muted_foreground.0);
    }
    match &f.widget {
        Widget::Number { .. } => {
            let shown = match &s.local.scrub {
                Some((p, None, _, now)) if *p == f.path => *now,
                _ => f.value.as_f64().unwrap_or(0.0),
            };
            number(
                l,
                &format!("field/{}", f.path),
                shown,
                f.unit.as_deref(),
                f.read_only,
                move |s, v| commit_number(s, &path, None, v),
            )
        }
        Widget::Vector {
            count,
            color: false,
            ..
        } => {
            let axes = ["X", "Y", "Z", "W"];
            let tints = [
                stillkit::Rgba::rgb8([240, 82, 82], 1.0),
                stillkit::Rgba::rgb8([94, 200, 108], 1.0),
                stillkit::Rgba::rgb8([80, 140, 255], 1.0),
                c.muted_foreground.0,
            ];
            let mut out = row().gap(4.0);
            for axis in 0..(*count).min(4) {
                let value = match &s.local.scrub {
                    Some((p, Some(a), _, now)) if *p == f.path && *a == axis => *now,
                    _ => f
                        .channels
                        .get(axis)
                        .and_then(Value::as_f64)
                        .or_else(|| f.value.get(axis).and_then(Value::as_f64))
                        .unwrap_or(0.0),
                };
                let path = f.path.clone();
                let tag = scrubbable(
                    el().w(14.0)
                        .h(22.0)
                        .align(Align::Center)
                        .justify(Justify::Center)
                        .child(
                            text(axes[axis])
                                .text_size(10.5)
                                .weight(700)
                                .color(tints[axis]),
                        ),
                    f.path.clone(),
                    Some(axis),
                    value,
                    0.01,
                    None,
                    None,
                );
                out = out.child(
                    row()
                        .grow(1.0)
                        .w(0.0)
                        .h(24.0)
                        .pad_x(2.0)
                        .align(Align::Center)
                        .radius(l.radius.xs)
                        .fill(c.input.0.with_alpha(0.35))
                        .border(1.0, c.border.0)
                        .child(tag)
                        .child(
                            number(
                                l,
                                &format!("field/{}/{axis}", f.path),
                                value,
                                None,
                                f.read_only,
                                move |s, v| commit_number(s, &path, Some(axis), v),
                            )
                            .grow(1.0)
                            .fill(stillkit::Rgba::TRANSPARENT)
                            .border(0.0, stillkit::Rgba::TRANSPARENT),
                        ),
                );
            }
            out
        }
        Widget::Vector { color: true, .. } => {
            let rgb: Vec<f64> = (0..3)
                .map(|i| {
                    f.value
                        .get(i)
                        .or_else(|| f.value.get(["r", "g", "b"][i]))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                })
                .collect();
            let to8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let swatch = stillkit::Rgba::rgb8([to8(rgb[0]), to8(rgb[1]), to8(rgb[2])], 1.0);
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(
                    el().w(36.0)
                        .h(20.0)
                        .radius(l.radius.xs)
                        .fill(swatch)
                        .border(1.0, c.separator_strong.0),
                )
                .child(
                    text(format!(
                        "#{:02X}{:02X}{:02X}",
                        to8(rgb[0]),
                        to8(rgb[1]),
                        to8(rgb[2])
                    ))
                    .text_size(11.5)
                    .family(mono())
                    .color(c.foreground.0),
                )
        }
        Widget::Bool => {
            let on = f.value.as_bool().unwrap_or(false);
            let p = f.path.clone();
            row().child(
                checkbox(l, format!("field/{}", f.path), &f.label, on)
                    .on_press(move |s: &mut Studio| commit(s, &p, json!(!on), "Toggle value")),
            )
        }
        Widget::Enum { options } => {
            let current = f.value.as_str().unwrap_or("").to_owned();
            let key = format!("field/{}", f.path);
            let open = s.local.select.as_deref() == Some(key.as_str());
            let k = key.clone();
            let mut trigger = select(l, Size::Sm, key.clone(), &current)
                .expanded(open)
                .on_press(move |s: &mut Studio| {
                    s.local.select = if s.local.select.as_deref() == Some(k.as_str()) {
                        None
                    } else {
                        Some(k.clone())
                    }
                });
            if open {
                let items = options
                    .iter()
                    .map(|o| {
                        let (p, v) = (f.path.clone(), o.clone());
                        menu_check(l, &format!("{key}/{o}"), o, *o == current).on_press(
                            move |s: &mut Studio| {
                                s.local.select = None;
                                commit(s, &p, json!(v), "Choose value");
                            },
                        )
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
        Widget::Text => {
            let p = f.path.clone();
            let shown = match &f.value {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            field(
                l,
                Size::Sm,
                format!("field/{}", f.path),
                None,
                "",
                &shown,
                move |x| x.on_submit(move |s: &mut Studio, t| commit(s, &p, json!(t), "Edit text")),
            )
        }
        Widget::Asset { .. } | Widget::Entity | Widget::Compound { .. } => {
            let shown = match &f.value {
                Value::String(s) => s.rsplit('/').next().unwrap_or(s).to_owned(),
                Value::Null => "None".to_owned(),
                Value::Array(a) => format!("{} items", a.len()),
                Value::Object(o) => format!("{} fields", o.len()),
                other => other.to_string(),
            };
            row()
                .h(24.0)
                .pad_x(8.0)
                .gap(6.0)
                .align(Align::Center)
                .radius(l.radius.xs)
                .fill(c.input.0.with_alpha(0.35))
                .border(1.0, c.border.0)
                .child(
                    icon(if matches!(f.widget, Widget::Asset { .. }) {
                        icons::PACKAGE
                    } else {
                        icons::LIST_TREE
                    })
                    .size(13.0)
                    .color(c.muted_foreground.0),
                )
                .child(text(shown).text_size(12.0).color(c.foreground.0).grow(1.0))
        }
    }
}

/// A number box: shows the value (with its unit), types a new one on Enter.
fn number(
    l: &ara_kit::look::Look,
    key: &str,
    value: f64,
    unit: Option<&str>,
    read_only: bool,
    commit: impl Fn(&mut Studio, f64) + 'static,
) -> El<Studio> {
    let shown = match unit {
        Some(u) if !u.is_empty() => format!("{} {u}", show(value)),
        _ => show(value),
    };
    let unit = unit.map(str::to_owned);
    let mut f = text_input(key.to_owned())
        .grow(1.0)
        .value(shown)
        .select_all_on_focus()
        .text_size(12.5)
        .text_align(stillkit::el::TextAlign::Start)
        .color(l.c.foreground.0)
        .on_submit(move |s: &mut Studio, t| {
            if let Ok(v) = crate::inspector::parse_number(t, unit.as_deref()) {
                commit(s, v);
            }
        });
    if read_only {
        f = f.read_only();
    }
    row()
        .h(24.0)
        .pad_x(6.0)
        .align(Align::Center)
        .radius(l.radius.xs)
        .fill(l.c.input.0.with_alpha(0.35))
        .border(1.0, l.c.border.0)
        .focus_within({
            let ring = l.c.ring.0;
            move |e| e.border(1.0, ring)
        })
        .child(f)
}

/// A schema name as a label: `emissiveStrength` reads "Emissive strength".
fn sentence(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else if c == '_' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}
