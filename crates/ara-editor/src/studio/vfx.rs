//! The VFX tool, laid out as Niagara's: the preview with its timeline (restart,
//! play/pause, step, rate) on the left, a placeholder with disabled controls until the renderer
//! can draw effects; the System Overview in the
//! middle, one card per emitter with its stages (spawn, initialize, update, render) in their
//! colours; the picked module's Selection on the right, where its values are typed and its
//! tools move, duplicate or remove it. A module's checkbox keeps it but skips it. Hide and
//! Isolate change only the preview (and are kept for when it returns). Every edit is one undo
//! step of the document's commands.
use std::collections::BTreeSet;

use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, Axis, El, Justify, col, el, icon, menu, row, text};

use super::Studio;

/// An emitter card's width in the System Overview.
const CARD: f32 = 300.0;

/// The VFX page's view state (what to show, never the effect itself).
#[derive(Default)]
pub struct Page {
    hidden: BTreeSet<String>,
    isolated: Option<String>,
    collapsed: BTreeSet<String>,
}

/// What a preview toggle does to one emitter (the effect itself is unchanged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Emitter {
    /// Fold or unfold its stack.
    Collapse,
    /// Leave it out of the preview, or bring it back.
    Hide,
    /// Preview only it, or every emitter again.
    Isolate,
}

impl Page {
    /// Flips one of emitter `name`'s preview toggles.
    pub(super) fn emitter(&mut self, name: &str, toggle: Emitter) {
        let flip = |set: &mut BTreeSet<String>| {
            if !set.remove(name) {
                set.insert(name.to_owned());
            }
        };
        match toggle {
            Emitter::Collapse => flip(&mut self.collapsed),
            Emitter::Hide => flip(&mut self.hidden),
            Emitter::Isolate => {
                self.isolated = (self.isolated.as_deref() != Some(name)).then(|| name.to_owned())
            }
        }
    }
}

/// The effect document the page edits: the selected resource when it is one, else the
/// project's first effect.
pub(super) fn document(s: &Studio) -> Option<String> {
    let workspace = s.editor.host.store().workspace();
    let matches = |doc: &ara_doc::Document| {
        doc.kind == "graph"
            && doc
                .record("g_meta")
                .and_then(|r| r.get("domain"))
                .and_then(Value::as_str)
                == Some("vfx")
    };
    let selected = s
        .editor
        .state
        .resource
        .as_ref()
        .and_then(|id| workspace.doc(id));
    selected
        .filter(|d| matches(d))
        .or_else(|| workspace.docs().map(|(_, d)| d).find(|d| matches(d)))
        .map(|d| d.id.to_string())
}

/// The effect in `doc`, or why it doesn't compile.
fn effect(s: &Studio, doc: &str) -> Result<ara_vfx::Effect, String> {
    let document = s
        .editor
        .host
        .store()
        .workspace()
        .doc(doc)
        .ok_or("effect missing")?;
    let graph = ara_graph::Graph::from_document(document).map_err(|d| d.to_string())?;
    ara_vfx::Effect::from_graph(&graph).map_err(|d| d.to_string())
}

