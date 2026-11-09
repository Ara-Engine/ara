//! The VFX tool (Niagara, Unity's VFX Graph): the open effect's emitters as module stacks
//! (spawn, initialize, update, render) on the left, and its live GPU preview in the middle
//! with a timeline (restart, play/pause, step, rate, time, particles). Pick a module to edit
//! its settings in place; its row moves it, duplicates or removes it; its checkbox keeps it
//! but skips it. Hide and Isolate change only the preview. The preview runs on its own worker
//! (`asset_tools::vfx_stream`); every edit is one undo step of the document's commands.
use std::collections::BTreeSet;
use std::sync::Arc;

use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, Axis, El, Justify, col, el, icon, image, row, text};

use super::Studio;
use crate::asset_tools::vfx_stream::VfxStream;

/// Preview rates, in percent of real time.
const RATES: [u32; 4] = [25, 50, 100, 200];

/// The VFX page's preview and view state (what to show, never the effect itself).
pub struct Page {
    stream: Option<VfxStream>,
    /// The newest preview frame, ready to draw.
    pub frame: Option<Arc<stillkit::Image>>,
    tick: u32,
    alive: u32,
    rate: u32,
    hz: u32,
    hidden: BTreeSet<String>,
    isolated: Option<String>,
    collapsed: BTreeSet<String>,
    /// What the preview runs (document and revision, playing, rate, hidden, isolated).
    shown: Option<(String, u64, bool, u32, usize, Option<String>)>,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            stream: None,
            frame: None,
            tick: 0,
            alive: 0,
            rate: 100,
            hz: 60,
            hidden: BTreeSet::new(),
            isolated: None,
            collapsed: BTreeSet::new(),
            shown: None,
        }
    }
}

/// The effect document the page edits: the selected resource when it is one, else the
/// project's first effect.
fn document(s: &Studio) -> Option<String> {
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

/// The host's pass: keep the preview running what the page shows (paused while the page is
/// not `visible`) and take its newest frame. Whether a new frame arrived.
pub(super) fn update(s: &mut Studio, visible: bool) -> bool {
    let Some(doc) = document(s).filter(|_| visible) else {
        if let Some(stream) = s.vfx.stream.as_mut() {
            stream.pause();
        }
        s.vfx.shown = None;
        return false;
    };
    let revision = s.editor.host.store().revision();
    let playing = s.editor.state.preview_playing;
    let page = &s.vfx;
    let key = (
        doc.clone(),
        revision,
        playing,
        page.rate,
        page.hidden.len(),
        page.isolated.clone(),
    );
    if page.shown.as_ref() != Some(&key)
        && let Ok(mut effect) = effect(s, &doc)
    {
        let page = &mut s.vfx;
        page.hz = effect.hz;
        effect.emitters.retain(|e| match &page.isolated {
            Some(only) => &e.name == only,
            None => !page.hidden.contains(&e.name),
        });
        let root = s.editor.host.root().to_owned();
        page.stream
            .get_or_insert_with(|| VfxStream::new(&root))
            .show(effect, playing, page.rate);
        page.shown = Some(key);
    }
    let page = &mut s.vfx;
    let Some(stream) = page.stream.as_mut() else {
        return false;
    };
    let mut landed = None;
    for frame in stream.poll() {
        match frame {
            Ok(frame) => landed = Some(frame),
            Err(error) => s.editor.report_failure(&*anyhow::anyhow!("{error}")),
        }
    }
    let Some(frame) = landed else {
        return false;
    };
    page.tick = frame.tick;
    page.alive = frame.alive;
    // The worker's RGBA as premultiplied BGRA, as stillkit draws images.
    let (width, height) = frame.image.dimensions();
    let mut pixels = frame.image.into_raw();
    for px in pixels.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        let mul = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
        let (r, g, b) = (mul(px[0]), mul(px[1]), mul(px[2]));
        px[0] = b;
        px[1] = g;
        px[2] = r;
    }
    page.frame = Some(Arc::new(stillkit::Image {
        width,
        height,
        pixels,
    }));
    true
}

/// Whether the preview is running (the host wakes for its frames).
pub(super) fn running(s: &Studio) -> bool {
    s.vfx.shown.is_some() && s.editor.state.preview_playing
}

