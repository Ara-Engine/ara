//! The Material and Animation tools: a material graph, or an animation controller's state
//! machine, on the shared node canvas (`crate::graph::Canvas`, the legacy window's), drawn
//! into the page's own native layer ([`PAGES`]) like the Scene. Drag nodes, drag from a port
//! to connect, drag empty space to box-select, middle or right drag to pan, the wheel to
//! zoom; F frames the graph; Space or Add Node searches the material library. Every edit is
//! a command on the document (one undo step per gesture), the same ones agents send.
use std::ffi::c_void;
use std::ptr::NonNull;

use ara_kit::icons;
use ara_kit::ui::*;
use ara_ui::paint::{Box2, DisplayList};
use ara_ui::style::Color;
use serde_json::{Value, json};
use stillkit::el::{
    Align, Axis, El, Handled, Justify, PointerPhase, Side, col, el, native, row, text,
};

use super::Studio;
use crate::graph::{Canvas, Edit, Theme};

/// The graph pages and their native layers.
pub(crate) const PAGES: [(&str, &str); 2] = [
    ("material", "graph-material"),
    ("animation", "graph-animation"),
];

/// The native layer of `domain`'s page.
fn layer(domain: &str) -> &'static str {
    PAGES
        .iter()
        .find(|(d, _)| *d == domain)
        .map_or("graph-material", |(_, l)| l)
}

/// The graph pages' state: the canvas for the open document and the text it shapes with.
#[derive(Default)]
pub struct Page {
    text: Option<ara_ui_text::TextSystem>,
    shown: Option<Shown>,
    dragging: bool,
    /// The canvas changed since it was last drawn.
    pub dirty: bool,
    /// The Add Node palette's query, while it is open.
    pub adding: Option<String>,
}

/// The document a canvas shows, at which store revision.
struct Shown {
    domain: &'static str,
    doc: String,
    revision: u64,
    canvas: Canvas,
    /// Opened and not yet framed to its laid-out size.
    fresh: bool,
}

