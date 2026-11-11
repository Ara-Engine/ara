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
    Align, Axis, El, Justify, PointerPhase, col, el, icon, menu, popover, row, text, text_input,
};

use super::Studio;
use crate::inspector::{Field, Provenance, Widget};

/// The label column's share of a row (Unity's 40/60 split).
const LABEL: f32 = 0.4;
/// A field row's height.
const ROW: f32 = super::style::ROW;

pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    // An asset picked in Assets has the Inspector until an entity is picked again (Unity).
    if s.local.inspect_asset
        && let Some(path) = s.editor.state.asset_path.as_deref()
    {
        return super::asset_inspector::view(s, path);
    }
    let selection = s.inspected();
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
    // The search keeps fields whose name, path or component matches (Unreal's Details).
    let query = s.local.inspector_search.trim().to_lowercase();
    let matches = |f: &&Field| {
        query.is_empty()
            || f.label.to_lowercase().contains(&query)
            || f.path.to_lowercase().contains(&query)
            || spaced(&f.label).to_lowercase().contains(&query)
    };
    let mut by_component: BTreeMap<String, Vec<&Field>> = BTreeMap::new();
    let mut order = Vec::new();
    for f in fields.iter().filter(matches) {
        let component = f.path.split('.').next().unwrap_or("").to_owned();
        if !by_component.contains_key(&component) {
            order.push(component.clone());
        }
        by_component.entry(component).or_default().push(f);
    }
    let mut page = col()
        .pad(4.0)
        .gap(3.0)
        .child(header(s))
        .children(prefab_bar(s))
        .children(super::running::section(s));
    // Debug: the record itself instead of its fields.
    if let Some(raw) = raw_record(s) {
        return col()
            .grow(1.0)
            .min_h(0.0)
            .scroll(Axis::Vertical)
            .child(page.child(raw).child(el().h(12.0)));
    }
    page = page.child(search_row(s));
    if order.is_empty() && !query.is_empty() {
        page = page.child(
            row().pad(10.0).child(
                text(format!(
                    "No properties match “{}”",
                    s.local.inspector_search.trim()
                ))
                .text_size(12.0)
                .color(l.c.muted_foreground.0),
            ),
        );
    }
    // The order Move Up and Move Down set, then Transform first, then record order.
    sort_components(s, &mut order);
    let last = order.last().cloned();
    for component in order {
        let section = section(s, &component, &by_component[&component]);
        page = page.child(section.children(drop_line(s, &component, last.as_ref())));
    }
    page = page.children(paste_as_new(s));
    page = page.child(super::components::button_and_picker(s));
    col()
        .grow(1.0)
        .min_h(0.0)
        .scroll(Axis::Vertical)
        .child(page.child(el().h(12.0)))
}

/// The selection in one row: what it is, its name (editable for one entity) and its id.
fn header(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let selection = s.inspected();
    // What it is, as the Hierarchy shows it (a light, a camera, a mesh…).
    let glyph = match selection {
        [id] => s
            .resolved
            .get(id)
            .map_or(icons::BOX, |e| super::hierarchy::glyph(&e.record)),
        _ => icons::BOXES,
    };
    let locked = s.local.inspector_lock.is_some();
    let lock = super::scene_overlays::toggle(
        l,
        if locked {
            icons::LOCK
        } else {
            icons::LOCK_OPEN
        },
        if locked {
            "Unlock: follow the selection again"
        } else {
            "Lock: keep showing these while you select others"
        },
        locked,
    )
    .key("inspector/lock")
    .on_press(|s: &mut Studio| {
        s.local.inspector_lock = match s.local.inspector_lock {
            Some(_) => None,
            None => Some(s.editor.state.selection.clone()),
        }
    });
    let mut head = row().h(28.0).gap(8.0).align(Align::Center).child(
        el().size(24.0)
            .radius(6.0)
            .fill(c.primary.0.with_alpha(0.14))
            .align(Align::Center)
            .justify(Justify::Center)
            .child(icon(glyph).size(14.0).color(c.primary.0)),
    );
    if let [id] = selection {
        let entity = id.clone();
        head = head
            .child(
                field(l, Size::Sm, "inspector/name", None, "Name", &s.name(id), move |f| {
                    f.text_size(13.0).on_submit(move |s: &mut Studio, t| {
                        let name = t.trim().to_owned();
                        if !name.is_empty() {
                            s.apply(vec![json!({"cmd":"property.set","args":{"entity":entity,"path":"name","value":name}})], "Rename entity");
                        }
                    })
                })
                .w(0.0)
                .grow(1.0),
            )
            .child(text(id.clone()).text_size(10.5).family(mono()).color(c.muted_foreground.0.with_alpha(0.75)).tooltip("Entity id: what agents and references use"));
    } else {
        head = head
            .child(
                text(format!("{} entities", selection.len()))
                    .text_size(13.0)
                    .weight(600)
                    .color(c.foreground.0)
                    .grow(1.0),
            )
            .child(
                text("Differing values show —")
                    .text_size(11.0)
                    .color(c.muted_foreground.0),
            );
    }
    let raw = s.local.inspector_raw;
    let debug = super::scene_overlays::toggle(
        l,
        icons::BRACES,
        if raw {
            "Back to the fields"
        } else {
            "Debug: the record as the scene resolves it"
        },
        raw,
    )
    .key("inspector/raw")
    .on_press(|s: &mut Studio| s.local.inspector_raw = !s.local.inspector_raw);
    head.child(debug).child(lock)
}