/// The page.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let Some(doc) = document(s) else {
        return col().grow(1.0).justify(Justify::Center).child(empty(
            l,
            icons::SPARKLES,
            "No effect yet",
            "Effects are emitters of particles, each a stack of modules that run in order.",
            Some(
                button(l, Tone::Primary, Size::Sm, Some(icons::PLUS), "New effect")
                    .key("vfx/new")
                    .on_press(|s: &mut Studio| create(s)),
            ),
        ));
    };
    let fx = effect(s, &doc);
    let stacks = match &fx {
        Ok(effect) => {
            let mut list = col()
                .key("vfx/stacks")
                .h(0.0)
                .grow(1.0)
                .pad(4.0)
                .gap(2.0)
                .scroll(Axis::Vertical);
            for emitter in &effect.emitters {
                list = list.child(emitter_header(s, emitter));
                if !s.vfx.collapsed.contains(&emitter.name) {
                    list = list.children(stack(s, &doc, emitter));
                }
            }
            list
        }
        Err(_) => col().grow(1.0),
    };
    let side = col()
        .w(320.0)
        .min_w(260.0)
        .fill(super::style::tones(l).panel)
        .child(
            row()
                .h(28.0)
                .pad_x(10.0)
                .align(Align::Center)
                .child(
                    text("Emitters")
                        .text_size(12.0)
                        .weight(600)
                        .color(c.foreground.0)
                        .grow(1.0),
                )
                .children(fx.as_ref().ok().map(|e| {
                    text(e.emitters.len().to_string())
                        .text_size(11.0)
                        .color(c.muted_foreground.0)
                })),
        )
        .child(stacks);
    let preview = match &s.vfx.frame {
        Some(frame) => image(frame.clone())
            .size(360.0)
            .radius(super::style::RADIUS),
        None => el()
            .size(360.0)
            .align(Align::Center)
            .justify(Justify::Center)
            .child(
                text("Compiling the effect…")
                    .text_size(12.0)
                    .color(c.muted_foreground.0),
            ),
    };
    let status = match &fx {
        Ok(_) => text("Compiled")
            .text_size(11.5)
            .color(c.success_foreground.0),
        Err(error) => text(format!(
            "Compile failed · {error} · the last good preview keeps running"
        ))
        .text_size(11.5)
        .color(c.destructive_foreground.0)
        .wrap(2),
    };
    let main = col()
        .key("vfx/preview")
        .grow(1.0)
        .min_w(0.0)
        .child(
            col()
                .grow(1.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .fill(c.background.0)
                .child(preview),
        )
        .child(timeline(s))
        .child(row().h(26.0).pad_x(10.0).align(Align::Center).child(status));
    row().grow(1.0).child(side).child(main)
}

/// An emitter's header: collapse, name, and the preview-only Hide and Isolate.
fn emitter_header(s: &Studio, emitter: &ara_vfx::Emitter) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let name = emitter.name.clone();
    let collapsed = s.vfx.collapsed.contains(&name);
    let hidden = s.vfx.hidden.contains(&name);
    let isolated = s.vfx.isolated.as_deref() == Some(name.as_str());
    let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
    row()
        .key(format!("vfx/emitter/{name}"))
        .h(28.0)
        .pad_x(6.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(super::style::tones(l).band)
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
            .on_press(move |s: &mut Studio| {
                if !s.vfx.collapsed.remove(&n1) {
                    s.vfx.collapsed.insert(n1.clone());
                }
            }),
        )
        .child(
            icon(icons::SPARKLE)
                .size(13.0)
                .color(c.warning_foreground.0),
        )
        .child(
            text(name.clone())
                .text_size(12.5)
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
            .size(20.0)
            .on_press(move |s: &mut Studio| {
                if !s.vfx.hidden.remove(&n2) {
                    s.vfx.hidden.insert(n2.clone());
                }
            }),
        )
        .child(
            toggle_button(
                l,
                Size::Sm,
                icons::FOCUS,
                "Isolate in the preview",
                isolated,
            )
            .size(20.0)
            .on_press(move |s: &mut Studio| {
                s.vfx.isolated = if s.vfx.isolated.as_deref() == Some(n3.as_str()) {
                    None
                } else {
                    Some(n3.clone())
                };
            }),
        )
}