/// The document `domain`'s page edits: the selected resource when it is one of its kind,
/// else the project's first.
fn document(s: &Studio, domain: &str) -> Option<String> {
    let workspace = s.editor.host.store().workspace();
    let matches = |doc: &ara_doc::Document| {
        if domain == "animation" {
            return doc.kind == "animation";
        }
        doc.kind == "graph"
            && doc
                .record("g_meta")
                .and_then(|r| r.get("domain"))
                .and_then(Value::as_str)
                == Some(domain)
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

/// Loads `domain`'s document into the canvas if it changed (another document, or a newer
/// revision of it); the same document refreshes in place, keeping its view and selection.
pub(super) fn sync(s: &mut Studio, domain: &'static str) {
    let Some(doc) = document(s, domain) else {
        s.graph.shown = None;
        return;
    };
    let revision = s.editor.host.store().revision();
    if s.graph
        .shown
        .as_ref()
        .is_some_and(|g| g.domain == domain && g.doc == doc && g.revision == revision)
    {
        return;
    }
    let workspace = s.editor.host.store().workspace();
    let Some(document) = workspace.doc(&doc) else {
        return;
    };
    let graph = if domain == "animation" {
        // The state machine, laid out from its sidecar layout document.
        let layout = workspace
            .find(&crate::authoring::animation::layout_path(&doc))
            .and_then(|id| workspace.doc(id.as_str()))
            .and_then(|d| ara_graph::Graph::from_document(d).ok())
            .map(|g| g.editor)
            .unwrap_or_default();
        crate::authoring::animation::projection(document, layout, false)
            .map_err(|e| anyhow::anyhow!("{e}"))
    } else {
        ara_graph::Graph::from_document(document).map_err(|e| anyhow::anyhow!("{e}"))
    };
    let graph = match graph {
        Ok(graph) => graph,
        Err(error) => {
            s.editor.report_failure(&*error);
            s.graph.shown = None;
            return;
        }
    };
    if s.graph.text.is_none() {
        match crate::shell::text_system() {
            Ok(text) => s.graph.text = Some(text),
            Err(error) => {
                s.editor.report_failure(&*error);
                return;
            }
        }
    }
    let page = &mut s.graph;
    let Some(text) = page.text.as_mut() else {
        return;
    };
    match page
        .shown
        .as_mut()
        .filter(|g| g.domain == domain && g.doc == doc)
    {
        Some(shown) => {
            shown.canvas.refresh(graph, text);
            shown.revision = revision;
        }
        None => {
            page.shown = Some(Shown {
                domain,
                doc,
                revision,
                canvas: Canvas::new(graph, text),
                fresh: true,
            })
        }
    }
    page.dirty = true;
}

/// Draws the canvas into the page's layer. `extent` is in physical pixels.
/// # Safety
/// `layer` is the live `CAMetalLayer` of the graph page.
pub(super) unsafe fn draw(
    s: &mut Studio,
    target: &mut super::layer::Layer,
    renderer: &ara_render::Renderer,
    layer: NonNull<c_void>,
    extent: [u32; 2],
    scale: f32,
) -> anyhow::Result<()> {
    let rect = Box2 {
        x: 0.0,
        y: 0.0,
        w: extent[0] as f32 / scale,
        h: extent[1] as f32 / scale,
    };
    let theme = theme(&s.look);
    let page = &mut s.graph;
    let (Some(shown), Some(text)) = (page.shown.as_mut(), page.text.as_ref()) else {
        return Ok(());
    };
    if std::mem::take(&mut shown.fresh) {
        shown.canvas.camera = shown.canvas.framed_readable(rect);
    }
    let mut list = DisplayList::default();
    shown.canvas.paint(&mut list, rect, theme);
    // SAFETY: forwarded caller contract.
    unsafe { target.draw(renderer, layer, extent, scale, &list, &text.assets)? };
    page.dirty = false;
    Ok(())
}

/// The canvas colours from the studio's tokens.
fn theme(l: &ara_kit::look::Look) -> Theme {
    let c = &l.c;
    let color = |rgba: stillkit::Rgba| {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Color(b(rgba.r), b(rgba.g), b(rgba.b), b(rgba.a))
    };
    // Unreal's contrast: dark nodes on lighter graph paper, coloured headers.
    Theme {
        background: color(ara_kit::ui::mix(c.card.0, c.raised.0, 0.35)),
        card: color(ara_kit::ui::mix(c.background.0, c.card.0, 0.4)),
        raised: color(c.raised.0),
        border: color(c.border.0),
        separator: color(c.separator_strong.0),
        foreground: color(c.foreground.0),
        muted: color(c.muted_foreground.0),
        accent: color(c.primary.0),
        error: color(c.destructive_foreground.0),
        success: color(c.success_foreground.0),
        chart: [
            c.chart_1.0,
            c.chart_2.0,
            c.chart_3.0,
            c.chart_4.0,
            c.chart_5.0,
            c.chart_6.0,
        ]
        .map(color),
        shadow: Color(0, 0, 0, if l.dark { 140 } else { 40 }),
        select: color(c.warning.0),
    }
}

/// `domain`'s page: its toolbar and the canvas, or a way to start one.
pub(super) fn view(s: &Studio, domain: &'static str) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let animation = domain == "animation";
    let Some(doc) = document(s, domain) else {
        let (glyph, title, detail, new) = if animation {
            (
                icons::PERSON_STANDING,
                "No animation controller yet",
                "Controllers are state machines: states play clips, transitions move between them.",
                "New controller",
            )
        } else {
            (
                icons::PALETTE,
                "No material yet",
                "Materials are node graphs: start one, then add nodes and connect them.",
                "New material",
            )
        };
        return col().grow(1.0).justify(Justify::Center).child(empty(
            l,
            glyph,
            title,
            detail,
            Some(
                button(l, Tone::Primary, Size::Sm, Some(icons::PLUS), new)
                    .key(format!("{domain}/new"))
                    .on_press(move |s: &mut Studio| super::asset_create::create(s, domain)),
            ),
        ));
    };
    let path = s
        .editor
        .host
        .store()
        .workspace()
        .path_of(&doc)
        .unwrap_or_default()
        .to_owned();
    let (folder, file) = path.rsplit_once('/').unwrap_or(("", &path));
    let mut add = button(l, Tone::Ghost, Size::Sm, Some(icons::PLUS), "Add Node")
        .key("material/add")
        .tooltip("Add a node (Space)")
        .on_press(|s: &mut Studio| {
            s.graph.adding = match s.graph.adding {
                Some(_) => None,
                None => Some(String::new()),
            };
            s.local.focus = Some("material/add/search");
        });
    if let Some(query) = s.graph.adding.as_ref().filter(|_| !animation) {
        add = add.overlay(
            stillkit::el::popover(palette(s, &doc, query))
                .side(Side::Bottom)
                .align(Align::Start)
                .offset(4.0)
                .on_dismiss(|s: &mut Studio| s.graph.adding = None),
        );
    }
    let toolbar = row()
        .h(super::style::TOOLBAR + 6.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .child(
            text(folder.to_owned())
                .text_size(12.0)
                .color(c.muted_foreground.0),
        )
        .children(
            (!folder.is_empty()).then(|| text("/").text_size(12.0).color(c.muted_foreground.0)),
        )
        .child(
            text(file.to_owned())
                .text_size(12.0)
                .weight(600)
                .color(c.foreground.0),
        )
        .child(el().grow(1.0))
        .children((!animation).then_some(add))
        .children(animation.then(|| {
            let edit = |kind: &'static str, label: &str, glyph| {
                let doc = doc.clone();
                button(l, Tone::Ghost, Size::Sm, Some(glyph), label)
                    .key(format!("animation/add/{kind}"))
                    .on_press(move |s: &mut Studio| {
                        if let Err(error) =
                            crate::shell::animation_panel::edit(&mut s.editor, &doc, kind)
                        {
                            s.editor.report_failure(&*error);
                        }
                    })
            };
            row()
                .gap(2.0)
                .child(edit("state", "Add State", icons::PLUS))
                .child(edit("blend", "Add Blend", icons::SPLINE))
        }))
        .child(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::FOCUS,
                "Frame the graph (F)",
            )
            .key(format!("{domain}/frame"))
            .on_press(|s: &mut Studio| frame(s)),
        );
    let canvas = native(layer(domain))
        .key("graph/view")
        .grow(1.0)
        .w_full()
        .focusable()
        .label(if animation {
            "State machine"
        } else {
            "Material graph"
        })
        .on_pointer(pointer)
        .on_wheel(|s: &mut Studio, w, _| {
            let rect = view_rect(s);
            let (left, top) = origin(s);
            if let Some(shown) = s.graph.shown.as_mut() {
                shown
                    .canvas
                    .zoom([w.at.x - left, w.at.y - top], w.delta, rect);
                s.graph.dirty = true;
            }
            true
        })
        .on_key(move |s: &mut Studio, key, _| match key.0 {
            0x46 => {
                frame(s);
                Handled::Yes
            }
            0x20 if !animation => {
                s.graph.adding = Some(String::new());
                s.local.focus = Some("material/add/search");
                Handled::Yes
            }
            _ => Handled::No,
        });
    let body = row()
        .grow(1.0)
        .min_h(0.0)
        .child(col().grow(1.0).min_w(0.0).child(canvas));
    let body = if animation {
        body
    } else {
        body.child(details(s, &doc))
    };
    col().grow(1.0).child(toolbar).child(body)
}

/// The material's Details (Unreal) / Graph Inspector (Shader Graph): with nothing selected,
/// its parameters, as Shader Graph's Blackboard lists them; with a node, its value (typed
/// here), what feeds each input and what its output feeds.
fn details(s: &Studio, doc: &str) -> El<Studio> {
    use crate::graph::style;
    let l = &s.look;
    let c = &l.c;
    let t = super::style::tones(l);
    let mut out = col()
        .key("material/details")
        .w(300.0)
        .min_w(240.0)
        .fill(t.panel)
        .scroll(Axis::Vertical);
    let Some(shown) = s.graph.shown.as_ref().filter(|g| g.domain == "material") else {
        return out;
    };
    let graph = shown.canvas.graph();
    let material = ara_material::Material::from_graph(graph).ok();
    let band = |title: String, caption: String, glyph| {
        row()
            .h(28.0)
            .pad_x(10.0)
            .gap(8.0)
            .align(Align::Center)
            .fill(t.band)
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
            .child(text(caption).text_size(11.0).color(c.muted_foreground.0))
    };
    let prop = |label: String, value: El<Studio>| {
        row()
            .min_h(24.0)
            .pad_x(10.0)
            .gap(8.0)
            .align(Align::Center)
            .child(
                text(label)
                    .text_size(12.0)
                    .color(c.muted_foreground.0)
                    .w(0.0)
                    .grow(0.4),
            )
            .child(value.w(0.0).grow(0.6))
    };
    let selection = &shown.canvas.selection;
    if selection.is_empty() {
        out = out.child(band(
            "Parameters".into(),
            String::new(),
            icons::SLIDERS_HORIZONTAL,
        ));
        let mut any = false;
        for (id, node) in &graph.nodes {
            if node.kind != "material.parameter" {
                continue;
            }
            any = true;
            let name = node
                .payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_owned();
            let ty =
                style::short_type(node.payload.get("ty").and_then(Value::as_str).unwrap_or(""));
            let (pick, hover) = (id.clone(), c.hover_surface.0);
            out = out.child(
                row()
                    .key(format!("material/param/{id}"))
                    .h(24.0)
                    .pad_x(10.0)
                    .gap(8.0)
                    .align(Align::Center)
                    .hovered(move |e| e.fill(hover))
                    .on_press(move |s: &mut Studio| {
                        if let Some(shown) = s.graph.shown.as_mut() {
                            shown.canvas.selection = std::iter::once(pick.clone()).collect();
                            s.graph.dirty = true;
                        }
                    })
                    .child(el().size(8.0).radius(4.0).fill(c.info.0))
                    .child(text(name).text_size(12.0).color(c.foreground.0).grow(1.0))
                    .child(text(ty).text_size(11.0).color(c.muted_foreground.0)),
            );
        }
        if !any {
            out = out.child(
                row().pad(10.0).child(
                    text("No parameters. Add a Parameter node to expose a value to instances.")
                        .text_size(11.5)
                        .color(c.muted_foreground.0)
                        .wrap(3),
                ),
            );
        }
        return out;
    }
    for id in selection {
        let Some(node) = graph.nodes.get(id) else {
            continue;
        };
        let op = node.kind.strip_prefix("material.").unwrap_or(&node.kind);
        let ty = node.payload.get("ty").and_then(Value::as_str).unwrap_or("");
        let name = node
            .payload
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| style::op_title(op));
        let kind = match op {
            "parameter" => format!("Parameter · {}", style::short_type(ty)),
            "literal" => format!("Constant · {}", style::short_type(ty)),
            "output" => "Material output".into(),
            _ => style::op_title(op),
        };
        out = out.child(band(name, kind, icons::CONTRAST));
        let doc_line = match op {
            "output" => "The material's typed outputs for its domain.",
            "parameter" => "A value material instances and scripts can change.",
            "literal" => "A typed constant, folded into the shader.",
            _ => ara_material::library::get(op).map_or("", |b| b.doc),
        };
        if !doc_line.is_empty() {
            out = out.child(
                row().pad_x(10.0).pad_y(6.0).child(
                    text(doc_line)
                        .text_size(11.5)
                        .color(c.muted_foreground.0)
                        .wrap(4),
                ),
            );
        }
        let value = material.as_ref().and_then(|m| {
            if let Some(p) = m.parameters.get(id) {
                ara_json::to_value(&p.default).ok()
            } else if let Some(ara_material::Expr::Literal { values }) =
                m.bindings.get(id).map(|b| &b.expr)
            {
                Some(json!(values))
            } else {
                None
            }
        });
        if let Some(value) = value {
            let shown = value
                .as_array()
                .map(|v| {
                    v.iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| value.to_string());
            let (doc, node_id) = (doc.to_owned(), id.clone());
            out = out.child(prop(
                if op == "parameter" { "Default" } else { "Value" }.into(),
                field(l, Size::Sm, format!("material/value/{id}"), None, "", &shown, move |x| {
                    x.on_submit(move |s: &mut Studio, t| {
                        // JSON, or lanes typed as `0.18, 0.42, 0.9`.
                        let Ok(mut value) = serde_json::from_str::<Value>(t)
                            .or_else(|_| serde_json::from_str(&format!("[{}]", t.trim())))
                        else {
                            return;
                        };
                        if value.is_number() {
                            value = json!([value]);
                        }
                        let command = json!({"cmd": "editor.material.value.set", "args": {"node": node_id, "value": value}});
                        if let Err(error) = s.editor.apply_to(&doc, vec![command], "Edit material node value") {
                            s.editor.report_failure(&error);
                        }
                    })
                }),
            ));
        }
        let inputs: Vec<_> = node
            .ports
            .iter()
            .filter(|(_, p)| p.direction == ara_graph::Direction::Input)
            .collect();
        if !inputs.is_empty() {
            out = out.child(band("Inputs".into(), String::new(), icons::ARROW_RIGHT));
        }
        for (port, socket) in inputs {
            let source = graph
                .edges
                .values()
                .find(|e| e.to.node == *id && e.to.port == *port)
                .and_then(|e| graph.nodes.get(&e.from.node))
                .map(|from| {
                    from.payload
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            style::op_title(from.kind.trim_start_matches("material."))
                        })
                });
            let pin = style::pin_label(node, port, socket.direction, &socket.ty);
            out = out.child(prop(
                pin,
                row()
                    .gap(6.0)
                    .align(Align::Center)
                    .child(
                        text(source.clone().unwrap_or_else(|| "Not connected".into()))
                            .text_size(12.0)
                            .color(if source.is_some() {
                                c.foreground.0
                            } else {
                                c.muted_foreground.0
                            })
                            .grow(1.0),
                    )
                    .child(
                        text(style::short_type(&socket.ty))
                            .text_size(10.5)
                            .color(c.muted_foreground.0),
                    ),
            ));
        }
        for (port, socket) in node
            .ports
            .iter()
            .filter(|(_, p)| p.direction == ara_graph::Direction::Output)
        {
            let users = graph
                .edges
                .values()
                .filter(|e| e.from.node == *id && e.from.port == *port)
                .count();
            let uses = match users {
                0 => "Unused".to_owned(),
                1 => "Feeds 1 input".to_owned(),
                n => format!("Feeds {n} inputs"),
            };
            out = out.child(prop(
                format!("Output · {}", style::short_type(&socket.ty)),
                text(uses).text_size(12.0).color(if users > 0 {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                }),
            ));
        }
    }
    out
}

