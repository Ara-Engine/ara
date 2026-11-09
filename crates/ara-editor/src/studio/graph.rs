//! The Material tool: the material graph on the shared node canvas (`crate::graph::Canvas`,
//! the legacy window's), drawn into its own native layer (`GRAPH_LAYER`) like the Scene.
//! Drag nodes, drag from a port to connect, drag empty space to box-select, middle or right
//! drag to pan, the wheel to zoom; F frames the graph; Add Node searches the material
//! library. Every edit is a command on the document (one undo step per gesture), the same
//! ones agents send.
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

/// The graph canvas's native layer.
pub(crate) const GRAPH_LAYER: &str = "graph";

/// The graph page's state: the canvas for the open document and the text it shapes with.
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
    doc: String,
    revision: u64,
    canvas: Canvas,
    /// Opened and not yet framed to its laid-out size.
    fresh: bool,
}

/// The material document the page edits: the selected resource when it is one, else the
/// project's first material.
fn document(s: &Studio, domain: &str) -> Option<String> {
    let workspace = s.editor.host.store().workspace();
    let matches = |doc: &ara_doc::Document| {
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

/// Loads the page's document into the canvas if it changed (another document, or a newer
/// revision of it); the same document refreshes in place, keeping its view and selection.
pub(super) fn sync(s: &mut Studio) {
    let Some(doc) = document(s, "material") else {
        s.graph.shown = None;
        return;
    };
    let revision = s.editor.host.store().revision();
    if s.graph
        .shown
        .as_ref()
        .is_some_and(|g| g.doc == doc && g.revision == revision)
    {
        return;
    }
    let Some(document) = s.editor.host.store().workspace().doc(&doc) else {
        return;
    };
    let graph = match ara_graph::Graph::from_document(document) {
        Ok(graph) => graph,
        Err(error) => {
            s.editor.report_failure(&*anyhow::anyhow!("{error}"));
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
    match page.shown.as_mut().filter(|g| g.doc == doc) {
        Some(shown) => {
            shown.canvas.refresh(graph, text);
            shown.revision = revision;
        }
        None => {
            page.shown = Some(Shown {
                doc,
                revision,
                canvas: Canvas::new(graph, text),
                fresh: true,
            })
        }
    }
    page.dirty = true;
}

/// Draws the canvas into the page's layer when it changed. `extent` is in physical pixels.
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
    Theme {
        background: color(c.background.0),
        card: color(c.card.0),
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
        shadow: Color(0, 0, 0, if l.dark { 90 } else { 22 }),
    }
}

/// The Material page: its toolbar and the canvas, or a way to start one.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let Some(doc) = document(s, "material") else {
        return col().grow(1.0).justify(Justify::Center).child(empty(
            l,
            icons::PALETTE,
            "No material yet",
            "Materials are node graphs: start one, then add nodes and connect them.",
            Some(
                button(
                    l,
                    Tone::Primary,
                    Size::Sm,
                    Some(icons::PLUS),
                    "New material",
                )
                .key("material/new")
                .on_press(|s: &mut Studio| create(s, "material")),
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
    if let Some(query) = &s.graph.adding {
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
        .child(add)
        .child(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::FOCUS,
                "Frame the graph (F)",
            )
            .key("material/frame")
            .on_press(|s: &mut Studio| frame(s)),
        );
    let canvas = native(GRAPH_LAYER)
        .key("graph/view")
        .grow(1.0)
        .w_full()
        .focusable()
        .label("Material graph")
        .on_pointer(pointer)
        .on_wheel(|s: &mut Studio, w, _| {
            let size = s
                .local
                .rects
                .get("graph/view")
                .map_or((800.0, 600.0), |r| (r.width(), r.height()));
            let rect = Box2 {
                x: 0.0,
                y: 0.0,
                w: size.0,
                h: size.1,
            };
            let (left, top) = origin(s);
            if let Some(shown) = s.graph.shown.as_mut() {
                shown
                    .canvas
                    .zoom([w.at.x - left, w.at.y - top], w.delta, rect);
                s.graph.dirty = true;
            }
            true
        })
        .on_key(|s: &mut Studio, key, _| match key.0 {
            0x46 => {
                frame(s);
                Handled::Yes
            }
            0x20 => {
                s.graph.adding = Some(String::new());
                s.local.focus = Some("material/add/search");
                Handled::Yes
            }
            _ => Handled::No,
        });
    col().grow(1.0).child(toolbar).child(canvas)
}

/// Where the canvas is in the window (design px), from the last layout.
fn origin(s: &Studio) -> (f32, f32) {
    s.local
        .rects
        .get("graph/view")
        .map_or((0.0, 0.0), |r| (r.left, r.top))
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
            shown
                .canvas
                .begin(p, button, e.mods.shift || e.mods.meta || e.mods.ctrl, rect);
            s.graph.dragging = true;
        }
        PointerPhase::Move if s.graph.dragging => shown.canvas.motion(p),
        PointerPhase::Up if s.graph.dragging => {
            s.graph.dragging = false;
            match shown.canvas.end(p, rect) {
                Ok(edits) => {
                    let doc = shown.doc.clone();
                    submit(s, &doc, edits);
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

/// Fits the whole graph in view.
fn frame(s: &mut Studio) {
    let size = s
        .local
        .rects
        .get("graph/view")
        .map_or((800.0, 600.0), |r| (r.width(), r.height()));
    let rect = Box2 {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    };
    if let Some(shown) = s.graph.shown.as_mut() {
        shown.canvas.camera = shown.canvas.framed_readable(rect);
        s.graph.dirty = true;
    }
}

/// Starts a new `domain` graph in the open Assets folder and opens it.
fn create(s: &mut Studio, domain: &str) {
    let folder = s.editor.state.asset_folder.clone();
    let first = s.editor.host.store().revision();
    let workspace = s.editor.host.store().workspace();
    let path = (first..first.saturating_add(10_000))
        .map(|n| {
            let leaf = format!("{domain}-{n}.graph");
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
        json!({"path": path, "domain": domain}),
    ) {
        Ok(result) => s.act(
            "editor.workspace",
            json!({"resource": result["doc"], "asset": ""}),
        ),
        Err(error) => s.editor.report_failure(&error),
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
    let size = s
        .local
        .rects
        .get("graph/view")
        .map_or((800.0, 600.0), |r| (r.width(), r.height()));
    let rect = Box2 {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    };
    let position = s.graph.shown.as_ref().map_or([240.0, 160.0], |g| {
        g.canvas.world([size.0 * 0.45, size.1 * 0.4], rect)
    });
    let command =
        json!({"cmd": "editor.material.node.add", "args": {"kind": kind, "position": position}});
    if let Err(error) = s.editor.apply_to(doc, vec![command], "Add material node") {
        s.editor.report_failure(&error);
    }
}