/// An emitter's four stages and their modules, each stage with its Add.
fn stack(s: &Studio, doc: &str, emitter: &ara_vfx::Emitter) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let mut out = Vec::new();
    for stage in ara_vfx::Stacks::STAGES {
        let stage_name = serde_json::to_value(stage)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let (modules, rail) = match stage {
            ara_vfx::Stage::Spawn => (&emitter.stacks.spawn, c.success_foreground.0),
            ara_vfx::Stage::Initialize => (&emitter.stacks.initialize, c.chart_2.0),
            ara_vfx::Stage::Update => (&emitter.stacks.update, c.warning_foreground.0),
            ara_vfx::Stage::Render => (&emitter.stacks.render, c.chart_4.0),
        };
        let (doc_add, emitter_name, stage_add, index) = (
            doc.to_owned(),
            emitter.name.clone(),
            stage_name.clone(),
            modules.len(),
        );
        out.push(
            row()
                .h(24.0)
                .pad_x(8.0)
                .gap(6.0)
                .align(Align::Center)
                .child(el().w(3.0).h(12.0).radius(1.5).fill(rail))
                .child(
                    text(stage_name.to_uppercase())
                        .text_size(10.5)
                        .weight(600)
                        .color(c.muted_foreground.0)
                        .grow(1.0),
                )
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, &format!("Add a {stage_name} module"))
                        .size(20.0)
                        .key(format!("vfx/add/{}/{stage_name}", emitter.name))
                        .on_press(move |s: &mut Studio| {
                            let module = crate::shell::vfx_panel::default_module(stage, s.editor.host.store().revision());
                            let command = json!({"cmd":"vfx.stack.edit","args":{"emitter":emitter_name,"stage":stage_add,"name":module.name,"operation":"insert","index":index,"module":module}});
                            if let Err(error) = s.editor.apply_to(&doc_add, vec![command], "Add VFX module") {
                                s.editor.report_failure(&error);
                            }
                        }),
                ),
        );
        for module in modules {
            out.extend(module_row(s, doc, &emitter.name, &stage_name, module, rail));
        }
    }
    out
}