/// Where the canvas is in the window (design px), from the last layout.
fn origin(s: &Studio) -> (f32, f32) {
    s.local
        .rects
        .get("graph/view")
        .map_or((0.0, 0.0), |r| (r.left, r.top))
}

/// The canvas's own rect (design px, at its origin), from the last layout.
fn view_rect(s: &Studio) -> Box2 {
    let (w, h) = s
        .local
        .rects
        .get("graph/view")
        .map_or((800.0, 600.0), |r| (r.width(), r.height()));
    Box2 {
        x: 0.0,
        y: 0.0,
        w,
        h,
    }
}

/// A press, drag or release on the canvas: the canvas's own gesture, then its edits as
/// commands on the document.
fn pointer(s: &mut Studio, e: &stillkit::el::PointerEvent, _: &mut stillkit::el::Tx) -> bool {
    let rect = Box2 {
        x: 0.0,
        y: 0.0,
        w: e.size.0,
        h: e.size.1,
    };
    let p = [e.local.x, e.local.y];
    let Some(shown) = s.graph.shown.as_mut() else {
        return false;
    };
    match e.phase {
        PointerPhase::Down => {
            let button = match e.button {
                stillkit::Button::Left => 1,
                stillkit::Button::Middle => 2,
                _ => 3,
            };
            let additive = e.mods.shift || e.mods.meta || e.mods.ctrl;
            shown.canvas.begin(p, button, additive, rect);
            s.graph.dragging = true;
        }
        PointerPhase::Move if s.graph.dragging => shown.canvas.motion(p),
        PointerPhase::Up if s.graph.dragging => {
            s.graph.dragging = false;
            match shown.canvas.end(p, rect) {
                Ok(edits) => {
                    let (domain, doc) = (shown.domain, shown.doc.clone());
                    if domain == "animation" {
                        submit_animation(s, &doc, edits);
                    } else {
                        submit(s, &doc, edits);
                    }
                }
                Err(error) => shown.canvas.diagnostic = Some(error),
            }
        }
        PointerPhase::Cancel => {
            s.graph.dragging = false;
            shown.canvas.cancel();
        }
        _ => return false,
    }
    s.graph.dirty = true;
    true
}