/// The page, laid out as Niagara's: the preview and its timeline on the left, the System
/// Overview (one card per emitter, its stages in their colours) in the middle, and the picked
/// module's Selection on the right.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let t = super::style::tones(l);
    let Some(doc) = document(s) else {
        return col().grow(1.0).justify(Justify::Center).child(empty(
            l,
            icons::SPARKLES,
            "No effect yet",
            "Effects are emitters of particles, each a stack of modules that run in order.",
            Some(
                button(l, Tone::Primary, Size::Sm, Some(icons::PLUS), "New effect")
                    .key("vfx/new")
                    .on_press(|s: &mut Studio| super::asset_create::create(s, "vfx")),
            ),
        ));
    };
    let fx = effect(s, &doc);
    // The preview fills its column (as the last layout measured it), square.
    let size = s
        .local
        .rects
        .get("vfx/preview")
        .map_or(360.0, |r| (r.width() - 20.0).clamp(160.0, 720.0));
    let preview = col().size(size).child(super::unavailable::tile(
        l,
        "vfx/preview-unavailable",
        "Effect preview is not available yet.",
    ));
    let status = match &fx {
        Ok(_) => row()
            .gap(6.0)
            .align(Align::Center)
            .child(el().size(7.0).radius(3.5).fill(c.success_foreground.0))
            .child(text("Compiled").text_size(11.5).color(c.muted_foreground.0)),
        Err(error) => row().child(
            text(format!("Compile failed · {error}"))
                .text_size(11.5)
                .color(c.destructive_foreground.0)
                .wrap(3)
                .grow(1.0),
        ),
    };
    let left = col()
        .key("vfx/preview")
        .w(380.0)
        .min_w(300.0)
        .fill(t.panel)
        .child(heading(l, icons::EYE, "Preview", None))
        .child(
            col()
                .pad(10.0)
                .align(Align::Center)
                .fill(c.background.0)
                .child(preview),
        )
        .child(
            row()
                .min_h(28.0)
                .pad_x(10.0)
                .align(Align::Center)
                .child(status),
        );
    let overview = match &fx {
        Ok(effect) => {
            // Cards wrap into as many columns as fit (the last layout's width).
            let width = s.local.rects.get("vfx/stacks").map_or(960.0, |r| r.width());
            let columns = (((width - 32.0 + 16.0) / (CARD + 16.0)).floor() as usize).max(1);
            let mut cards = col().pad(16.0).gap(16.0);
            for line in effect.emitters.chunks(columns) {
                cards = cards.child(
                    row()
                        .gap(16.0)
                        .align(Align::Start)
                        .children(line.iter().map(|e| emitter_card(s, &doc, e))),
                );
            }
            col()
                .key("vfx/stacks")
                .h(0.0)
                .grow(1.0)
                .scroll(Axis::Vertical)
                .child(cards)
        }
        Err(_) => col().grow(1.0),
    };
    let middle = col()
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        // Niagara's graph paper behind the emitter cards.
        .fill(mix(c.card.0, c.raised.0, 0.35))
        .child(
            heading(
                l,
                icons::SPARKLES,
                "System Overview",
                fx.as_ref().ok().map(|e| match e.emitters.len() {
                    1 => "1 emitter".to_owned(),
                    n => format!("{n} emitters"),
                }),
            )
            .child({
                let doc = doc.clone();
                icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, "Add emitter")
                    .size(22.0)
                    .key("vfx/emitter/add")
                    .on_press(move |s: &mut Studio| {
                        if let Err(error) =
                            crate::vfx_edit::emitter_edit(&mut s.editor, &doc, "", "add")
                        {
                            s.editor.report_failure(&*anyhow::anyhow!("{error}"));
                        }
                    })
            }),
        )
        .child(overview);
    let right = match &fx {
        Ok(effect) => selection(s, &doc, effect),
        Err(_) => col(),
    }
    .key("vfx/selection")
    .w(300.0)
    .min_w(240.0)
    .fill(t.panel)
    .scroll(Axis::Vertical);
    // Bounded, so the overview and Selection scroll inside the page.
    row()
        .grow(1.0)
        .min_h(0.0)
        .child(left)
        .child(middle)
        .child(right)
}

/// An emitter's ⋯ menu: Duplicate and Delete (one undo step each).
fn emitter_menu(s: &Studio, doc: &str, name: &str) -> El<Studio> {
    let l = &s.look;
    let key = format!("vfx/emitter/{name}/more");
    let open = s.local.select.as_deref() == Some(key.as_str());
    let toggle = key.clone();
    let mut more = icon_button(l, Tone::Ghost, Size::Sm, icons::ELLIPSIS, "More")
        .size(22.0)
        .key(key.clone())
        .on_press(move |s: &mut Studio| {
            s.local.select =
                (s.local.select.as_deref() != Some(toggle.as_str())).then(|| toggle.clone());
        });
    if open {
        let item = |op: &'static str, glyph, label: &str| {
            let (doc, name) = (doc.to_owned(), name.to_owned());
            menu_item(l, &format!("{key}/{op}"), Some(glyph), label, None, false).on_press(
                move |s: &mut Studio| {
                    s.local.select = None;
                    if let Err(error) =
                        crate::vfx_edit::emitter_edit(&mut s.editor, &doc, &name, op)
                    {
                        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
                    }
                },
            )
        };
        more = more.overlay(
            menu(menu_panel(
                l,
                vec![
                    item("duplicate", icons::COPY, "Duplicate emitter"),
                    item("delete", icons::TRASH, "Delete emitter"),
                ],
            ))
            .on_dismiss(|s: &mut Studio| s.local.select = None),
        );
    }
    more
}