/// A module: its rail, name and kind, a summary of its values, its checkbox; picked, its
/// tools and its settings below it.
fn module_row(
    s: &Studio,
    doc: &str,
    emitter: &str,
    stage: &str,
    module: &ara_vfx::StackModule,
    rail: stillkit::Rgba,
) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let item = format!("{emitter}|{stage}|{}", module.name);
    let picked = s.editor.state.resource_item.as_deref() == Some(item.as_str());
    let value = ara_json::to_value(&module.module).unwrap_or_default();
    let kind = value["op"].as_str().unwrap_or("module").to_owned();
    let summary = value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| *k != "op")
        .map(|(k, v)| {
            format!(
                "{} {}",
                crate::shell::inspector_panel::sentence(k).to_lowercase(),
                crate::shell::vfx_panel::short(v)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let title = if module.name == kind {
        crate::shell::inspector_panel::sentence(&kind)
    } else {
        module.name.clone()
    };
    let (doc_pick, item_pick) = (doc.to_owned(), item.clone());
    let hover = c.hover_surface.0;
    let mut out = vec![
        super::style::chosen(l, row(), picked)
            .key(format!("vfx/module/{item}"))
            .h(26.0)
            .pad_x(8.0)
            .gap(8.0)
            .align(Align::Center)
            .radius(super::style::RADIUS)
            .label(title.clone())
            .hovered(move |e| if picked { e } else { e.fill(hover) })
            .on_press(move |s: &mut Studio| {
                s.act(
                    "editor.workspace",
                    json!({"resource": doc_pick, "item": item_pick}),
                )
            })
            .child(el().w(2.0).h(14.0).radius(1.0).fill(rail))
            .child(
                checkbox(l, format!("vfx/enable/{item}"), "Run", module.enabled).on_press({
                    let (doc, item) = (doc.to_owned(), item.clone());
                    move |s: &mut Studio| edit(s, &doc, &item, "toggle")
                }),
            )
            .child(
                text(title)
                    .text_size(12.0)
                    .weight(500)
                    .color(if module.enabled {
                        c.foreground.0
                    } else {
                        c.muted_foreground.0
                    }),
            )
            .child(
                text(if module.enabled {
                    summary
                } else {
                    "disabled".into()
                })
                .text_size(11.0)
                .color(c.muted_foreground.0)
                .wrap(1)
                .w(0.0)
                .grow(1.0),
            ),
    ];
    if picked {
        let mut tools = row().pad_x(30.0).gap(2.0).align(Align::Center);
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
        out.push(tools);
        out.extend(settings(s, doc, emitter, stage, module, &value));
    }
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
        let name = crate::shell::inspector_panel::sentence(key);
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
                        .map(crate::shell::vfx_panel::short)
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
                        x.on_submit(move |s: &mut Studio, t| {
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
            other => text(crate::shell::vfx_panel::short(other))
                .text_size(11.5)
                .color(c.muted_foreground.0),
        };
        out.push(
            row()
                .min_h(24.0)
                .pad_x(30.0)
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

fn edit(s: &mut Studio, doc: &str, item: &str, op: &str) {
    if let Err(error) = crate::shell::vfx_panel::module_edit(&mut s.editor, doc, item, op) {
        s.editor.report_failure(&*error);
    }
}

/// The timeline: restart, play or pause, step, rate, time and particles.
fn timeline(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let playing = s.editor.state.preview_playing;
    let seconds = s.vfx.tick as f32 / s.vfx.hz.max(1) as f32;
    let mut rates = row().gap(1.0);
    for rate in RATES {
        let on = s.vfx.rate == rate;
        rates = rates.child(
            super::style::chosen(l, row(), on)
                .key(format!("vfx/rate/{rate}"))
                .h(22.0)
                .pad_x(7.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(format!("Run at {rate}%"))
                .selected(on)
                .on_press(move |s: &mut Studio| s.vfx.rate = rate)
                .child(
                    text(format!("{rate}%"))
                        .text_size(11.0)
                        .family(mono())
                        .color(if on {
                            c.foreground.0
                        } else {
                            c.muted_foreground.0
                        }),
                ),
        );
    }
    row()
        .h(34.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .fill(super::style::tones(l).strip)
        .child(
            icon_button(l, Tone::Ghost, Size::Sm, icons::REFRESH_CCW, "Restart")
                .key("vfx/restart")
                .on_press(|s: &mut Studio| {
                    if let Some(stream) = s.vfx.stream.as_mut() {
                        stream.restart();
                    }
                }),
        )
        .child(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                if playing { icons::PAUSE } else { icons::PLAY },
                if playing { "Pause" } else { "Play" },
            )
            .key("vfx/play")
            .on_press(move |s: &mut Studio| s.editor.state.preview_playing = !playing),
        )
        .child(enabled(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::STEP_FORWARD,
                "Step one frame",
            )
            .key("vfx/step")
            .on_press(|s: &mut Studio| {
                if let Some(stream) = s.vfx.stream.as_mut() {
                    stream.step();
                }
            }),
            !playing,
        ))
        .child(rates)
        .child(el().grow(1.0))
        .child(
            text(format!(
                "{seconds:.2} s · tick {} · {} particles",
                s.vfx.tick, s.vfx.alive
            ))
            .text_size(11.5)
            .family(mono())
            .color(c.muted_foreground.0),
        )
}

/// Starts a new effect in the open Assets folder and opens it.
fn create(s: &mut Studio) {
    let folder = s.editor.state.asset_folder.clone();
    let first = s.editor.host.store().revision();
    let workspace = s.editor.host.store().workspace();
    let path = (first..first.saturating_add(10_000))
        .map(|n| {
            let leaf = format!("vfx-{n}.graph");
            if folder.is_empty() {
                leaf
            } else {
                format!("{folder}/{leaf}")
            }
        })
        .find(|path| workspace.find(path).is_none());
    let Some(path) = path else { return };
    match s.editor.action(
        "editor.resource.create",
        json!({"path": path, "domain": "vfx"}),
    ) {
        Ok(result) => s.act(
            "editor.workspace",
            json!({"resource": result["doc"], "asset": ""}),
        ),
        Err(error) => s.editor.report_failure(&error),
    }
}