/// Unity's Inspector Debug mode: one entity's resolved record (prefab values included)
/// as JSON, read-only, with Copy; `None` unless Debug is on for a single entity.
fn raw_record(s: &Studio) -> Option<El<Studio>> {
    if !s.local.inspector_raw {
        return None;
    }
    let [id] = s.inspected() else {
        return None;
    };
    let record = &s.resolved.get(id)?.record;
    // The canonical writer's text, as files and agents read it (short arrays inline).
    let copy = ara_json::to_canonical_string(&serde_json::to_value(record).ok()?);
    let shown = copy.trim_end().to_owned();
    let l = &s.look;
    let c = &l.c;
    Some(
        col()
            .key("inspector/record")
            .gap(4.0)
            .child(
                row()
                    .gap(6.0)
                    .align(Align::Center)
                    .child(
                        text("Resolved record, read-only; edits go through the fields")
                            .text_size(11.0)
                            .color(c.muted_foreground.0)
                            .grow(1.0),
                    )
                    .child(
                        icon_button(l, Tone::Ghost, Size::Sm, icons::COPY, "Copy the record")
                            .key("inspector/record/copy")
                            .on_press(move |s: &mut Studio| {
                                s.local.copy = Some(copy.clone());
                                super::note(s, "Copied the record");
                            }),
                    ),
            )
            .child(
                col()
                    .pad(8.0)
                    .radius(l.radius.sm)
                    .fill(c.code_background.0)
                    // A line each, indents kept (text runs collapse spaces and newlines).
                    .children(shown.lines().map(|line| {
                        let indent = line.len() - line.trim_start().len();
                        text(format!("{}{}", "\u{a0}".repeat(indent), line.trim_start()))
                            .text_size(11.0)
                            .family(mono())
                            .color(c.code_foreground.0)
                    })),
            ),
    )
}

/// A prefab instance's bar (Unity's Prefab header): where it comes from, Open, and the
/// Overrides menu listing each overridden field to apply or revert, then all of them.
fn prefab_bar(s: &Studio) -> Option<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let [entity] = s.inspected() else {
        return None;
    };
    let found = crate::prefab::instance(s.editor.host.store(), &s.editor.state.doc, entity)?;
    let run = |id: &'static str, target: crate::catalog::Target| {
        move |s: &mut Studio| {
            s.local.select = None;
            super::commands::run(s, id, &target);
        }
    };
    const MENU: &str = "inspector/overrides";
    let open = s.local.select.as_deref() == Some(MENU);
    let count = found.overrides.len();
    let mut overrides = button(
        l,
        Tone::Ghost,
        Size::Sm,
        None,
        &match count {
            0 => "No overrides".to_owned(),
            1 => "1 override".to_owned(),
            n => format!("{n} overrides"),
        },
    )
    .key(MENU)
    .expanded(open)
    .on_press(|s: &mut Studio| {
        s.local.select = match s.local.select.as_deref() == Some(MENU) {
            true => None,
            false => Some(MENU.into()),
        }
    });
    if open {
        let mut items: Vec<El<Studio>> = Vec::new();
        for path in &found.overrides {
            let field = crate::catalog::Target::Field(path.clone());
            items.push(
                row()
                    .key(format!("{MENU}/{path}"))
                    .h(26.0)
                    .pad_x(8.0)
                    .gap(4.0)
                    .align(Align::Center)
                    .child(el().w(2.0).h(14.0).radius(1.0).fill(c.info.0))
                    .child(
                        text(path.clone())
                            .text_size(11.5)
                            .family(mono())
                            .color(c.foreground.0)
                            .wrap(1)
                            .w(0.0)
                            .grow(1.0),
                    )
                    .child(
                        icon_button(l, Tone::Ghost, Size::Sm, icons::UPLOAD, "Apply to prefab")
                            .key(format!("{MENU}/{path}/apply"))
                            .on_press(run("prefab.apply_override", field.clone())),
                    )
                    .child(
                        icon_button(l, Tone::Ghost, Size::Sm, icons::UNDO_2, "Revert")
                            .key(format!("{MENU}/{path}/revert"))
                            .on_press(run("prefab.revert_override", field)),
                    ),
            );
        }
        if count > 0 {
            items.push(menu_separator(l));
        }
        let all = crate::catalog::Target::Entity(entity.clone());
        items.push(enabled(
            menu_item(
                l,
                &format!("{MENU}/apply"),
                Some(icons::UPLOAD),
                "Apply All",
                None,
                false,
            )
            .on_press(run("prefab.apply_all", all.clone())),
            count > 0,
        ));
        items.push(enabled(
            menu_item(
                l,
                &format!("{MENU}/revert"),
                Some(icons::UNDO_2),
                "Revert All",
                None,
                true,
            )
            .on_press(run("prefab.revert_all", all)),
            count > 0,
        ));
        overrides = overrides.overlay(
            menu(menu_panel(l, items).min_w(300.0))
                .on_dismiss(|s: &mut Studio| s.local.select = None),
        );
    }
    let target = crate::catalog::Target::Entity(entity.clone());
    Some(
        row()
            .key("inspector/prefab")
            .h(28.0)
            .pad_x(8.0)
            .gap(6.0)
            .align(Align::Center)
            .radius(super::style::RADIUS)
            .fill(c.info.0.with_alpha(0.10))
            .child(icon(icons::PACKAGE).size(13.0).color(c.info_foreground.0))
            .child(
                text(found.prefab_path.clone())
                    .text_size(11.5)
                    .family(mono())
                    .color(c.foreground.0)
                    .wrap(1)
                    .w(0.0)
                    .grow(1.0)
                    .tooltip("The prefab this instance follows"),
            )
            .child(
                button(l, Tone::Ghost, Size::Sm, Some(icons::PACKAGE_OPEN), "Open")
                    .key("inspector/prefab/open")
                    .tooltip("Edit the prefab itself; every instance follows")
                    .on_press(run("prefab.open", target)),
            )
            .child(overrides),
    )
}

/// The property search under the header.
fn search_row(s: &Studio) -> El<Studio> {
    row().child(
        search(
            &s.look,
            "inspector/search",
            "Search properties",
            &s.local.inspector_search,
            None,
            |f| f.on_input(|s: &mut Studio, t| s.local.inspector_search = t.to_owned()),
            |s: &mut Studio| s.local.inspector_search.clear(),
        )
        .grow(1.0),
    )
}

/// The copied component, offered as a new one on entities that lack it (Unity's Paste
/// Component As New).
fn paste_as_new(s: &Studio) -> Option<El<Studio>> {
    let (kind, value) = s.local.component_clipboard.as_ref()?;
    let lacking = s.inspected().iter().any(|id| {
        s.resolved
            .get(id)
            .is_some_and(|e| e.record.get(kind).is_none())
    });
    if !lacking {
        return None;
    }
    let (kind, value) = (kind.clone(), value.clone());
    Some(
        row().justify(Justify::Center).child(
            button(
                &s.look,
                Tone::Ghost,
                Size::Sm,
                Some(icons::CLIPBOARD_PASTE),
                &format!("Paste {} as new", spaced(&kind)),
            )
            .key("inspector/paste-new")
            .on_press(move |s: &mut Studio| {
                let commands = s
                    .inspected()
                    .iter()
                    .filter(|id| s.resolved.get(*id).is_some_and(|e| e.record.get(&kind).is_none()))
                    .map(|e| json!({"cmd": "component.add", "args": {"entity": e, "component": kind, "value": value}}))
                    .collect();
                s.apply(commands, &format!("Paste {kind}"));
            }),
        ),
    )
}