/// A panel's title row: its glyph, title and a muted note.
fn heading(
    l: &ara_kit::look::Look,
    glyph: stillkit::icons::Icon,
    title: &str,
    note: Option<String>,
) -> El<Studio> {
    let c = &l.c;
    row()
        .h(28.0)
        .pad_x(10.0)
        .gap(8.0)
        .align(Align::Center)
        .fill(super::style::tones(l).band)
        .child(icon(glyph).size(13.0).color(c.muted_foreground.0))
        .child(
            text(title.to_owned())
                .text_size(12.0)
                .weight(600)
                .color(c.foreground.0)
                .grow(1.0),
        )
        .children(note.map(|n| text(n).text_size(11.0).color(c.muted_foreground.0)))
}

/// A stage's colour, as Niagara tints its groups: emitter spawning orange, particle
/// initialising and updating green and teal, rendering red.
fn stage_color(c: &ara_kit::look::Colors, stage: ara_vfx::Stage) -> stillkit::Rgba {
    match stage {
        ara_vfx::Stage::Spawn => c.warning_foreground.0,
        ara_vfx::Stage::Initialize => c.success_foreground.0,
        ara_vfx::Stage::Update => c.chart_2.0,
        ara_vfx::Stage::Render => c.destructive_foreground.0,
    }
}

/// An emitter as Niagara's System Overview shows it: a card with its name, the preview-only
/// Hide and Isolate, and (unless collapsed) its four stages.
fn emitter_card(s: &Studio, doc: &str, emitter: &ara_vfx::Emitter) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let name = emitter.name.clone();
    let collapsed = s.vfx.collapsed.contains(&name);
    let hidden = s.vfx.hidden.contains(&name);
    let isolated = s.vfx.isolated.as_deref() == Some(name.as_str());
    let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
    let accent = c.warning_foreground.0;
    let card = mix(c.background.0, c.card.0, 0.4);
    let header = row()
        .key(format!("vfx/emitter/{name}"))
        .h(34.0)
        .pad_x(6.0)
        .gap(6.0)
        .align(Align::Center)
        .fill(mix(card, accent, 0.32))
        .radius(6.0)
        .edges(stillkit::Rgba::TRANSPARENT, accent.with_alpha(0.8))
        .child(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                if collapsed {
                    icons::CHEVRON_RIGHT
                } else {
                    icons::CHEVRON_DOWN
                },
                if collapsed { "Expand" } else { "Collapse" },
            )
            .size(20.0)
            .on_press(move |s: &mut Studio| s.vfx.emitter(&n1, Emitter::Collapse)),
        )
        .child(icon(icons::SPARKLE).size(14.0).color(accent))
        .child(
            text(name.clone())
                .text_size(13.0)
                .weight(600)
                .color(c.foreground.0)
                .grow(1.0),
        )
        .child(
            toggle_button(
                l,
                Size::Sm,
                if hidden { icons::EYE_OFF } else { icons::EYE },
                "Hide in the preview",
                hidden,
            )
            .size(22.0)
            .on_press(move |s: &mut Studio| s.vfx.emitter(&n2, Emitter::Hide)),
        )
        .child(
            toggle_button(
                l,
                Size::Sm,
                icons::FOCUS,
                "Isolate in the preview",
                isolated,
            )
            .size(22.0)
            .on_press(move |s: &mut Studio| s.vfx.emitter(&n3, Emitter::Isolate)),
        )
        .child(emitter_menu(s, doc, &emitter.name));
    let mut out = col()
        .w(CARD)
        .radius(6.0)
        .fill(card)
        .border(1.0, stillkit::Rgba::new(0.0, 0.0, 0.0, 0.55))
        .shadow()
        .child(header);
    if !collapsed {
        out = out.child(col().pad(4.0).gap(2.0).children(stack(s, doc, emitter)));
    }
    if hidden {
        out = out.opacity(0.55);
    }
    out
}