/// The canvas's edits as material commands on `doc` (one undo step).
fn submit(s: &mut Studio, doc: &str, edits: Vec<Edit>) {
    let mut commands = Vec::new();
    for edit in edits {
        match edit {
            Edit::Move(nodes) => commands.extend(nodes.into_iter().map(|(node, position)| {
                json!({"cmd": "graph.node.move", "args": {"node": node, "position": position}})
            })),
            Edit::Connect(from, to) => commands.push(json!({
                "cmd": "material.connect",
                "args": {"source": from.node, "node": to.node, "port": to.port}
            })),
            Edit::Layout(layout) => {
                commands.push(json!({"cmd": "graph.editor.set", "args": {"editor": layout}}))
            }
        }
    }
    if !commands.is_empty()
        && let Err(error) = s.editor.apply_to(doc, commands, "Edit material graph")
    {
        s.editor.report_failure(&error);
    }
}

/// The canvas's edits on an animation controller: node moves and layout go to its layout
/// document, a connection adds a transition (or feeds a blend input).
fn submit_animation(s: &mut Studio, doc: &str, edits: Vec<Edit>) {
    let Some(shown) = s.graph.shown.as_ref() else {
        return;
    };
    let mut layout = shown.canvas.graph().editor.clone();
    let graph = shown.canvas.graph().clone();
    let mut moved = false;
    for edit in edits {
        let result = match edit {
            Edit::Move(nodes) => {
                layout.positions.extend(nodes);
                moved = true;
                Ok(())
            }
            Edit::Layout(next) => {
                layout = next;
                moved = true;
                Ok(())
            }
            Edit::Connect(from, to) => {
                crate::shell::animation_panel::connect(&mut s.editor, doc, &graph, from, to)
            }
        };
        if let Err(error) = result {
            s.editor.report_failure(&*error);
        }
    }
    if moved {
        s.act(
            "editor.animation.layout",
            json!({"doc": doc, "editor": layout}),
        );
    }
}