/// Resets `component` on every inspected entity (prefab instances back to their prefab),
/// or with `paste`, sets every field from those values (defaults where they have none),
/// as one undo step.
pub(super) fn component_values(s: &mut Studio, component: &str, paste: Option<&Value>) {
    let db = s.editor.host.store().schema();
    let Some(ara_schema::Shape::Struct { fields, .. }) = db.get(component).map(|t| &t.shape) else {
        return;
    };
    let mut commands = Vec::new();
    for desc in fields.iter().filter(|f| !f.editor.read_only) {
        let path = format!("{component}.{}", desc.name);
        match paste {
            None => commands.extend(s.inspected().iter().map(
                |entity| json!({"cmd": "editor.property.reset", "args": {"entity": entity, "path": path}}),
            )),
            Some(values) => {
                let value = values
                    .get(&desc.name)
                    .cloned()
                    .unwrap_or_else(|| db.field_default(desc));
                commands.extend(crate::inspector::edit_commands(s.inspected(), &path, &value));
            }
        }
    }
    let why = format!(
        "{} {component}",
        if paste.is_some() { "Paste" } else { "Reset" }
    );
    s.apply(commands, &why);
}

/// A component's section: a header (fold, icon, name) over its fields.
fn section(s: &Studio, component: &str, fields: &[&Field]) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let folded = s.local.folded.contains(component) && s.local.inspector_search.trim().is_empty();
    let name = component.to_owned();
    let t = super::style::tones(l);
    let hover = t.hover;
    // A band a step above the panel, the fields flush under it: no card, no rule.
    let head = row()
        .key(format!("component/{component}"))
        .h(22.0)
        .pad_x(4.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(t.band)
        .hovered(move |e| e.fill(hover))
        .color(c.foreground.0)
        .cascade_ink()
        .label(component.to_owned())
        .expanded(!folded)
        .on_pointer({
            let name = name.clone();
            move |s: &mut Studio, e, _| drag_component(s, &name, e)
        })
        .on_press(move |s: &mut Studio| {
            // The release that ends a reorder is not a fold.
            if std::mem::take(&mut s.local.component_dropped) {
                return;
            }
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
            .size(11.0)
            .color(c.muted_foreground.0),
        )
        .child(
            icon(component_icon(component))
                .size(13.0)
                .color(c.info_foreground.0),
        )
        .child(
            text(spaced(component))
                .text_size(super::style::TEXT)
                .weight(600)
                .grow(1.0),
        )
        .children(about(s, component))
        .child(component_menu(s, component));
    let mut out = col().child(head);
    if !folded {
        let mut body = col().pad_x(6.0).pad_y(3.0).gap(1.0);
        for f in fields {
            body = body.child(field_row(s, f));
            if let Widget::Compound { ty, .. } = &f.widget {
                body = body.children(super::compound::rows(s, f, ty));
            }
        }
        out = out.child(body);
    }
    out
}

/// What a component is, from its schema doc, on hover over an ⓘ in its header (Unity's
/// help button); `None` for a component the schema does not know.
fn about(s: &Studio, component: &str) -> Option<El<Studio>> {
    let schema = s.editor.host.store().schema();
    let doc = schema
        .get(component)?
        .doc
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!doc.is_empty()).then(|| {
        el().key(format!("component/{component}/about"))
            .size(18.0)
            .align(Align::Center)
            .justify(stillkit::el::Justify::Center)
            .label(format!("About {}", spaced(component)))
            .description(doc.clone())
            .tooltip(doc)
            .child(
                icon(icons::INFO)
                    .size(12.0)
                    .color(s.look.c.muted_foreground.0),
            )
    })
}