/// An emitter's four stages, each a tinted group header with its Add and its modules.
fn stack(s: &Studio, doc: &str, emitter: &ara_vfx::Emitter) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let mut out = Vec::new();
    for stage in ara_vfx::Stacks::STAGES {
        let stage_name = serde_json::to_value(stage)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let modules = match stage {
            ara_vfx::Stage::Spawn => &emitter.stacks.spawn,
            ara_vfx::Stage::Initialize => &emitter.stacks.initialize,
            ara_vfx::Stage::Update => &emitter.stacks.update,
            ara_vfx::Stage::Render => &emitter.stacks.render,
        };
        let rail = stage_color(c, stage);
        let (doc_add, emitter_name, stage_add, index) = (
            doc.to_owned(),
            emitter.name.clone(),
            stage_name.clone(),
            modules.len(),
        );
        let title = match stage {
            ara_vfx::Stage::Spawn => "Emitter Spawn",
            ara_vfx::Stage::Initialize => "Particle Spawn",
            ara_vfx::Stage::Update => "Particle Update",
            ara_vfx::Stage::Render => "Render",
        };
        out.push(
            row()
                .h(26.0)
                .pad_x(6.0)
                .gap(7.0)
                .align(Align::Center)
                .radius(4.0)
                .fill(rail.with_alpha(0.2))
                .child(el().w(3.0).h(14.0).radius(1.5).fill(rail))
                .child(
                    text(title)
                        .text_size(11.5)
                        .weight(600)
                        .color(c.foreground.0)
                        .grow(1.0),
                )
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, &format!("Add a {stage_name} module"))
                        .size(20.0)
                        .key(format!("vfx/add/{}/{stage_name}", emitter.name))
                        .on_press(move |s: &mut Studio| {
                            let module = crate::vfx_edit::default_module(stage, s.editor.host.store().revision());
                            let command = json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter_name,"stage":stage_add,"name":module.name,"operation":"insert","index":index,"module":module}});
                            if let Err(error) = s.editor.apply_to(&doc_add, vec![command], "Add VFX module") {
                                s.editor.report_failure(&error);
                            }
                        }),
                ),
        );
        for module in modules {
            out.push(module_row(s, doc, &emitter.name, &stage_name, module));
        }
    }
    out
}

/// A module's title: its name in words (`hearth_light` reads "Hearth light"), so a stack
/// reads one way whether its modules are named after their kinds or not. Keys keep the
/// name itself.
fn module_title(module: &ara_vfx::StackModule) -> String {
    crate::names::sentence(&module.name)
}

/// A module in its stage: its checkbox and name, as Niagara lists them; picking it shows it
/// in the Selection.
fn module_row(
    s: &Studio,
    doc: &str,
    emitter: &str,
    stage: &str,
    module: &ara_vfx::StackModule,
) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let item = format!("{emitter}|{stage}|{}", module.name);
    let picked = s.editor.state.resource_item.as_deref() == Some(item.as_str());
    let title = module_title(module);
    let (doc_pick, item_pick) = (doc.to_owned(), item.clone());
    let hover = c.hover_surface.0;
    super::style::chosen(l, row(), picked)
        .key(format!("vfx/module/{item}"))
        .h(26.0)
        .pad_x(10.0)
        .gap(8.0)
        .align(Align::Center)
        .radius(4.0)
        .label(title.clone())
        .hovered(move |e| if picked { e } else { e.fill(hover) })
        .on_press(move |s: &mut Studio| {
            s.act(
                "editor.workspace",
                json!({"resource": doc_pick, "item": item_pick}),
            )
        })
        .child(
            checkbox(l, format!("vfx/enable/{item}"), "Run", module.enabled).on_press({
                let (doc, item) = (doc.to_owned(), item.clone());
                move |s: &mut Studio| edit(s, &doc, &item, "toggle")
            }),
        )
        .child(
            text(title)
                .text_size(12.0)
                .color(if module.enabled {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                })
                .grow(1.0),
        )
}