/// Fits the whole graph in view.
fn frame(s: &mut Studio) {
    let rect = view_rect(s);
    if let Some(shown) = s.graph.shown.as_mut() {
        shown.canvas.camera = shown.canvas.framed_readable(rect);
        s.graph.dirty = true;
    }
}

/// Add Node: the material library, searched, by category; a pick lands at the view's centre.
fn palette(s: &Studio, doc: &str, query: &str) -> El<Studio> {
    use crate::graph::style;
    let l = &s.look;
    let c = &l.c;
    let mut list = col()
        .key("material/add/list")
        .max_h(360.0)
        .pad_x(6.0)
        .pad_y(2.0)
        .gap(1.0)
        .scroll(Axis::Vertical);
    let registry = ara_material::library::registry();
    let mut found: Vec<(usize, i32, String, String, String)> = registry
        .iter()
        .flat_map(|r| r.search(""))
        .filter(|k| k.name != "material.output" && k.name != "material.custom")
        .filter_map(|k| {
            let op = k.name.trim_start_matches("material.");
            let title = style::op_title(op);
            let score = if query.trim().is_empty() {
                0
            } else {
                super::chrome::score(query, &title)
                    .or_else(|| super::chrome::score(query, &k.name))?
            };
            let category = style::category(op).0;
            let order = style::CATEGORIES
                .iter()
                .position(|cat| *cat == category)
                .unwrap_or(usize::MAX);
            Some((order, -score, title, k.name.clone(), category.to_owned()))
        })
        .collect();
    if query.trim().is_empty() {
        found.sort_by(|a, b| (a.0, &a.2).cmp(&(b.0, &b.2)));
    } else {
        found.sort_by(|a, b| (a.1, &a.2).cmp(&(b.1, &b.2)));
    }
    let mut group = String::new();
    for (_, _, title, kind, category) in found.into_iter().take(80) {
        if query.trim().is_empty() && category != group {
            group = category.clone();
            list = list.child(
                row().h(22.0).pad_x(8.0).align(Align::End).child(
                    text(category)
                        .text_size(10.5)
                        .weight(600)
                        .color(c.muted_foreground.0),
                ),
            );
        }
        let (doc, hover) = (doc.to_owned(), c.hover_surface.0);
        list = list.child(
            row()
                .key(format!("material/add/{kind}"))
                .h(26.0)
                .pad_x(8.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(title.clone())
                .hovered(move |e| e.fill(hover))
                .on_press(move |s: &mut Studio| add_node(s, &doc, &kind))
                .child(text(title).text_size(12.5).color(c.foreground.0)),
        );
    }
    super::style::float(l, col())
        .key("material/add/panel")
        .w(300.0)
        .radius(l.radius.sm + 2.0)
        .child(
            row().pad(6.0).child(
                field(
                    l,
                    Size::Sm,
                    "material/add/search",
                    Some(icons::SEARCH),
                    "Search nodes",
                    query,
                    |f| f.on_input(|s: &mut Studio, t| s.graph.adding = Some(t.to_owned())),
                )
                .grow(1.0),
            ),
        )
        .child(list)
}

/// Adds a `kind` node at the centre of the view (one undo step).
fn add_node(s: &mut Studio, doc: &str, kind: &str) {
    s.graph.adding = None;
    let rect = view_rect(s);
    let position = s.graph.shown.as_ref().map_or([240.0, 160.0], |g| {
        g.canvas.world([rect.w * 0.45, rect.h * 0.4], rect)
    });
    let command =
        json!({"cmd": "editor.material.node.add", "args": {"kind": kind, "position": position}});
    if let Err(error) = s.editor.apply_to(doc, vec![command], "Add material node") {
        s.editor.report_failure(&error);
    }
}

/// The nodes on the open canvas (tests read the page without a window).
#[cfg(test)]
pub(super) fn node_ids(s: &Studio) -> Vec<String> {
    s.graph
        .shown
        .as_ref()
        .map_or_else(Vec::new, |g| g.canvas.node_ids())
}