pub(super) fn component_icon(component: &str) -> stillkit::icons::Icon {
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
    let mut label = row()
        .w(0.0)
        .grow(LABEL)
        .min_w(84.0)
        .h(ROW)
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
    let (path, value) = (f.path.clone(), f.value.to_string());
    let mut out = row()
        .gap(6.0)
        .align(Align::Center)
        .min_h(ROW)
        .on_context_menu(move |s: &mut Studio, x, y| {
            s.local.field_menu = Some((path.clone(), value.clone(), (x, y)))
        });
    if let Some((_, value, (x, y))) = s.local.field_menu.as_ref().filter(|(p, ..)| *p == f.path) {
        out = out.overlay(field_menu(s, f, value).at(*x, *y));
    }
    out.child(label)
        .child(editor(s, f).w(0.0).grow(1.0 - LABEL).min_w(0.0))
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
    // A drag handle: the field beside it is the keyboard's stop, as in Unity.
    label
        .skip_tab()
        .cursor(Cursor::ResizeHorizontal)
        .on_pointer(move |s: &mut Studio, e, _| {
            match e.phase {
                PointerPhase::Down => {
                    for id in s.inspected().to_vec() {
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
                        let now = *now;
                        // The whole value previews in the Scene and the Inspector as it moves.
                        s.local.draft = Some((path.clone(), whole(s, &path, axis, now)));
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    if let Some((path, axis, base, now)) = s.local.scrub.take() {
                        s.local.draft = None;
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

/// While a gizmo drags the first selected entity, its Transform field at `path` as the drag
/// has it (position, rotation in degrees, scale), so the Inspector moves with the gizmo.
fn dragged(s: &Studio, path: &str) -> Option<[f64; 3]> {
    let gesture = s.editor.gesture.as_ref()?;
    let matrix = gesture.current.get(s.inspected().first()?)?;
    let (scale, rotation, position) = matrix.to_scale_rotation_translation();
    let v = match path {
        "Transform.position" => position,
        "Transform.scale" => scale,
        "Transform.rotation" => {
            let (x, y, z) = rotation.to_euler(glam::EulerRot::XYZ);
            glam::Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
        }
        _ => return None,
    };
    Some(v.to_array().map(f64::from))
}

/// The whole value of `path` with `value` set: the number itself, or the first selected
/// entity's vector with channel `axis` replaced (every channel scaled, for a linked scale).
fn whole(s: &Studio, path: &str, axis: Option<usize>, value: f64) -> Value {
    let Some(a) = axis else {
        return json!(round(value));
    };
    let scale = path.ends_with(".scale");
    let current: Vec<f64> = s
        .inspected()
        .first()
        .and_then(|id| s.resolved.get(id))
        .and_then(|e| {
            let (component, field) = path.split_once('.')?;
            e.record.get(component)?.get(field)?.as_array().cloned()
        })
        .map(|v| v.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
        // An unauthored scale is its default, one on every axis.
        .unwrap_or_else(|| vec![if scale { 1.0 } else { 0.0 }; 3]);
    let base = current.get(a).copied().unwrap_or(0.0);
    let linked = scale && s.local.scale_linked;
    let out = current.iter().enumerate().map(|(i, v)| {
        let v = match (i == a, linked) {
            (true, _) => value,
            (false, true) if base.abs() > 1e-9 => v * value / base,
            (false, true) => value,
            (false, false) => *v,
        };
        json!(round(v))
    });
    Value::Array(out.collect())
}

/// The Inspector's live preview for the Scene: the field being scrubbed, as its whole value
/// on every selected entity (`SceneSync::preview` draws it until the commit lands).
pub(super) struct Preview {
    key: BTreeMap<String, String>,
    draft: Option<(String, Value)>,
    selection: Vec<String>,
}

impl Preview {
    /// The preview of `s`'s current draft (empty when nothing is being scrubbed).
    pub fn of(s: &Studio) -> Self {
        let draft = s.local.draft.clone();
        let key = draft
            .iter()
            .map(|(path, value)| (path.clone(), value.to_string()))
            .collect();
        Self {
            key,
            draft,
            selection: s.inspected().to_vec(),
        }
    }
}

impl crate::studio::scene_sync::Drafts for Preview {
    fn drafts(&self) -> &BTreeMap<String, String> {
        &self.key
    }
    fn draft_records(
        &self,
        _: &crate::editor::Editor,
        view: &crate::world::DocumentView,
    ) -> Option<BTreeMap<String, ara_doc::Record>> {
        use crate::world::EditWorldView;
        let (path, value) = self.draft.as_ref()?;
        let (component, field) = path.split_once('.')?;
        let records: BTreeMap<_, _> = self
            .selection
            .iter()
            .filter_map(|id| {
                let mut record = view.entities().get(id)?.record.clone();
                record
                    .entry(component.to_owned())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()?
                    .insert(field.to_owned(), value.clone());
                Some((id.clone(), record))
            })
            .collect();
        (!records.is_empty()).then_some(records)
    }
}

/// Axis `axis` of vector `path` when every inspected entity has the same value there
/// (`None` where they differ: the field shows "—" for that axis).
fn shared_axis(s: &Studio, path: &str, axis: usize) -> Option<f64> {
    let (component, field) = path.split_once('.')?;
    let mut values = s.inspected().iter().map(|id| {
        s.resolved
            .get(id)
            .and_then(|e| e.record.get(component)?.get(field)?.get(axis)?.as_f64())
    });
    let first = values.next()??;
    values.all(|v| v == Some(first)).then_some(first)
}

/// Commits a number (one channel of a vector when `axis` is set) to every selected entity.
pub(super) fn commit_number(s: &mut Studio, path: &str, axis: Option<usize>, value: f64) {
    // A linked scale moves every axis by the same ratio (equal values from a zero axis),
    // when the selection shares one scale; scales that differ set the one axis.
    let shared = (0..3).all(|a| shared_axis(s, path, a).is_some()) || s.inspected().len() < 2;
    if let Some(a) = axis
        && s.local.scale_linked
        && shared
        && path.ends_with(".scale")
        && let Some(id) = s.inspected().first()
    {
        // An unauthored scale is its default, one on every axis.
        let current = s
            .resolved
            .get(id)
            .and_then(|e| {
                let (component, field) = path.split_once('.')?;
                e.record.get(component)?.get(field)?.as_array().cloned()
            })
            .unwrap_or_else(|| vec![json!(1.0); 3]);
        let old: Vec<f64> = current.iter().map(|v| v.as_f64().unwrap_or(1.0)).collect();
        let base = old.get(a).copied().unwrap_or(1.0);
        let scaled: Vec<Value> = old
            .iter()
            .map(|v| {
                json!(round(if base.abs() < 1e-9 {
                    value
                } else {
                    v * value / base
                }))
            })
            .collect();
        commit(s, path, Value::Array(scaled), "Scale uniformly");
        return;
    }
    let target = match axis {
        Some(a) => format!("{path}.{}", ["x", "y", "z", "w"][a.min(3)]),
        None => path.to_owned(),
    };
    commit(s, &target, json!(round(value)), "Set value");
}

/// Commits `value` at `path` to every selected entity (one undo step).
fn commit(s: &mut Studio, path: &str, value: Value, why: &str) {
    let commands = crate::inspector::edit_commands(s.inspected(), path, &value);
    s.apply(commands, why);
}

/// Six significant decimals, so scrubbing never writes float noise.
fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// A number shown compactly: at most three decimals, as Unreal's transform fields, so it
/// fits a vector field. Typing a value keeps its full precision.
fn show(v: f64) -> String {
    let r = (v * 1e3).round() / 1e3;
    if r.fract() == 0.0 && r.abs() < 1e9 {
        format!("{:.0}", r + 0.0)
    } else {
        format!("{r:.3}").trim_end_matches('0').to_owned()
    }
}

/// The field's editor by widget kind.
fn editor(s: &Studio, f: &Field) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let path = f.path.clone();
    // Values that differ across the selection show "—" and, as in Unity, take a new value
    // for all of them; a vector, axis by axis (each entity keeps its other axes).
    let scalar = matches!(
        f.widget,
        Widget::Number { .. }
            | Widget::Bool
            | Widget::Enum { .. }
            | Widget::Text
            | Widget::Vector { color: false, .. }
    );
    if f.mixed && !scalar {
        return text("—").text_size(12.0).color(c.muted_foreground.0);
    }
    match &f.widget {
        Widget::Number { .. } => {
            let shown = match &s.local.draft {
                Some((p, v)) if *p == f.path => v.as_f64().unwrap_or(0.0),
                _ if f.mixed => f64::NAN,
                _ => f.value.as_f64().unwrap_or(0.0),
            };
            number(
                l,
                &format!("field/{}", f.path),
                &sentence(&f.label),
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
                let value = match (&s.local.draft, dragged(s, &f.path)) {
                    (_, Some(v)) => v[axis.min(2)],
                    (Some((p, v)), _) if *p == f.path => {
                        v.get(axis).and_then(Value::as_f64).unwrap_or(0.0)
                    }
                    _ if f.mixed => shared_axis(s, &f.path, axis).unwrap_or(f64::NAN),
                    _ => f
                        .channels
                        .get(axis)
                        .and_then(Value::as_f64)
                        .or_else(|| f.value.get(axis).and_then(Value::as_f64))
                        .unwrap_or(0.0),
                };
                let path = f.path.clone();
                let tag = scrubbable(
                    el().key(format!("field/{}/{axis}/scrub", f.path))
                        .w(14.0)
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
                        .h(ROW)
                        .pad_x(2.0)
                        .align(Align::Center)
                        .radius(l.radius.xs)
                        .fill(l.well())
                        .child(tag)
                        .child(
                            number(
                                l,
                                &format!("field/{}/{axis}", f.path),
                                // Announced by its field and axis ("Position X").
                                &format!("{} {}", sentence(&f.label), axes[axis]),
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
            // Scale: a chain keeps the proportions (I06), off by default as in Unity.
            if f.path.ends_with(".scale") {
                let linked = s.local.scale_linked;
                out = out.child(
                    toggle_button(
                        l,
                        Size::Sm,
                        if linked { icons::LINK } else { icons::UNLINK },
                        if linked {
                            "Proportions kept: click to edit axes alone"
                        } else {
                            "Keep proportions"
                        },
                        linked,
                    )
                    .size(ROW)
                    .key(format!("field/{}/link", f.path))
                    .on_press(|s: &mut Studio| s.local.scale_linked = !s.local.scale_linked),
                );
            }
            out
        }
        Widget::Vector {
            color: true,
            count,
            hdr,
            ..
        } => color_field(s, f, *count, *hdr),
        Widget::Bool => {
            // Mixed reads as off: a press turns it on for all.
            let on = !f.mixed && f.value.as_bool().unwrap_or(false);
            let p = f.path.clone();
            row().child(
                checkbox(l, format!("field/{}", f.path), &f.label, on)
                    .on_press(move |s: &mut Studio| commit(s, &p, json!(!on), "Toggle value")),
            )
        }
        Widget::Enum { options } => {
            let current = match f.mixed {
                true => "—".to_owned(),
                false => f.value.as_str().unwrap_or("").to_owned(),
            };
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
            let mixed = f.mixed;
            let shown = match &f.value {
                _ if mixed => String::new(),
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            field(
                l,
                Size::Sm,
                format!("field/{}", f.path),
                None,
                if mixed { "—" } else { "" },
                &shown,
                move |x| {
                    x.on_submit(move |s: &mut Studio, t| {
                        // Enter on an untouched mixed field keeps each value.
                        if !(mixed && t.is_empty()) {
                            commit(s, &p, json!(t), "Edit text")
                        }
                    })
                },
            )
        }
        Widget::Asset { .. } | Widget::Entity if !f.read_only => {
            super::references::field(s, f, commit)
        }
        Widget::Compound { ty, .. } => super::compound::head(s, f, ty),
        // Read-only references show what they point at.
        Widget::Asset { .. } | Widget::Entity => {
            let shown = match &f.value {
                Value::String(s) => s.rsplit('/').next().unwrap_or(s).to_owned(),
                Value::Null => "None".to_owned(),
                other => other.to_string(),
            };
            row()
                .h(ROW)
                .pad_x(8.0)
                .gap(6.0)
                .align(Align::Center)
                .radius(l.radius.xs)
                .fill(l.well())
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

/// A colour field: a swatch that opens the picker (live in the Scene while dragging, one
/// undo step on release) and the hex value, typed on Enter. An HDR colour picks its base
/// colour and keeps its intensity (its brightest channel above one).
fn color_field(s: &Studio, f: &Field, count: usize, hdr: bool) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let value = match &s.local.draft {
        Some((p, v)) if *p == f.path => v.clone(),
        _ => f.value.clone(),
    };
    let channel = |i: usize| {
        value
            .get(i)
            .or_else(|| value.get(["r", "g", "b", "a"][i]))
            .and_then(Value::as_f64)
            .unwrap_or(if i == 3 { 1.0 } else { 0.0 }) as f32
    };
    let rgba = stillkit::Rgba::new(channel(0), channel(1), channel(2), channel(3));
    let intensity = if hdr {
        rgba.r.max(rgba.g).max(rgba.b).max(1.0)
    } else {
        1.0
    };
    let base = stillkit::Rgba::new(
        rgba.r / intensity,
        rgba.g / intensity,
        rgba.b / intensity,
        rgba.a,
    );
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let hex = format!("#{:02X}{:02X}{:02X}", to8(base.r), to8(base.g), to8(base.b));
    let key = format!("field/{}/swatch", f.path);
    let open = s.local.picker.as_ref().is_some_and(|(p, _)| *p == f.path);
    // The value in the field's own shape (an array or `r g b a` keys), at its intensity.
    let shaped = {
        let template = f.value.clone();
        move |c: stillkit::Rgba| {
            let channels = [c.r * intensity, c.g * intensity, c.b * intensity, c.a];
            let n = count.clamp(3, 4);
            match &template {
                Value::Object(_) => Value::Object(
                    ["r", "g", "b", "a"][..n]
                        .iter()
                        .zip(channels)
                        .map(|(k, v)| ((*k).to_owned(), json!(round(f64::from(v)))))
                        .collect(),
                ),
                _ => Value::Array(
                    channels[..n]
                        .iter()
                        .map(|v| json!(round(f64::from(*v))))
                        .collect(),
                ),
            }
        }
    };
    let path = f.path.clone();
    let mut swatch = row()
        .key(key.clone())
        .w(36.0)
        .h(ROW - 2.0)
        .radius(super::style::RADIUS)
        .fill(base.with_alpha(1.0))
        .label("Pick colour")
        .tooltip("Pick colour")
        .expanded(open)
        .on_press(move |s: &mut Studio| {
            s.local.picker = match &s.local.picker {
                Some((p, _)) if *p == path => None,
                _ => Some((path.clone(), stillkit_kit::Hsva::from_rgba(base))),
            };
        });
    if open && let Some((_, hsva)) = &s.local.picker {
        let (path, commit_path, shaped_commit) = (f.path.clone(), f.path.clone(), shaped.clone());
        let picker = stillkit_kit::ColorPicker::new(*hsva)
            .alpha(count >= 4)
            .width(232.0)
            .on_change(move |s: &mut Studio, picked, done| {
                let value = shaped(picked.rgba());
                s.local.picker = Some((path.clone(), picked));
                if done {
                    s.local.draft = None;
                    commit(s, &path, value, "Pick colour");
                } else {
                    s.local.draft = Some((path.clone(), value));
                }
            })
            .build();
        let hex_field = field(
            l,
            Size::Sm,
            format!("field/{}/hex", f.path),
            Some(icons::HASH),
            "",
            hex.trim_start_matches('#'),
            move |x| {
                x.on_submit(move |s: &mut Studio, t| {
                    if let Some(rgb) = parse_hex(t) {
                        let picked = stillkit::Rgba::new(rgb[0], rgb[1], rgb[2], base.a);
                        s.local.picker =
                            Some((commit_path.clone(), stillkit_kit::Hsva::from_rgba(picked)));
                        commit(s, &commit_path, shaped_commit(picked), "Set colour");
                    }
                })
            },
        );
        let panel = super::style::float(l, col())
            .pad(10.0)
            .gap(10.0)
            .radius(l.radius.md)
            .child(picker)
            .child(hex_field);
        swatch = swatch.overlay(popover(panel).on_dismiss(|s: &mut Studio| {
            s.local.picker = None;
            s.local.draft = None;
        }));
    }
    row()
        .gap(8.0)
        .align(Align::Center)
        .child(swatch)
        .child(
            text(hex)
                .text_size(11.5)
                .family(mono())
                .color(c.foreground.0),
        )
        .children(hdr.then(|| {
            text(format!("× {intensity:.2}"))
                .text_size(11.0)
                .color(c.muted_foreground.0)
        }))
}

/// `RRGGBB` or `RGB` (with or without `#`) as channels from 0 to 1.
fn parse_hex(text: &str) -> Option<[f32; 3]> {
    let t = text.trim().trim_start_matches('#');
    let full: String = match t.len() {
        3 => t.chars().flat_map(|c| [c, c]).collect(),
        6 => t.to_owned(),
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(full.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?].map(|b| f32::from(b) / 255.0))
}

/// A number box announced as `name`: the value to edit (NaN shows "—", values that differ
/// across the selection), its unit after it as a quiet label (not part of what you type, as
/// in Unreal); a new value on Enter.
fn number(
    l: &ara_kit::look::Look,
    key: &str,
    name: &str,
    value: f64,
    unit: Option<&str>,
    read_only: bool,
    commit: impl Fn(&mut Studio, f64) + 'static,
) -> El<Studio> {
    // NaN: values that differ across the selection.
    let shown = if value.is_nan() {
        "—".to_owned()
    } else {
        show(value)
    };
    let unit = unit.filter(|u| !u.is_empty()).map(str::to_owned);
    let label = unit
        .clone()
        .map(|u| text(u).text_size(10.5).color(l.c.muted_foreground.0));
    let mut f = text_input(key.to_owned())
        .label(name.to_owned())
        .focus_ring(false)
        .grow(1.0)
        .value(shown.clone())
        .select_all_on_focus()
        .text_size(12.5)
        .text_align(stillkit::el::TextAlign::Start)
        .color(l.c.foreground.0)
        .on_submit(move |s: &mut Studio, t| {
            // The box shows a rounded value: Enter on it untouched keeps the exact one.
            if t == shown {
                return;
            }
            if let Ok(v) = crate::inspector::parse_number(t, unit.as_deref()) {
                commit(s, v);
            }
        });
    if read_only {
        f = f.read_only();
    }
    row()
        .h(ROW)
        .pad_x(6.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(l.well())
        .border(1.0, stillkit::Rgba::TRANSPARENT)
        .focus_within({
            let ring = l.c.ring.0;
            move |e| e.border(1.0, ring)
        })
        .child(f)
        .children(label)
}

/// A field's menu: Reset to Default (when it has its own value), Copy Value, Copy Path.
/// The inspected entities' field `path`, as the Inspector shows it.
pub(super) fn field_of(s: &Studio, path: &str) -> Option<Field> {
    shown_fields(s).into_iter().find(|f| f.path == path)
}

/// The inspected entities' editable asset fields (drop targets for a dragged asset).
pub(super) fn asset_fields(s: &Studio) -> Vec<String> {
    shown_fields(s)
        .into_iter()
        .filter(|f| matches!(f.widget, Widget::Asset { .. }) && !f.read_only)
        .map(|f| f.path)
        .collect()
}

/// Every field the Inspector shows for the inspected entities.
fn shown_fields(s: &Studio) -> Vec<Field> {
    let Some(doc) = s.editor.host.store().workspace().doc(&s.editor.state.doc) else {
        return Vec::new();
    };
    let changes = s.editor.agent_fields();
    crate::inspector::fields(
        s.editor.host.store().schema(),
        doc,
        &s.resolved,
        s.inspected(),
        &changes,
    )
}

/// Why field `f` has its value: authored here, the schema's default, its prefab's, an
/// override of it, or an agent's change (and what it was before).
pub(super) fn why(s: &Studio, f: &Field) -> String {
    let prefab = || {
        s.inspected()
            .first()
            .and_then(|e| crate::prefab::instance(s.editor.host.store(), &s.editor.state.doc, e))
            .map(|i| i.prefab_path)
            .unwrap_or_else(|| "its prefab".into())
    };
    let mut why = match f.provenance {
        Provenance::Default => "Not set: the schema's default applies.".to_owned(),
        Provenance::Prefab => format!("From {}.", prefab()),
        Provenance::Override => format!("Overridden here; {} has another value.", prefab()),
        Provenance::Authored => "Set on this entity in the scene.".to_owned(),
        Provenance::AgentChanged => "Changed by an agent; review it in Agents.".to_owned(),
    };
    if let Some(was) = &f.was {
        why.push_str(&format!(" It was {was} before."));
    }
    why
}

fn field_menu(s: &Studio, f: &Field, value: &str) -> stillkit::el::Overlay<Studio> {
    let l = &s.look;
    let (path, provenance) = (f.path.as_str(), f.provenance);
    let (reset, copy_value, copy_path) = (path.to_owned(), value.to_owned(), path.to_owned());
    // Why it has this value leads the menu (Unreal's property source).
    let mut items = vec![
        row()
            .key(format!("field/{path}/why"))
            .max_w(280.0)
            .pad_x(8.0)
            .pad_y(6.0)
            .child(
                text(why(s, f))
                    .text_size(11.5)
                    .wrap(3)
                    .color(l.c.muted_foreground.0),
            ),
        menu_separator(l),
    ];
    if provenance != Provenance::Default {
        items.push(menu_item(l, &format!("field/{path}/reset"), Some(icons::UNDO_2), "Reset to Default", None, false).on_press(move |s: &mut Studio| {
            s.local.field_menu = None;
            let commands = s.inspected().iter().map(|e| json!({"cmd": "editor.property.reset", "args": {"entity": e, "path": reset}})).collect();
            s.apply(commands, "Reset value");
        }));
        items.push(menu_separator(l));
    }
    if provenance == Provenance::Override {
        for (id, glyph, title) in [
            ("prefab.apply_override", icons::UPLOAD, "Apply to Prefab"),
            ("prefab.revert_override", icons::UNDO_2, "Revert Override"),
        ] {
            let target = crate::catalog::Target::Field(path.to_owned());
            items.push(
                menu_item(
                    l,
                    &format!("field/{path}/{id}"),
                    Some(glyph),
                    title,
                    None,
                    false,
                )
                .on_press(move |s: &mut Studio| {
                    s.local.field_menu = None;
                    super::commands::run(s, id, &target);
                }),
            );
        }
        items.push(menu_separator(l));
    }
    items.push(
        menu_item(
            l,
            &format!("field/{path}/copy"),
            Some(icons::COPY),
            "Copy Value",
            None,
            false,
        )
        .on_press(move |s: &mut Studio| {
            s.local.field_menu = None;
            s.local.copy = Some(copy_value.clone());
        }),
    );
    items.push(
        menu_item(
            l,
            &format!("field/{path}/path"),
            Some(icons::LINK),
            "Copy Path",
            None,
            false,
        )
        .on_press(move |s: &mut Studio| {
            s.local.field_menu = None;
            if let Some(entity) = s.inspected().first().cloned() {
                s.local.copy = Some(format!(
                    "{}/{copy_path}",
                    super::commands::reference(&s.editor, &entity)
                ));
            }
        }),
    );
    menu(menu_panel(l, items)).on_dismiss(|s: &mut Studio| s.local.field_menu = None)
}

/// The Inspector's component order (`inspector.componentOrder`): names in their set
/// order first, then Transform, then the rest as the record lists them.
pub(super) fn sort_components(s: &Studio, components: &mut [String]) {
    let saved: Vec<&str> = s
        .editor
        .state
        .settings
        .get("inspector.componentOrder")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    components.sort_by_key(|name| {
        saved
            .iter()
            .position(|n| n == name)
            .unwrap_or(saved.len() + usize::from(name != "Transform"))
    });
}

/// The inspected selection's sections in the order the Inspector shows them (unfiltered).
fn shown_components(s: &Studio) -> Vec<String> {
    let Some(doc) = s.editor.host.store().workspace().doc(&s.editor.state.doc) else {
        return Vec::new();
    };
    let changes = s.editor.agent_fields();
    let fields = crate::inspector::fields(
        s.editor.host.store().schema(),
        doc,
        &s.resolved,
        s.inspected(),
        &changes,
    );
    let mut shown: Vec<String> = Vec::new();
    for f in &fields {
        let component = f.path.split('.').next().unwrap_or("");
        if !shown.iter().any(|c| c == component) {
            shown.push(component.to_owned());
        }
    }
    sort_components(s, &mut shown);
    shown
}

/// Where a dragged component lands: before another section, or after the last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Above this component's section.
    Before(String),
    /// Below every section.
    End,
}

/// Puts `component` at `slot`, saved as the user's component order; whether it moved.
fn place_component(s: &mut Studio, component: &str, slot: &Slot) -> bool {
    let mut shown = shown_components(s);
    let before = shown.clone();
    let Some(at) = shown.iter().position(|c| c == component) else {
        return false;
    };
    let moving = shown.remove(at);
    let to = match slot {
        Slot::Before(other) => match shown.iter().position(|c| c == other) {
            Some(to) => to,
            None => return false,
        },
        Slot::End => shown.len(),
    };
    shown.insert(to, moving);
    if shown == before {
        return false;
    }
    s.act(
        "editor.settings.set",
        json!({"key": "inspector.componentOrder", "value": shown}),
    );
    true
}

/// Moves `component` one place up (or down) among the inspected selection's sections,
/// saved as the user's component order; whether it moved.
pub(super) fn move_component(s: &mut Studio, component: &str, down: bool) -> bool {
    let shown = shown_components(s);
    let Some(at) = shown.iter().position(|c| c == component) else {
        return false;
    };
    let slot = match down {
        true if at + 2 < shown.len() => Slot::Before(shown[at + 2].clone()),
        true if at + 1 < shown.len() => Slot::End,
        false if at > 0 => Slot::Before(shown[at - 1].clone()),
        _ => return false,
    };
    place_component(s, component, &slot)
}

/// The section headers a dragged component can drop among, while one is dragged.
pub(super) fn drop_keys(s: &Studio) -> Vec<String> {
    shown_components(s)
        .iter()
        .map(|c| format!("component/{c}"))
        .collect()
}

/// The line showing where a dragged component lands, appended to `component`'s section (so
/// the headers under the held pointer keep their place in the tree): along its top, or
/// under the last section for the End slot.
fn drop_line(s: &Studio, component: &str, last: Option<&String>) -> Option<El<Studio>> {
    let slot = s.local.component_drag.as_ref()?.1.as_ref()?;
    let line = || el().h(2.0).w_full().radius(1.0).fill(s.look.c.primary.0);
    match slot {
        Slot::Before(c) if c == component => Some(line().absolute(0.0, -2.5)),
        Slot::End if last.is_some_and(|l| l == component) => Some(line()),
        _ => None,
    }
}

/// A component header's drag, as Unity's: past a few px it reorders, the slot following
/// the pointer between the sections' middles; the release places it.
fn drag_component(s: &mut Studio, component: &str, e: &stillkit::el::PointerEvent) -> bool {
    use stillkit::el::PointerPhase;
    if e.button != stillkit::Button::Left {
        return false;
    }
    match e.phase {
        PointerPhase::Down => {
            s.local.component_drag = Some((component.to_owned(), None));
            // A drop released off its header left this set; this press is a new one.
            s.local.component_dropped = false;
            false
        }
        PointerPhase::Move => {
            if e.travel().x.abs() + e.travel().y.abs() < 4.0 {
                return false;
            }
            let shown = shown_components(s);
            let slot = shown
                .iter()
                .find(|c| {
                    s.local
                        .rects
                        .get(&format!("component/{c}"))
                        .is_some_and(|r| e.at.y < (r.top + r.bottom) / 2.0)
                })
                .map_or(Slot::End, |c| Slot::Before(c.clone()));
            match s.local.component_drag.as_mut() {
                Some((_, now)) if now.as_ref() != Some(&slot) => {
                    *now = Some(slot);
                    true
                }
                _ => false,
            }
        }
        PointerPhase::Up => match s.local.component_drag.take() {
            Some((dragged, Some(slot))) => {
                s.local.component_dropped = true;
                place_component(s, &dragged, &slot);
                true
            }
            _ => false,
        },
        PointerPhase::Cancel => s.local.component_drag.take().is_some(),
        PointerPhase::Hover => false,
    }
}

/// A component's ⋯ menu: reset it, copy and paste its values, fold the others, remove it.
fn component_menu(s: &Studio, component: &str) -> El<Studio> {
    let l = &s.look;
    let open = s.local.component_menu.as_deref() == Some(component);
    let name = component.to_owned();
    let mut b = icon_button(l, Tone::Ghost, Size::Sm, icons::ELLIPSIS, "More")
        .size(22.0)
        .key(format!("component/{component}/more"))
        .expanded(open)
        .on_press(move |s: &mut Studio| {
            s.local.component_menu = if s.local.component_menu.as_deref() == Some(name.as_str()) {
                None
            } else {
                Some(name.clone())
            };
        });
    if open {
        let (remove, others) = (component.to_owned(), component.to_owned());
        let (reset, copy, paste) = (
            component.to_owned(),
            component.to_owned(),
            component.to_owned(),
        );
        let can_paste = s
            .local
            .component_clipboard
            .as_ref()
            .is_some_and(|(kind, _)| kind == component);
        let items = vec![
            menu_item(l, &format!("component/{component}/reset"), Some(icons::UNDO_2), "Reset", None, false).on_press(move |s: &mut Studio| {
                s.local.component_menu = None;
                component_values(s, &reset, None);
            }),
            menu_item(l, &format!("component/{component}/copy"), Some(icons::COPY), "Copy Values", None, false).on_press(move |s: &mut Studio| {
                s.local.component_menu = None;
                let value = s.inspected().first().and_then(|id| s.resolved.get(id)).and_then(|e| e.record.get(&copy).cloned());
                s.local.component_clipboard = value.map(|v| (copy.clone(), v));
            }),
            enabled(
                menu_item(l, &format!("component/{component}/paste"), Some(icons::CLIPBOARD), "Paste Values", None, false).on_press(move |s: &mut Studio| {
                    s.local.component_menu = None;
                    if let Some((_, value)) = s.local.component_clipboard.clone() {
                        component_values(s, &paste, Some(&value));
                    }
                }),
                can_paste,
            ),
            menu_separator(l),
            menu_item(l, &format!("component/{component}/up"), Some(icons::CHEVRON_UP), "Move Up", None, false).on_press({
                let name = component.to_owned();
                move |s: &mut Studio| {
                    s.local.component_menu = None;
                    move_component(s, &name, false);
                }
            }),
            menu_item(l, &format!("component/{component}/down"), Some(icons::CHEVRON_DOWN), "Move Down", None, false).on_press({
                let name = component.to_owned();
                move |s: &mut Studio| {
                    s.local.component_menu = None;
                    move_component(s, &name, true);
                }
            }),
            menu_item(l, &format!("component/{component}/fold"), Some(icons::CHEVRONS_UP_DOWN), "Fold Others", None, false).on_press(move |s: &mut Studio| {
                s.local.component_menu = None;
                let all: Vec<String> = s.resolved.get(&s.inspected()[0]).map(|e| e.record.keys().filter(|k| ara_doc::is_component_key(k)).cloned().collect()).unwrap_or_default();
                s.local.folded = all.into_iter().filter(|k| *k != others).collect();
            }),
            menu_separator(l),
            menu_item(l, &format!("component/{component}/remove"), Some(icons::TRASH), "Remove Component", None, true).on_press(move |s: &mut Studio| {
                s.local.component_menu = None;
                let commands = s.inspected().iter().map(|e| json!({"cmd": "component.remove", "args": {"entity": e, "component": remove}})).collect();
                s.apply(commands, &format!("Remove {remove}"));
            }),
        ];
        b = b.overlay(
            menu(menu_panel(l, items)).on_dismiss(|s: &mut Studio| s.local.component_menu = None),
        );
    }
    b
}

/// A type name with spaces between its words: `MeshRenderer` reads "Mesh Renderer".
pub(super) fn spaced(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if i > 0 && c.is_uppercase() && (chars[i - 1].is_lowercase() || next_lower) {
            out.push(' ');
        }
        out.push(c);
    }
    out
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