/// Niagara's Selection: the picked module's name, stage and tools (move, duplicate, delete),
/// then its values; with nothing picked, how to pick one.
fn selection(s: &Studio, doc: &str, effect: &ara_vfx::Effect) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let picked = s.editor.state.resource_item.as_deref().and_then(|item| {
        let mut parts = item.splitn(3, '|');
        let (emitter, stage, name) = (parts.next()?, parts.next()?, parts.next()?);
        let found = effect.emitters.iter().find(|e| e.name == emitter)?;
        let stage_kind = ara_vfx::Stacks::STAGES.into_iter().find(|st| {
            serde_json::to_value(st)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .as_deref()
                == Some(stage)
        })?;
        let modules = match stage_kind {
            ara_vfx::Stage::Spawn => &found.stacks.spawn,
            ara_vfx::Stage::Initialize => &found.stacks.initialize,
            ara_vfx::Stage::Update => &found.stacks.update,
            ara_vfx::Stage::Render => &found.stacks.render,
        };
        let module = modules.iter().find(|m| m.name == name)?;
        Some((
            item.to_owned(),
            emitter.to_owned(),
            stage.to_owned(),
            stage_kind,
            module,
        ))
    });
    let mut out = col().child(heading(l, icons::SLIDERS_HORIZONTAL, "Selection", None));
    let Some((item, emitter, stage, stage_kind, module)) = picked else {
        return out.child(
            row().pad(12.0).child(
                text("Pick a module in an emitter to edit its values here.")
                    .text_size(11.5)
                    .color(c.muted_foreground.0)
                    .wrap(3),
            ),
        );
    };
    let value = ara_json::to_value(&module.module).unwrap_or_default();
    let kind = value["op"].as_str().unwrap_or("module").to_owned();
    let rail = stage_color(c, stage_kind);
    let mut tools = row().gap(2.0).align(Align::Center);
    for (op, glyph, label) in [
        ("up", icons::CHEVRON_UP, "Move up"),
        ("down", icons::CHEVRON_DOWN, "Move down"),
        ("duplicate", icons::COPY, "Duplicate"),
        ("delete", icons::TRASH, "Delete"),
    ] {
        let (doc, item) = (doc.to_owned(), item.clone());
        tools = tools.child(
            icon_button(l, Tone::Ghost, Size::Sm, glyph, label)
                .size(22.0)
                .key(format!("vfx/{op}/{item}"))
                .on_press(move |s: &mut Studio| edit(s, &doc, &item, op)),
        );
    }
    out = out
        .child(
            row()
                .pad_x(10.0)
                .pad_y(8.0)
                .gap(8.0)
                .align(Align::Center)
                .child(el().w(3.0).h(28.0).radius(1.5).fill(rail))
                .child(
                    col()
                        .grow(1.0)
                        .gap(2.0)
                        .child(
                            text(module_title(module))
                                .text_size(13.0)
                                .weight(600)
                                .color(c.foreground.0),
                        )
                        .child(
                            text(format!(
                                "{} · {}",
                                crate::names::sentence(&kind),
                                crate::names::sentence(&stage)
                            ))
                            .text_size(11.0)
                            .color(c.muted_foreground.0),
                        ),
                )
                .child(tools),
        )
        .children((!module.enabled).then(|| {
            row().pad_x(10.0).pad_y(4.0).child(
                text("Disabled: kept in the stack, skipped when the effect runs.")
                    .text_size(11.0)
                    .color(c.warning_foreground.0)
                    .wrap(2),
            )
        }))
        .child(heading(l, icons::SETTINGS, "Values", None))
        .children(settings(s, doc, &emitter, &stage, module, &value));
    out
}

