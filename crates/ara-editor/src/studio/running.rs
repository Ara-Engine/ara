//! The Inspector while Play runs: the selection's running values (read on request from the
//! controlled player, editable on the authority; Enter applies), and Keep Runtime Changes,
//! the runtime fields to tick and save to the scene in one undo step. The requests are
//! `crate::play_controls`'s, polled by the host.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, El, col, el, icon, row, text};

use super::Studio;
use crate::catalog::Target;
use crate::play_controls::FieldTarget;

/// Runs command `id` (the failure, if any, shows where failures do).
fn run(id: &'static str) -> impl Fn(&mut Studio) + Clone {
    move |s: &mut Studio| super::commands::run(s, id, &Target::None)
}

/// A value in one line: text as itself, whole numbers without a point, others as JSON.
fn show(value: &Value) -> String {
    match value {
        Value::Null => "none".into(),
        Value::String(text) => text.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(v) if v.fract().abs() < 1e-5 && v.abs() < 1e15 => format!("{v:.0}"),
            Some(v) => format!("{v:.3}").trim_end_matches('0').to_owned(),
            None => n.to_string(),
        },
        _ => {
            let text = value.to_string();
            match text.chars().count() > 80 {
                true => format!("{}…", text.chars().take(80).collect::<String>()),
                false => text,
            }
        }
    }
}

/// The Running section and the Keep chooser, while Play runs and one entity is inspected.
pub(super) fn section(s: &Studio) -> Option<El<Studio>> {
    s.editor.state.playing?;
    let [entity] = s.inspected() else {
        return None;
    };
    let l = &s.look;
    let c = &l.c;
    let busy = s.play.busy();
    let read = s.play.inspection().filter(|i| i.entity == *entity);
    let mut head = row()
        .h(28.0)
        .pad_x(8.0)
        .gap(8.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(c.success.0.with_alpha(0.14))
        .child(
            icon(icons::ACTIVITY)
                .size(13.0)
                .color(c.success_foreground.0),
        )
        .child(
            text(match read {
                Some(i) => format!("Running · tick {}", i.tick),
                None => "Running".into(),
            })
            .text_size(12.0)
            .weight(600)
            .color(c.foreground.0)
            .grow(1.0),
        );
    if busy {
        head = head.child(spinner(l, 12.0));
    }
    head = head.child(enabled(
        button(
            l,
            Tone::Ghost,
            Size::Sm,
            Some(icons::REFRESH_CW),
            if read.is_some() {
                "Read again"
            } else {
                "Read running values"
            },
        )
        .key("running/read")
        .tooltip("Read this entity's values from the running game")
        .on_press(run("play.inspect")),
        !busy,
    ));
    let mut out = col().gap(2.0).child(head);
    if let Some(inspection) = read {
        let schema = s.editor.host.store().schema();
        let components = inspection.components.as_object().into_iter().flatten();
        for (component, fields) in components {
            out = out.child(
                row().h(22.0).pad_x(8.0).child(
                    text(super::inspector::spaced(component))
                        .text_size(11.5)
                        .weight(600)
                        .color(c.muted_foreground.0),
                ),
            );
            let descs = match schema.get(component).map(|t| &t.shape) {
                Some(ara_schema::Shape::Struct { fields, .. }) => fields.as_slice(),
                _ => &[],
            };
            for (name, value) in fields.as_object().into_iter().flatten() {
                let path = format!("{component}.{name}");
                let target = descs.iter().find(|d| d.name == *name).map(|d| FieldTarget {
                    path: path.clone(),
                    pointer: String::new(),
                    ty: d.ty.clone(),
                    unit: d.unit.map(|u| u.symbol().to_owned()),
                });
                out = out.child(field(s, &path, value, target, inspection.editable && !busy));
            }
        }
        if inspection.drafts() > 0 {
            out = out.child(
                row()
                    .pad_x(8.0)
                    .gap(6.0)
                    .align(Align::Center)
                    .child(
                        text(format!("{} edits", inspection.drafts()))
                            .text_size(11.5)
                            .color(c.muted_foreground.0)
                            .grow(1.0),
                    )
                    .child(
                        button(l, Tone::Ghost, Size::Sm, None, "Discard")
                            .key("running/discard")
                            .on_press(run("play.runtime.discard")),
                    )
                    .child(
                        button(l, Tone::Primary, Size::Sm, Some(icons::CHECK), "Apply")
                            .key("running/apply")
                            .on_press(run("play.runtime.patch")),
                    ),
            );
        }
    }
    out = out.child(
        row()
            .pad_x(4.0)
            .gap(4.0)
            .child(enabled(
                button(l, Tone::Ghost, Size::Sm, Some(icons::SAVE), "Keep changes…")
                    .key("running/keep")
                    .tooltip("Choose runtime changes to save to the scene")
                    .on_press(run("play.keep")),
                !busy,
            ))
            .child(enabled(
                button(l, Tone::Ghost, Size::Sm, Some(icons::USER), "Possess")
                    .key("running/possess")
                    .tooltip("The player drives this entity")
                    .on_press(run("play.possess")),
                !busy,
            )),
    );
    out = out.children(keep(s));
    if let Some(message) = &s.play.message {
        out = out.child(
            row().pad_x(8.0).pad_y(2.0).child(
                text(message.clone())
                    .text_size(11.5)
                    .wrap(3)
                    .color(c.muted_foreground.0),
            ),
        );
    }
    Some(out)
}

/// One running field: its name and value, typed into on the authority (Enter applies).
fn field(
    s: &Studio,
    path: &str,
    value: &Value,
    target: Option<FieldTarget>,
    editable: bool,
) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let name = path.split_once('.').map_or(path, |(_, f)| f);
    let shown = s
        .play
        .inspection()
        .and_then(|i| i.draft(path, ""))
        .map_or_else(|| show(value), show);
    let label = text(super::inspector::spaced(name))
        .text_size(12.0)
        .color(c.muted_foreground.0)
        .w(0.0)
        .grow(0.4)
        .min_w(84.0);
    let scalar = !value.is_object() && !value.is_array();
    let editor = match target.filter(|_| editable && scalar) {
        Some(target) => ara_kit::ui::field(
            l,
            Size::Sm,
            format!("running/{path}"),
            None,
            name,
            &shown,
            move |f| {
                f.on_submit(move |s: &mut Studio, typed| {
                    let input = match target.ty {
                        ara_schema::ValueType::Bool => {
                            json!(matches!(typed.trim(), "true" | "on" | "1" | "yes"))
                        }
                        _ => json!(typed),
                    };
                    let drafted = s.play.draft(target.clone(), input);
                    match drafted.and_then(|()| s.play.apply(&mut s.editor)) {
                        Ok(()) => {}
                        Err(error) => s.editor.report_failure(&*anyhow::anyhow!("{error}")),
                    }
                })
            },
        ),
        None => text(shown)
            .text_size(12.0)
            .family(mono())
            .color(c.foreground.0)
            .wrap(1),
    };
    row()
        .key(format!("running/row/{path}"))
        .min_h(super::style::ROW)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .child(label)
        .child(editor.w(0.0).grow(0.6).min_w(0.0))
}

