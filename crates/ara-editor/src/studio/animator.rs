//! The Animation page's Details, as Unity's Animator shows them: the controller's
//! Parameters (add, type defaults, remove); a picked state (make it the default, its motion,
//! looping, length, sync group, the transitions leaving it); a picked transition arrow (its
//! blend and exit times, its conditions to add, edit and remove, and its removal). Each
//! change is one `editor.animation.controller.set` of the whole controller (one undo step).
use ara_anim::graph::{Comparison, Condition, ControllerData, ParameterKind};
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, Axis, El, col, menu, row, text};

use super::Studio;
use crate::authoring::animation::{seconds_value, ticks, transition_edge};

/// Changes the controller in `doc` and saves it as one undo step; a failed change reports
/// and saves nothing.
fn save(
    s: &mut Studio,
    doc: &str,
    why: &str,
    change: impl FnOnce(&mut ControllerData) -> Result<(), String>,
) {
    let result = (|| {
        let document = s
            .editor
            .host
            .store()
            .workspace()
            .doc(doc)
            .ok_or("controller missing")?;
        let (record, mut controller) = crate::authoring::animation::controller(document)?;
        change(&mut controller)?;
        let command = json!({"cmd": "editor.animation.controller.set", "args": {"record": record, "controller": controller}});
        s.editor
            .apply_to(doc, vec![command], why)
            .map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        s.editor.report_failure(&*anyhow::anyhow!(error));
    }
}

/// A section band: glyph, title and a muted note.
fn band(s: &Studio, glyph: stillkit::icons::Icon, title: String, note: String) -> El<Studio> {
    let c = &s.look.c;
    row()
        .h(28.0)
        .pad_x(10.0)
        .gap(8.0)
        .align(Align::Center)
        .fill(super::style::tones(&s.look).band)
        .child(
            stillkit::el::icon(glyph)
                .size(13.0)
                .color(c.muted_foreground.0),
        )
        .child(
            text(title)
                .text_size(12.5)
                .weight(600)
                .color(c.foreground.0)
                .grow(1.0),
        )
        .child(text(note).text_size(11.0).color(c.muted_foreground.0))
}

/// A labelled row: the label on the left, its editor on the right.
fn prop(s: &Studio, label: &str, value: El<Studio>) -> El<Studio> {
    row()
        .min_h(26.0)
        .pad_x(10.0)
        .gap(8.0)
        .align(Align::Center)
        .child(
            text(label.to_owned())
                .text_size(12.0)
                .color(s.look.c.muted_foreground.0)
                .w(0.0)
                .grow(0.4),
        )
        .child(value.w(0.0).grow(0.6))
}

/// A field typed and committed with Enter.
fn typed(
    s: &Studio,
    key: String,
    value: &str,
    commit: impl Fn(&mut Studio, &str) + 'static,
) -> El<Studio> {
    field(&s.look, Size::Sm, key, None, "", value, move |x| {
        x.select_all_on_focus().on_submit(commit)
    })
}

/// A dropdown of `options` under `key`, showing `current`; picking one runs `pick`.
fn dropdown(
    s: &Studio,
    key: String,
    current: &str,
    options: Vec<String>,
    pick: impl Fn(&mut Studio, &str) + Clone + 'static,
) -> El<Studio> {
    let l = &s.look;
    let open = s.local.select.as_deref() == Some(key.as_str());
    let toggle = key.clone();
    let mut trigger = select(l, Size::Sm, key.clone(), current)
        .expanded(open)
        .on_press(move |s: &mut Studio| {
            s.local.select = match s.local.select.as_deref() == Some(toggle.as_str()) {
                true => None,
                false => Some(toggle.clone()),
            }
        });
    if open {
        let items = options
            .into_iter()
            .map(|option| {
                let pick = pick.clone();
                menu_check(l, &format!("{key}/{option}"), &option, option == current).on_press(
                    move |s: &mut Studio| {
                        s.local.select = None;
                        pick(s, &option);
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

const KINDS: [(&str, ParameterKind); 4] = [
    ("Float", ParameterKind::Float),
    ("Int", ParameterKind::Int),
    ("Bool", ParameterKind::Bool),
    ("Trigger", ParameterKind::Trigger),
];

fn kind_name(kind: ParameterKind) -> &'static str {
    KINDS
        .iter()
        .find(|(_, k)| *k == kind)
        .map_or("Float", |(name, _)| name)
}

/// The Animation page's Details panel for controller `doc`.
pub(super) fn details(s: &Studio, doc: &str) -> El<Studio> {
    let out = col()
        .key("animation/details")
        .w(300.0)
        .min_w(240.0)
        .fill(super::style::tones(&s.look).panel)
        .scroll(Axis::Vertical);
    let Some(controller) = s
        .editor
        .host
        .store()
        .workspace()
        .doc(doc)
        .and_then(|d| crate::authoring::animation::controller(d).ok())
        .map(|(_, c)| c)
    else {
        return out;
    };
    let Some(shown) = s.graph.shown_graph() else {
        return out;
    };
    let selection = s.graph.selection();
    let transition = controller
        .transitions
        .iter()
        .find(|t| selection.contains(&transition_edge(&t.id)));
    let state = selection.iter().find_map(|id| {
        let name = shown.nodes.get(id)?.payload.get("state")?.as_str()?;
        controller.states.get_key_value(name)
    });
    let mut out = out;
    if let Some(transition) = transition {
        out = out.children(transition_details(s, doc, &controller, transition));
    } else if let Some((name, _)) = state {
        out = out.children(state_details(s, doc, &controller, name));
    }
    out.children(parameters(s, doc, &controller))
}

/// The Parameters list: each one's type, default and removal; the Add menu.
fn parameters(s: &Studio, doc: &str, controller: &ControllerData) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    // + Add opens a menu of the four types.
    let open = s.local.select.as_deref() == Some("animation/param/add");
    let mut add = button(l, Tone::Ghost, Size::Sm, Some(icons::PLUS), "Add")
        .key("animation/param/add")
        .expanded(open)
        .on_press(|s: &mut Studio| {
            s.local.select = match s.local.select.as_deref() == Some("animation/param/add") {
                true => None,
                false => Some("animation/param/add".into()),
            }
        });
    if open {
        let items = KINDS
            .iter()
            .map(|(name, kind)| {
                let (doc, kind) = (doc.to_owned(), *kind);
                menu_item(
                    l,
                    &format!("animation/param/add/{name}"),
                    None,
                    name,
                    None,
                    false,
                )
                .on_press(move |s: &mut Studio| {
                    s.local.select = None;
                    add_parameter(s, &doc, kind);
                })
            })
            .collect();
        add = add.overlay(
            menu(menu_panel(l, items))
                .side(stillkit::el::Side::Bottom)
                .align(Align::End)
                .on_dismiss(|s: &mut Studio| s.local.select = None),
        );
    }
    let mut out = vec![
        row()
            .h(32.0)
            .pad_x(10.0)
            .gap(8.0)
            .align(Align::Center)
            .fill(super::style::tones(l).band)
            .child(
                stillkit::el::icon(icons::SLIDERS_HORIZONTAL)
                    .size(13.0)
                    .color(c.muted_foreground.0),
            )
            .child(
                text("Parameters")
                    .text_size(12.5)
                    .weight(600)
                    .color(c.foreground.0)
                    .grow(1.0),
            )
            .child(add),
    ];
    if controller.parameters.is_empty() {
        out.push(
            row().pad(10.0).child(
                text("No parameters. Gameplay sets them; transitions test them.")
                    .text_size(11.5)
                    .color(c.muted_foreground.0)
                    .wrap(3),
            ),
        );
    }
    for (name, parameter) in &controller.parameters {
        let (doc_value, doc_remove) = (doc.to_owned(), doc.to_owned());
        let (name_value, name_remove) = (name.clone(), name.clone());
        let value: El<Studio> = match parameter.kind {
            ParameterKind::Bool => {
                let on = parameter.bool_value;
                checkbox(l, format!("animation/param/{name}/value"), "", on).on_press(
                    move |s: &mut Studio| {
                        save(s, &doc_value, "Set parameter default", |controller| {
                            let p = controller
                                .parameters
                                .get_mut(&name_value)
                                .ok_or("parameter missing")?;
                            p.bool_value = !on;
                            Ok(())
                        })
                    },
                )
            }
            ParameterKind::Trigger => text("unset").text_size(11.5).color(c.muted_foreground.0),
            kind => {
                let shown = if kind == ParameterKind::Int {
                    parameter.int_value.to_string()
                } else {
                    seconds_like(parameter.float_value)
                };
                typed(
                    s,
                    format!("animation/param/{name}/value"),
                    &shown,
                    move |s: &mut Studio, t: &str| {
                        let t = t.trim().to_owned();
                        save(s, &doc_value, "Set parameter default", |controller| {
                            let p = controller
                                .parameters
                                .get_mut(&name_value)
                                .ok_or("parameter missing")?;
                            if p.kind == ParameterKind::Int {
                                p.int_value = t.parse().map_err(|_| "a whole number")?;
                            } else {
                                let v: f32 = t.parse().map_err(|_| "a number")?;
                                if !v.is_finite() {
                                    return Err("a finite number".into());
                                }
                                p.float_value = v;
                            }
                            Ok(())
                        })
                    },
                )
            }
        };
        out.push(
            row()
                .key(format!("animation/param/{name}"))
                .min_h(28.0)
                .pad_x(10.0)
                .gap(8.0)
                .align(Align::Center)
                .child(
                    text(name.clone())
                        .text_size(12.0)
                        .color(c.foreground.0)
                        .w(0.0)
                        .grow(0.45),
                )
                .child(
                    text(kind_name(parameter.kind))
                        .text_size(10.5)
                        .color(c.muted_foreground.0)
                        .w(44.0),
                )
                .child(row().w(0.0).grow(0.55).child(value.grow(1.0)))
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Remove parameter")
                        .size(20.0)
                        .key(format!("animation/param/{name}/remove"))
                        .on_press(move |s: &mut Studio| {
                            save(s, &doc_remove, "Remove animation parameter", |controller| {
                                controller.parameters.remove(&name_remove);
                                // Conditions on it go with it.
                                for t in &mut controller.transitions {
                                    t.conditions.retain(|c| c.parameter != name_remove);
                                }
                                Ok(())
                            })
                        }),
                ),
        );
    }
    out
}

/// Adds a parameter of `kind` under the first free `Kind<n>` name.
fn add_parameter(s: &mut Studio, doc: &str, kind: ParameterKind) {
    save(s, doc, "Add animation parameter", |controller| {
        let name = (1..)
            .map(|n| format!("{}{n}", kind_name(kind)))
            .find(|n| !controller.parameters.contains_key(n))
            .ok_or("no free parameter name")?;
        controller.parameters.insert(
            name,
            ara_anim::graph::Parameter {
                kind,
                ..Default::default()
            },
        );
        Ok(())
    });
}

/// A float as typed: "0.2", "1", never "0.200000".
fn seconds_like(v: f32) -> String {
    let text = format!("{v:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A picked state: default, motion, looping, length, sync group and its transitions out.
fn state_details(
    s: &Studio,
    doc: &str,
    controller: &ControllerData,
    name: &str,
) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let Some(state) = controller.states.get(name) else {
        return Vec::new();
    };
    let initial = controller.initial == name;
    let motion = controller
        .nodes
        .get(&state.node)
        .map(|node| match &node.clip {
            Some(clip) if node.kind == ara_anim::graph::NodeKind::Clip => clip
                .0
                .rsplit('/')
                .next()
                .unwrap_or(&clip.0)
                .split('#')
                .next()
                .unwrap_or_default()
                .to_owned(),
            None if node.kind == ara_anim::graph::NodeKind::Clip => "No clip".to_owned(),
            _ => crate::shell::inspector_panel::sentence(
                &serde_json::to_value(node.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            ),
        })
        .unwrap_or_default();
    let set = |key: &'static str| {
        let (doc, name) = (doc.to_owned(), name.to_owned());
        move |s: &mut Studio, value: Value| {
            save(s, &doc, "Edit animation state", |controller| {
                let state = controller.states.get_mut(&name).ok_or("state missing")?;
                let mut source = ara_json::to_value(&*state).map_err(|e| e.to_string())?;
                source[key] = value;
                *state = serde_json::from_value(source).map_err(|e| e.to_string())?;
                Ok(())
            })
        }
    };
    let (default_doc, default_name) = (doc.to_owned(), name.to_owned());
    let mut out = vec![
        band(
            s,
            icons::PERSON_STANDING,
            name.to_owned(),
            if initial {
                "Default state".into()
            } else {
                "State".into()
            },
        ),
        prop(
            s,
            "Motion",
            text(motion).text_size(12.0).color(c.foreground.0),
        ),
    ];
    if !initial {
        out.push(
            row().pad_x(10.0).pad_y(4.0).child(
                button(
                    l,
                    Tone::Ghost,
                    Size::Sm,
                    Some(icons::FLAG),
                    "Set as default state",
                )
                .key("animation/state/default")
                .on_press(move |s: &mut Studio| {
                    let name = default_name.clone();
                    save(s, &default_doc, "Set default state", |controller| {
                        controller.initial = name;
                        Ok(())
                    })
                }),
            ),
        );
    }
    let looping = state.looping;
    let toggle = set("looping");
    out.push(prop(
        s,
        "Loop",
        checkbox(l, "animation/state/looping", "", looping)
            .on_press(move |s: &mut Studio| toggle(s, json!(!looping))),
    ));
    let length = set("duration");
    out.push(prop(
        s,
        "Length (s)",
        typed(
            s,
            "animation/state/duration".into(),
            &seconds_value(state.duration),
            move |s: &mut Studio, t: &str| match ticks(&json!(t)) {
                Ok(ticks) => length(s, json!(ticks.unwrap_or(0))),
                Err(error) => s.editor.report_failure(&*anyhow::anyhow!(error)),
            },
        ),
    ));
    let group = set("syncGroup");
    out.push(prop(
        s,
        "Sync group",
        typed(
            s,
            "animation/state/sync".into(),
            &state.sync_group,
            move |s: &mut Studio, t: &str| group(s, json!(t.trim())),
        ),
    ));
    out.push(band(
        s,
        icons::ARROW_RIGHT,
        "Transitions".into(),
        String::new(),
    ));
    let leaving: Vec<_> = controller
        .transitions
        .iter()
        .filter(|t| t.from == name)
        .collect();
    if leaving.is_empty() {
        out.push(
            row().pad(10.0).child(
                text("None. Drag from this state's edge to another to add one.")
                    .text_size(11.5)
                    .color(c.muted_foreground.0)
                    .wrap(2),
            ),
        );
    }
    for t in leaving {
        let edge = transition_edge(&t.id);
        let hover = c.hover_surface.0;
        out.push(
            row()
                .key(format!("animation/transition/{}", t.id))
                .min_h(26.0)
                .pad_x(10.0)
                .gap(8.0)
                .align(Align::Center)
                .hovered(move |e| e.fill(hover))
                .on_press(move |s: &mut Studio| s.graph.pick(edge.clone()))
                .child(
                    text(format!("→ {}", t.to))
                        .text_size(12.0)
                        .color(c.foreground.0),
                )
                .child(
                    text(crate::authoring::animation::conditions(
                        t,
                        &controller.parameters,
                    ))
                    .text_size(11.0)
                    .color(c.muted_foreground.0)
                    .wrap(1)
                    .w(0.0)
                    .grow(1.0),
                ),
        );
    }
    out
}

/// A picked transition: blend and exit times, its conditions, its removal.
fn transition_details(
    s: &Studio,
    doc: &str,
    controller: &ControllerData,
    transition: &ara_anim::graph::Transition,
) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let id = transition.id.clone();
    // Edits this transition through `change` (one undo step).
    let edit = |why: &'static str| {
        let (doc, id) = (doc.to_owned(), id.clone());
        move |s: &mut Studio,
              change: &dyn Fn(&mut ara_anim::graph::Transition) -> Result<(), String>| {
            save(s, &doc, why, |controller| {
                let t = controller
                    .transitions
                    .iter_mut()
                    .find(|t| t.id == id)
                    .ok_or("transition missing")?;
                change(t)
            })
        }
    };
    let mut out = vec![band(
        s,
        icons::ARROW_RIGHT,
        format!("{} → {}", transition.from, transition.to),
        "Transition".into(),
    )];
    let blend = edit("Edit transition blend");
    out.push(prop(
        s,
        "Blend (s)",
        typed(
            s,
            "animation/transition/duration".into(),
            &seconds_value(transition.duration),
            move |s: &mut Studio, t: &str| match ticks(&json!(t)) {
                Ok(ticks) => blend(s, &|tr| {
                    tr.duration = ticks.unwrap_or(0);
                    Ok(())
                }),
                Err(error) => s.editor.report_failure(&*anyhow::anyhow!(error)),
            },
        ),
    ));
    let exit = edit("Edit transition exit time");
    out.push(prop(
        s,
        "Exit time (s)",
        typed(
            s,
            "animation/transition/exit".into(),
            &transition.exit_tick.map(seconds_value).unwrap_or_default(),
            move |s: &mut Studio, t: &str| match ticks(&json!(t)) {
                Ok(ticks) => exit(s, &|tr| {
                    tr.exit_tick = ticks;
                    Ok(())
                }),
                Err(error) => s.editor.report_failure(&*anyhow::anyhow!(error)),
            },
        ),
    ));
    let add = edit("Add transition condition");
    let first = controller
        .parameters
        .iter()
        .next()
        .map(|(name, p)| (name.clone(), p.kind));
    let can_add = first.is_some();
    out.push(
        row()
            .h(32.0)
            .pad_x(10.0)
            .gap(8.0)
            .align(Align::Center)
            .fill(super::style::tones(l).band)
            .child(
                stillkit::el::icon(icons::LIST_FILTER)
                    .size(13.0)
                    .color(c.muted_foreground.0),
            )
            .child(
                text("Conditions")
                    .text_size(12.5)
                    .weight(600)
                    .color(c.foreground.0)
                    .grow(1.0),
            )
            .child(enabled(
                icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, "Add a condition")
                    .size(22.0)
                    .key("animation/condition/add")
                    .on_press(move |s: &mut Studio| {
                        let Some((parameter, kind)) = first.clone() else {
                            return;
                        };
                        add(s, &|tr| {
                            tr.conditions.push(Condition {
                                parameter: parameter.clone(),
                                comparison: default_comparison(kind),
                                bool_value: true,
                                ..Default::default()
                            });
                            Ok(())
                        })
                    }),
                can_add,
            )),
    );
    if transition.conditions.is_empty() {
        out.push(
            row().pad(10.0).child(
                text(if controller.parameters.is_empty() {
                    "Add a parameter to test it here."
                } else if transition.exit_tick.is_some() {
                    "None: it leaves at its exit time."
                } else {
                    "None: it leaves at once."
                })
                .text_size(11.5)
                .color(c.muted_foreground.0)
                .wrap(2),
            ),
        );
    }
    let names: Vec<String> = controller.parameters.keys().cloned().collect();
    for (index, condition) in transition.conditions.iter().enumerate() {
        let kind = controller
            .parameters
            .get(&condition.parameter)
            .map_or(ParameterKind::Float, |p| p.kind);
        let kinds = controller
            .parameters
            .iter()
            .map(|(n, p)| (n.clone(), p.kind))
            .collect::<std::collections::BTreeMap<_, _>>();
        let on_parameter = edit("Edit transition condition");
        let parameter = dropdown(
            s,
            format!("animation/condition/{index}/parameter"),
            &condition.parameter,
            names.clone(),
            move |s: &mut Studio, name: &str| {
                let (name, kind) = (
                    name.to_owned(),
                    kinds.get(name).copied().unwrap_or_default(),
                );
                on_parameter(s, &|tr| {
                    let c = tr.conditions.get_mut(index).ok_or("condition missing")?;
                    c.parameter = name.clone();
                    c.comparison = default_comparison(kind);
                    Ok(())
                })
            },
        );
        let (compare, value): (El<Studio>, El<Studio>) = match kind {
            ParameterKind::Trigger => (
                text("is set").text_size(12.0).color(c.muted_foreground.0),
                row(),
            ),
            ParameterKind::Bool => {
                let truth = (condition.comparison == Comparison::Equal) == condition.bool_value;
                let on_truth = edit("Edit transition condition");
                (
                    dropdown(
                        s,
                        format!("animation/condition/{index}/truth"),
                        if truth { "is true" } else { "is false" },
                        vec!["is true".into(), "is false".into()],
                        move |s: &mut Studio, pick: &str| {
                            let truth = pick == "is true";
                            on_truth(s, &|tr| {
                                let c = tr.conditions.get_mut(index).ok_or("condition missing")?;
                                c.comparison = Comparison::Equal;
                                c.bool_value = truth;
                                Ok(())
                            })
                        },
                    ),
                    row(),
                )
            }
            _ => {
                let on_compare = edit("Edit transition condition");
                let on_value = edit("Edit transition condition");
                let shown = if kind == ParameterKind::Int {
                    condition.int_value.to_string()
                } else {
                    seconds_like(condition.float_value)
                };
                (
                    dropdown(
                        s,
                        format!("animation/condition/{index}/compare"),
                        compare_symbol(condition.comparison),
                        [">", "<", "=", "≠"].map(str::to_owned).to_vec(),
                        move |s: &mut Studio, symbol: &str| {
                            let comparison = match symbol {
                                ">" => Comparison::Greater,
                                "<" => Comparison::Less,
                                "=" => Comparison::Equal,
                                _ => Comparison::NotEqual,
                            };
                            on_compare(s, &|tr| {
                                tr.conditions
                                    .get_mut(index)
                                    .ok_or("condition missing")?
                                    .comparison = comparison;
                                Ok(())
                            })
                        },
                    )
                    .w(56.0),
                    typed(
                        s,
                        format!("animation/condition/{index}/value"),
                        &shown,
                        move |s: &mut Studio, t: &str| {
                            let t = t.trim().to_owned();
                            on_value(s, &|tr| {
                                let c = tr.conditions.get_mut(index).ok_or("condition missing")?;
                                if kind == ParameterKind::Int {
                                    c.int_value = t.parse().map_err(|_| "a whole number")?;
                                } else {
                                    let v: f32 = t.parse().map_err(|_| "a number")?;
                                    if !v.is_finite() {
                                        return Err("a finite number".into());
                                    }
                                    c.float_value = v;
                                }
                                Ok(())
                            })
                        },
                    )
                    .w(0.0)
                    .grow(1.0),
                )
            }
        };
        let remove = edit("Remove transition condition");
        out.push(
            row()
                .key(format!("animation/condition/{index}"))
                .min_h(30.0)
                .pad_x(10.0)
                .gap(6.0)
                .align(Align::Center)
                .child(parameter.w(0.0).grow(1.0))
                .child(compare)
                .child(value)
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Remove condition")
                        .size(20.0)
                        .key(format!("animation/condition/{index}/remove"))
                        .on_press(move |s: &mut Studio| {
                            remove(s, &|tr| {
                                if index < tr.conditions.len() {
                                    tr.conditions.remove(index);
                                }
                                Ok(())
                            })
                        }),
                ),
        );
    }
    let (doc_delete, id_delete) = (doc.to_owned(), id.clone());
    out.push(
        row().pad(10.0).child(
            button(
                l,
                Tone::Ghost,
                Size::Sm,
                Some(icons::TRASH),
                "Delete transition",
            )
            .key("animation/transition/delete")
            .on_press(move |s: &mut Studio| {
                let id = id_delete.clone();
                save(s, &doc_delete, "Delete transition", |controller| {
                    controller.transitions.retain(|t| t.id != id);
                    Ok(())
                })
            }),
        ),
    );
    out
}

/// What a new condition on a parameter of `kind` tests first.
fn default_comparison(kind: ParameterKind) -> Comparison {
    match kind {
        ParameterKind::Trigger => Comparison::Trigger,
        ParameterKind::Bool => Comparison::Equal,
        _ => Comparison::Greater,
    }
}

fn compare_symbol(comparison: Comparison) -> &'static str {
    match comparison {
        Comparison::Greater => ">",
        Comparison::Less => "<",
        Comparison::Equal => "=",
        Comparison::NotEqual => "≠",
        Comparison::Trigger => "set",
    }
}