/// The picked module's values as typed rows: numbers and text type in on Enter, flags check,
/// vectors type as `x, y, z`. Each change sets the whole module (one undo step).
fn settings(
    s: &Studio,
    doc: &str,
    emitter: &str,
    stage: &str,
    module: &ara_vfx::StackModule,
    value: &Value,
) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let mut out = Vec::new();
    for (key, v) in value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| *k != "op")
    {
        let name = crate::names::sentence(key);
        let set = {
            let (doc, emitter, stage, module, value, key) = (
                doc.to_owned(),
                emitter.to_owned(),
                stage.to_owned(),
                module.name.clone(),
                value.clone(),
                key.clone(),
            );
            move |s: &mut Studio, next: Value| {
                let mut settings = value.clone();
                settings[&key] = next;
                let command = json!({"cmd":"editor.vfx.module.set","args":{"emitter":emitter,"stage":stage,"name":module,"module":settings}});
                if let Err(error) =
                    s.editor
                        .apply_to(&doc, vec![command], "Edit VFX module parameter")
                {
                    s.editor.report_failure(&error);
                }
            }
        };
        // Size over Life's keys are a curve: drawn and edited as one, full width.
        if let ara_vfx::Module::SizeOverLife { curve } = &module.module {
            let item = format!("vfx/{emitter}/{stage}/{}", module.name);
            out.push(
                col()
                    .pad_x(10.0)
                    .pad_y(6.0)
                    .gap(6.0)
                    .child(
                        text("Size over life (m)")
                            .text_size(11.5)
                            .color(c.muted_foreground.0),
                    )
                    .child(super::curve::editor(
                        s,
                        &item,
                        curve,
                        Some((0.0, 1.0)),
                        2,
                        move |s: &mut Studio, curve| {
                            let saved =
                                ara_json::to_value(&ara_vfx::Module::SizeOverLife { curve })
                                    .unwrap_or_default();
                            set(s, saved["keys"].clone());
                        },
                    )),
            );
            continue;
        }
        let editor: El<Studio> = match v {
            Value::Bool(on) => {
                let on = *on;
                row().child(
                    checkbox(l, format!("vfx/set/{key}"), &name, on)
                        .on_press(move |s: &mut Studio| set(s, json!(!on))),
                )
            }
            Value::Number(_) | Value::String(_) | Value::Array(_) => {
                let shown = match v {
                    Value::String(t) => t.clone(),
                    Value::Array(items) => items
                        .iter()
                        .map(crate::vfx_edit::short)
                        .collect::<Vec<_>>()
                        .join(", "),
                    other => other.to_string(),
                };
                let template = v.clone();
                field(
                    l,
                    Size::Sm,
                    format!("vfx/set/{key}"),
                    None,
                    "",
                    &shown,
                    move |x| {
                        x.select_all_on_focus().on_submit(move |s: &mut Studio, t| {
                            let next = match &template {
                                Value::Number(_) => t.trim().parse::<f64>().ok().map(|n| json!(n)),
                                Value::Array(_) => {
                                    let parts: Option<Vec<f64>> =
                                        t.split(',').map(|p| p.trim().parse().ok()).collect();
                                    parts.map(|p| json!(p))
                                }
                                _ => Some(json!(t)),
                            };
                            if let Some(next) = next {
                                set(s, next);
                            }
                        })
                    },
                )
            }
            other => text(crate::vfx_edit::short(other))
                .text_size(11.5)
                .color(c.muted_foreground.0),
        };
        out.push(
            row()
                .min_h(26.0)
                .pad_x(10.0)
                .gap(8.0)
                .align(Align::Center)
                .child(
                    text(name)
                        .text_size(11.5)
                        .color(c.muted_foreground.0)
                        .w(0.0)
                        .grow(0.45),
                )
                .child(editor.w(0.0).grow(0.55)),
        );
    }
    out
}

/// Runs module operation `op` (`toggle`, `up`, `down`, `duplicate`, `delete`) on `item`
/// of effect `doc` (one undo step).
pub(super) fn edit(s: &mut Studio, doc: &str, item: &str, op: &str) {
    if let Err(error) = crate::vfx_edit::module_edit(&mut s.editor, doc, item, op) {
        s.editor.report_failure(&*error);
    }
}