/// Keep Runtime Changes: each changed field with its scene and running values, ticked to
/// save; driven values only when offered.
fn keep(s: &Studio) -> Option<El<Studio>> {
    let choice = s.play.keep()?;
    let l = &s.look;
    let c = &l.c;
    let busy = s.play.busy();
    let mut out = col()
        .pad(6.0)
        .gap(2.0)
        .radius(super::style::RADIUS)
        .fill(super::style::tones(l).band)
        .child(
            row()
                .h(24.0)
                .gap(6.0)
                .align(Align::Center)
                .child(icon(icons::SAVE).size(13.0).color(c.muted_foreground.0))
                .child(
                    text("Keep runtime changes")
                        .text_size(12.0)
                        .weight(600)
                        .color(c.foreground.0)
                        .grow(1.0),
                )
                .child(
                    icon_button(
                        l,
                        Tone::Ghost,
                        Size::Sm,
                        icons::REFRESH_CW,
                        "Read the changes again",
                    )
                    .key("keep/refresh")
                    .on_press(run("play.keep.refresh")),
                ),
        );
    let driven = choice.include_driven();
    const DRIVEN: &str = "Offer simulation-driven values (physics, animation)";
    out = out.child(
        row()
            .key("keep/driven")
            .h(26.0)
            .gap(8.0)
            .align(Align::Center)
            .label(DRIVEN)
            .checked(driven)
            .on_press(move |s: &mut Studio| {
                if let Err(error) = s.play.keep_driven(!driven) {
                    s.editor.report_failure(&*anyhow::anyhow!("{error}"));
                }
            })
            .child(checkbox(l, "keep/driven/box", DRIVEN, driven))
            .child(text(DRIVEN).text_size(11.5).color(c.muted_foreground.0)),
    );
    if choice.rows().is_empty() {
        out = out.child(
            text("Nothing changed at runtime.")
                .text_size(11.5)
                .color(c.muted_foreground.0),
        );
    }
    for row_ in choice.rows() {
        let on = choice.picked(row_);
        let can = choice.enabled(row_);
        let (entity, path) = (row_.entity.clone(), row_.path.clone());
        let detail = match (&row_.reason, can) {
            (Some(reason), false) => reason.clone(),
            _ => format!("{} → {}", show(&row_.edit), show(&row_.play)),
        };
        out = out.child(enabled(
            row()
                .key(format!("keep/{}/{}", row_.entity, row_.path))
                .min_h(30.0)
                .gap(8.0)
                .align(Align::Center)
                .label(format!("Keep {}", row_.path))
                .checked(on)
                .on_press(move |s: &mut Studio| {
                    if let Err(error) = s.play.keep_field(&entity, &path, !on) {
                        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
                    }
                })
                .child(checkbox(
                    l,
                    format!("keep/{}/{}/box", row_.entity, row_.path),
                    &row_.path,
                    on,
                ))
                .child(
                    col()
                        .w(0.0)
                        .grow(1.0)
                        .gap(1.0)
                        .child(
                            text(row_.path.clone())
                                .text_size(12.0)
                                .family(mono())
                                .color(c.foreground.0)
                                .wrap(1),
                        )
                        .child(
                            text(detail)
                                .text_size(11.0)
                                .color(c.muted_foreground.0)
                                .wrap(2),
                        ),
                ),
            can && !busy,
        ));
    }
    let count = choice.count();
    out = out.child(
        row()
            .gap(6.0)
            .pad_y(2.0)
            .child(el().grow(1.0))
            .child(
                button(l, Tone::Ghost, Size::Sm, None, "Cancel")
                    .key("keep/cancel")
                    .on_press(run("play.keep.cancel")),
            )
            .child(enabled(
                button(
                    l,
                    Tone::Primary,
                    Size::Sm,
                    Some(icons::CHECK),
                    &match count {
                        1 => "Keep 1 field".to_owned(),
                        n => format!("Keep {n} fields"),
                    },
                )
                .key("keep/apply")
                .on_press(run("play.keep.apply")),
                count > 0 && !busy,
            )),
    );
    Some(out)
}
