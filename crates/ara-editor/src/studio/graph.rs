//! The Material and Animation tools: a material graph, or an animation controller's state
//! machine, on the shared node canvas (`crate::graph::Canvas`, the legacy window's), drawn
//! into the page's own native layer ([`PAGES`]) like the Scene. Drag nodes, drag from a port
//! to connect, drag empty space to box-select, middle or right drag to pan, the wheel to
//! zoom; F frames the graph; Space, Add Node, a right-click or a wire dropped on nothing
//! searches the material library (only what fits the wire). Inputs show their defaults
//! inline, typed where they show; C wraps the selection in a comment box; ⌘D duplicates it;
//! Delete removes it. Every edit is a command on the document (one undo step per gesture),
//! the same ones agents send.

use ara_kit::icons;
use ara_kit::ui::*;
use ara_ui::paint::{Box2, DisplayList};
use ara_ui::style::Color;
use serde_json::{Value, json};
use stillkit::el::{
    Align, Axis, El, Handled, Justify, PointerPhase, Side, col, el, icon, native, row, text,
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
    /// The Add Node search, while it is open.
    pub adding: Option<Adding>,
    /// An input's inline default being typed, while its field is open.
    pub editing: Option<Editing>,
    /// The Material page shows its source (the material as DSL text) instead of the graph:
    /// the text being edited, unapplied.
    pub source: Option<String>,
    /// The Material page's in-node previews ([`previews`]).
    previews: Previews,
}

/// In-node previews (Shader Graph style): each operation node's value rendered on its own
/// worker (`asset_tools::node_preview`), uploaded into the page layer's image slots as
/// renders land. Until a node's render lands its tile says so; one that fails keeps a
/// compact body.
#[derive(Default)]
struct Previews {
    /// Requests for the shown material and their failures.
    nodes: crate::asset_tools::node_preview::NodePreviews,
    /// The render worker, started with the first preview.
    queue: Option<crate::asset_tools::thumbnails::Thumbnails>,
    /// Each rendered preview key's image slot.
    slots: std::collections::HashMap<String, usize>,
    /// The next slot to fill (they cycle).
    next: usize,
    /// Images landed since the layer last drew, by image id.
    uploads: Vec<(u32, ara_ui_render::gpu::Image)>,
    /// The last render failure reported, so a cause shared by every node is said once.
    reported: Option<String>,
}

/// Image slots for in-node previews: twice the most a graph derives, so a revision's new
/// renders don't evict the ones still shown.
const PREVIEW_SLOTS: usize = 2 * crate::asset_tools::node_preview::MAX;

impl Page {
    /// The graph the canvas shows (its last loaded revision).
    pub(super) fn shown_graph(&self) -> Option<&ara_graph::Graph> {
        self.shown.as_ref().map(|g| g.canvas.graph())
    }
    /// What is picked on the canvas: nodes, wires (transition arrows), comments.
    pub(super) fn selection(&self) -> std::collections::BTreeSet<String> {
        self.shown
            .as_ref()
            .map(|g| g.canvas.selection.clone())
            .unwrap_or_default()
    }
    /// How many nodes show a rendered in-node preview (tests).
    #[cfg(test)]
    pub(super) fn previewed(&self) -> usize {
        self.shown.as_ref().map_or(0, |g| g.canvas.previews.len())
    }
    /// Why the in-node previews couldn't render, once the worker said (tests).
    #[cfg(test)]
    pub(super) fn preview_error(&self) -> Option<&str> {
        self.previews.reported.as_deref()
    }
    /// Picks `id` alone on the canvas (a transition from the Details' list).
    pub(super) fn pick(&mut self, id: String) {
        if let Some(shown) = self.shown.as_mut() {
            shown.canvas.selection = std::iter::once(id).collect();
            self.dirty = true;
        }
    }
}

/// An inline input default edited in place: its constant node, the text it started from,
/// and the value box's window rectangle (left, top, width).
#[derive(Clone)]
pub struct Editing {
    node: String,
    text: String,
    at: (f32, f32, f32),
}

/// The Add Node search: what is typed, where the node goes (graph space; the view's centre
/// when unset), where the search shows (window point; under its button when unset), and the
/// pin a dropped wire came from, which the new node connects to.
#[derive(Clone, Default)]
pub struct Adding {
    pub query: String,
    at: Option<[f32; 2]>,
    point: Option<(f32, f32)>,
    from: Option<(ara_graph::Endpoint, ara_graph::Direction)>,
}

/// The document a canvas shows, at which store revision.
struct Shown {
    domain: &'static str,
    doc: String,
    revision: u64,
    canvas: Canvas,
    /// Opened and not yet framed to its laid-out size.
    fresh: bool,
    /// A material's stats at this revision (`None` for a state machine).
    stats: Option<super::material_stats::Summary>,
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
        match crate::text::text_system() {
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
            shown.stats = (domain == "material").then(|| super::material_stats::summarize(&graph));
            shown.canvas.refresh(graph, text);
            shown.revision = revision;
        }
        None => {
            page.shown = Some(Shown {
                domain,
                doc,
                revision,
                stats: (domain == "material").then(|| super::material_stats::summarize(&graph)),
                canvas: Canvas::new(graph, text),
                fresh: true,
            })
        }
    }
    page.dirty = true;
}

/// Keeps the Material page's in-node previews current: derives them when the material
/// changes, asks the worker for those not rendered, and takes the renders that landed.
/// Whether the canvas changed (it needs drawing).
pub(super) fn previews(s: &mut Studio) -> bool {
    use crate::asset_tools::{node_preview, thumbnails};
    let root = s.editor.host.root().to_owned();
    let revision = s.editor.host.store().revision();
    let page = &mut s.graph;
    let (Some(shown), Some(text)) = (page.shown.as_mut(), page.text.as_mut()) else {
        return false;
    };
    if shown.domain != "material" {
        return false;
    }
    let p = &mut page.previews;
    let pixels = (2.0 * crate::graph::PREVIEW) as u32;
    let identity = (shown.doc.clone(), revision);
    if p.nodes.revision.as_ref() != Some(&identity) {
        p.nodes.revision = Some(identity);
        p.nodes.requests.clear();
        if let Ok(material) = ara_material::Material::from_graph(shown.canvas.graph()) {
            let nodes: Vec<String> = shown
                .canvas
                .preview_nodes()
                .take(node_preview::MAX)
                .map(str::to_owned)
                .collect();
            for node in nodes {
                if let Some((derived, shot)) = node_preview::derive(&material, &node) {
                    let key = thumbnails::Thumbnails::material_key(&derived, shot, pixels);
                    p.nodes.requests.insert(node, (key, derived, shot));
                }
            }
        }
    }
    let mut changed = false;
    let mut failures = Vec::new();
    // Renders that landed take a slot (the oldest is reused) and upload on the next draw.
    if let Some(queue) = p.queue.as_mut() {
        for done in queue.poll() {
            match done {
                thumbnails::Done::Image(thumb) => {
                    let slot = match p.slots.get(&thumb.key) {
                        Some(slot) => *slot,
                        None => {
                            let slot = p.next % PREVIEW_SLOTS;
                            p.next += 1;
                            p.slots.retain(|_, s| *s != slot);
                            p.slots.insert(thumb.key.clone(), slot);
                            slot
                        }
                    };
                    p.uploads.push((
                        thumbnails::image_id(slot),
                        ara_ui_render::gpu::Image {
                            width: thumb.image.width(),
                            height: thumb.image.height(),
                            rgba: thumb.image.into_raw(),
                        },
                    ));
                    changed = true;
                }
                thumbnails::Done::Failed(key, _, error) => {
                    // Said once per cause (a missing toolchain fails them all alike).
                    if p.reported.as_deref() != Some(error.as_str()) {
                        failures.push(format!("In-node previews can't render: {error}"));
                        p.reported = Some(error);
                    }
                    p.nodes.failed.insert(key);
                    changed = true;
                }
                thumbnails::Done::Pending(_) => {}
            }
        }
    }
    // Tiles for nodes with a render shown or on its way; the rest keep a compact body.
    let previewed = p
        .nodes
        .requests
        .iter()
        .filter(|(_, (key, ..))| !p.nodes.failed.contains(key))
        .map(|(node, _)| node.clone())
        .collect();
    shown.canvas.set_previewed(previewed, text);
    let path = s
        .editor
        .host
        .store()
        .workspace()
        .path_of(&shown.doc)
        .unwrap_or("material")
        .to_owned();
    for (node, (key, derived, shot)) in &p.nodes.requests {
        if p.nodes.failed.contains(key) {
            continue;
        }
        match p.slots.get(key) {
            Some(slot) => {
                let id = thumbnails::image_id(*slot);
                if shown.canvas.previews.insert(node.clone(), id) != Some(id) {
                    changed = true;
                }
            }
            None => p
                .queue
                .get_or_insert_with(|| thumbnails::Thumbnails::new(&root))
                .request_material(&path, key, derived, *shot, pixels),
        }
    }
    if changed {
        page.dirty = true;
    }
    for failure in failures {
        s.editor.report_failure(&*anyhow::anyhow!(failure));
    }
    changed
}

/// Draws the canvas into the page's layer. `extent` is in physical pixels.
/// # Safety
/// `layer` is the graph page's live native layer (`present::Target`).
pub(super) unsafe fn draw(
    s: &mut Studio,
    target: &mut super::layer::Layer,
    renderer: &ara_render::Renderer,
    layer: super::present::Target,
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
    for (id, image) in page.previews.uploads.drain(..) {
        target.image(id, image)?;
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
                None => Some(Adding::default()),
            };
            s.local.focus = Some("material/add/search");
        });
    // Under the button, or at the pointer (right-click, a dropped wire) from an element of
    // no size: a press on the element an overlay hangs from never dismisses it.
    let mut at_point = el().key("graph/search-anchor").absolute(0.0, 0.0).size(0.0);
    if let Some(adding) = s.graph.adding.as_ref().filter(|_| !animation) {
        let search = stillkit::el::popover(palette(s, &doc, adding))
            .on_dismiss(|s: &mut Studio| s.graph.adding = None);
        match adding.point {
            Some((x, y)) => at_point = at_point.overlay(search.at(x, y)),
            None => {
                add = add.overlay(search.side(Side::Bottom).align(Align::Start).offset(4.0));
            }
        }
    }
    // A clicked inline value opens its field over the box, from an element of no size.
    let mut at_value = el().key("graph/value-anchor").absolute(0.0, 0.0).size(0.0);
    if let Some(editing) = s.graph.editing.as_ref() {
        let (doc, node) = (doc.clone(), editing.node.clone());
        let input = field(
            l,
            Size::Sm,
            "graph/value",
            None,
            "",
            &editing.text,
            move |x| {
                x.select_all_on_focus().on_submit(move |s: &mut Studio, t| {
                    s.graph.editing = None;
                    set_value(s, &doc, &node, t);
                })
            },
        )
        .w(editing.at.2.max(96.0));
        at_value = at_value.overlay(
            stillkit::el::popover(input)
                .at(editing.at.0 - 4.0, editing.at.1 - 6.0)
                .on_dismiss(|s: &mut Studio| s.graph.editing = None),
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
        .children((!animation).then(|| {
            // Graph or Source: the same document, as nodes or as text.
            let on = usize::from(s.graph.source.is_some());
            let doc = doc.clone();
            segmented(
                l,
                &[
                    ("material/view/graph", Some(icons::WORKFLOW), "Graph"),
                    ("material/view/source", Some(icons::FILE_CODE), "Source"),
                ],
                on,
                move |i| {
                    let doc = doc.clone();
                    move |s: &mut Studio| {
                        s.graph.source = (i == 1).then(|| source_text(s, &doc).unwrap_or_default());
                    }
                },
            )
        }))
        .children((!animation && s.graph.source.is_none()).then_some(add))
        .children(animation.then(|| {
            let edit = |kind: &'static str, label: &str, glyph| {
                let doc = doc.clone();
                button(l, Tone::Ghost, Size::Sm, Some(glyph), label)
                    .key(format!("animation/add/{kind}"))
                    .on_press(move |s: &mut Studio| {
                        if let Err(error) = crate::animation_edit::edit(&mut s.editor, &doc, kind) {
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
        .on_key(move |s: &mut Studio, key, tx| match key.0 {
            0x46 => {
                frame(s);
                Handled::Yes
            }
            0x20 if !animation => {
                s.graph.adding = Some(Adding::default());
                s.local.focus = Some("material/add/search");
                Handled::Yes
            }
            // Delete and Backspace remove the selected nodes (one undo step).
            0x2E | 0x08 if !animation => {
                delete_selection(s);
                Handled::Yes
            }
            // C wraps the selection in a comment box, as Unreal does.
            0x43 if !animation && !command(tx) => {
                comment_selection(s);
                Handled::Yes
            }
            // ⌘D (Ctrl+D) duplicates the selected nodes.
            0x44 if !animation && command(tx) => {
                duplicate_selection(s);
                Handled::Yes
            }
            _ => Handled::No,
        });
    if let (false, Some(text)) = (animation, s.graph.source.as_ref()) {
        return col()
            .grow(1.0)
            .child(toolbar)
            .child(source_view(s, &doc, text));
    }
    let body = row().grow(1.0).min_h(0.0).child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .child(canvas)
            .children(
                s.graph
                    .shown
                    .as_ref()
                    .and_then(|g| g.stats.as_ref())
                    .filter(|_| !animation)
                    .map(|stats| super::material_stats::strip(s, stats)),
            )
            .child(at_point)
            .child(at_value)
            .children(empty_hint(s, animation)),
    );
    let body = if animation {
        body.child(super::animator::details(s, &doc))
    } else {
        body.child(details(s, &doc))
    };
    col().grow(1.0).child(toolbar).child(body)
}

/// Material `doc` as DSL text (what the Source view edits), if it reads as a material.
fn source_text(s: &Studio, doc: &str) -> Option<String> {
    let graph =
        ara_graph::Graph::from_document(s.editor.host.store().workspace().doc(doc)?).ok()?;
    let material = ara_material::Material::from_graph(&graph).ok()?;
    ara_material::dsl::print(&material).ok()
}

/// The Source view: the material as text, edited in place; Apply (⌘/Ctrl+Enter) writes it
/// back as one undo step (the graph follows), Revert takes the document's text again.
fn source_view(s: &Studio, doc: &str, draft: &str) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let changed = source_text(s, doc).as_deref() != Some(draft);
    let apply = {
        let doc = doc.to_owned();
        move |s: &mut Studio, text: &str| {
            let command = json!({"cmd": "material.text.apply", "args": {"text": text}});
            match s
                .editor
                .apply_to(&doc, vec![command], "Edit material source")
            {
                Ok(_) => super::note(s, "Material source applied"),
                Err(error) => s.editor.report_failure(&error),
            }
        }
    };
    let (typed, revert_doc) = (draft.to_owned(), doc.to_owned());
    let on_apply = apply.clone();
    col()
        .grow(1.0)
        .min_h(0.0)
        .pad(8.0)
        .gap(6.0)
        .child(
            stillkit::el::text_area("material/source")
                .value(draft.to_owned())
                .family(mono())
                .text_size(12.5)
                .grow(1.0)
                .w_full()
                .pad(10.0)
                .radius(super::style::RADIUS)
                .fill(super::style::tones(l).well)
                .on_input(|s: &mut Studio, t| s.graph.source = Some(t.to_owned()))
                .on_submit(move |s: &mut Studio, t| apply(s, t)),
        )
        .child(
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(
                    text(if changed {
                        "Edited: Apply writes it back (⌘/Ctrl+Enter)"
                    } else {
                        "The material as text: the same document as the graph"
                    })
                    .text_size(11.5)
                    .color(c.muted_foreground.0)
                    .grow(1.0),
                )
                .child(enabled(
                    button(l, Tone::Ghost, Size::Sm, None, "Revert")
                        .key("material/source/revert")
                        .on_press(move |s: &mut Studio| {
                            s.graph.source = Some(source_text(s, &revert_doc).unwrap_or_default())
                        }),
                    changed,
                ))
                .child(enabled(
                    button(l, Tone::Primary, Size::Sm, Some(icons::CHECK), "Apply")
                        .key("material/source/apply")
                        .on_press(move |s: &mut Studio| on_apply(s, &typed)),
                    changed,
                )),
        )
}

/// The parameter types the Parameters band's + offers (Shader Graph's Blackboard), as
/// (title, material type).
const PARAMETER_TYPES: [(&str, &str); 8] = [
    ("Float", "float"),
    ("Vector 2", "vec2"),
    ("Vector 3", "vec3"),
    ("Vector 4", "vec4"),
    ("Color", "ColorLinear"),
    ("Boolean", "bool"),
    ("Texture", "TextureColor"),
    ("Normal map", "TextureNormal"),
];

/// The Parameters band's add button: a menu of types; a pick adds a parameter of it at
/// the view's left, picked.
fn add_parameter_menu(s: &Studio, doc: &str) -> El<Studio> {
    const KEY: &str = "material/param/add";
    let l = &s.look;
    let open = s.local.select.as_deref() == Some(KEY);
    let mut add = icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, "Add a parameter")
        .size(22.0)
        .key(KEY)
        .expanded(open)
        .on_press(|s: &mut Studio| {
            s.local.select = match s.local.select.as_deref() == Some(KEY) {
                true => None,
                false => Some(KEY.into()),
            }
        });
    if open {
        let items = PARAMETER_TYPES
            .iter()
            .map(|(title, ty)| {
                let (doc, ty) = (doc.to_owned(), *ty);
                menu_item(l, &format!("{KEY}/{ty}"), None, title, None, false).on_press(
                    move |s: &mut Studio| {
                        s.local.select = None;
                        add_parameter(s, &doc, ty);
                    },
                )
            })
            .collect();
        add = add.overlay(
            stillkit::el::menu(menu_panel(l, items))
                .side(Side::Bottom)
                .align(Align::End)
                .on_dismiss(|s: &mut Studio| s.local.select = None),
        );
    }
    add
}

/// Adds a parameter of material type `ty` at the view's left and picks it.
fn add_parameter(s: &mut Studio, doc: &str, ty: &str) {
    let rect = view_rect(s);
    let position = s.graph.shown.as_ref().map_or([0.0, 0.0], |g| {
        g.canvas.world([rect.w * 0.15, rect.h * 0.3], rect)
    });
    let command = json!({"cmd": "editor.material.node.add", "args": {"kind": "material.parameter", "position": position, "ty": ty}});
    match s
        .editor
        .apply_to(doc, vec![command], "Add material parameter")
    {
        Ok(result) => {
            let id = result["outputs"][0]["id"].as_str().map(str::to_owned);
            sync(s, "material");
            if let (Some(id), Some(shown)) = (id, s.graph.shown.as_mut()) {
                shown.canvas.selection = std::iter::once(id).collect();
                s.graph.dirty = true;
            }
        }
        Err(error) => s.editor.report_failure(&error),
    }
}

/// Over an empty state machine, how to start one (Unity's Animator opens on Entry).
fn empty_hint(s: &Studio, animation: bool) -> Option<El<Studio>> {
    let empty = s.graph.shown_graph().is_none_or(|g| g.nodes.is_empty());
    (animation && empty).then(|| {
        let c = &s.look.c;
        row()
            .absolute(20.0, 16.0)
            .pad_x(12.0)
            .pad_y(8.0)
            .gap(8.0)
            .align(Align::Center)
            .radius(super::style::RADIUS)
            .fill(super::style::tones(&s.look).band)
            .child(icon(icons::INFO).size(13.0).color(c.muted_foreground.0))
            .child(
                text(
                    "No states yet. Add State plays a clip; Add Blend mixes clips by a parameter.",
                )
                .text_size(12.0)
                .color(c.muted_foreground.0),
            )
    })
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
    // A picked comment box: its title, typed here, and its removal (its nodes stay).
    if let Some((key, group)) = graph
        .editor
        .groups
        .iter()
        .find(|(key, _)| selection.contains(*key))
    {
        let (doc_label, doc_remove) = (doc.to_owned(), doc.to_owned());
        let (key_label, key_remove) = (key.clone(), key.clone());
        return out
            .child(band(
                "Comment".into(),
                format!("{} nodes", group.nodes.len()),
                icons::SQUARE_DASHED,
            ))
            .child(prop(
                "Title".into(),
                field(
                    l,
                    Size::Sm,
                    "material/comment/title",
                    None,
                    "",
                    &group.label,
                    move |x| {
                        x.select_all_on_focus().on_submit(move |s: &mut Studio, t| {
                            edit_layout(s, &doc_label, |layout| {
                                if let Some(group) = layout.groups.get_mut(&key_label) {
                                    group.label = t.trim().to_owned();
                                }
                            });
                        })
                    },
                ),
            ))
            .child(
                row().pad(10.0).child(
                    button(
                        l,
                        Tone::Ghost,
                        Size::Sm,
                        Some(icons::TRASH),
                        "Remove comment",
                    )
                    .key("material/comment/remove")
                    .on_press(move |s: &mut Studio| {
                        edit_layout(s, &doc_remove, |layout| {
                            layout.groups.remove(&key_remove);
                        });
                    }),
                ),
            );
    }
    if selection.is_empty() {
        out = out.child(
            band(
                "Parameters".into(),
                String::new(),
                icons::SLIDERS_HORIZONTAL,
            )
            .child(add_parameter_menu(s, doc)),
        );
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
        let renamed = name.clone();
        out = out.child(band(name, kind, icons::CONTRAST));
        if op != "output" {
            let doc = doc.to_owned();
            out = out.child(prop(
                "Name".into(),
                field(l, Size::Sm, format!("material/name/{id}"), None, "", &renamed.clone(), move |x| {
                    x.select_all_on_focus().on_submit(move |s: &mut Studio, t| {
                        let to = t.trim();
                        if to.is_empty() || to == renamed {
                            return;
                        }
                        let command = json!({"cmd": "material.rename", "args": {"from": renamed, "to": to}});
                        if let Err(error) = s.editor.apply_to(&doc, vec![command], "Rename material value") {
                            s.editor.report_failure(&error);
                        }
                    })
                }),
            ));
        }
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
                if op == "parameter" {
                    "Default"
                } else {
                    "Value"
                }
                .into(),
                field(
                    l,
                    Size::Sm,
                    format!("material/value/{id}"),
                    None,
                    "",
                    &shown,
                    move |x| {
                        x.select_all_on_focus()
                            .on_submit(move |s: &mut Studio, t| set_value(s, &doc, &node_id, t))
                    },
                ),
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
            let pin = style::pin_label(node, port, socket.direction, &socket.ty);
            // A default drawn on the pin is typed here too, as Unreal's Details do.
            if let Some((default, value)) = shown.canvas.inline(id, port) {
                let (doc, node_id) = (doc.to_owned(), default.to_owned());
                out = out.child(prop(
                    pin,
                    field(
                        l,
                        Size::Sm,
                        format!("material/input/{id}/{port}"),
                        None,
                        "",
                        value,
                        move |x| {
                            x.select_all_on_focus()
                                .on_submit(move |s: &mut Studio, t| set_value(s, &doc, &node_id, t))
                        },
                    ),
                ));
                continue;
            }
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
        // A double-click on a wire adds a reroute point there (Unreal's), one undo step.
        PointerPhase::Down
            if e.clicks == 2
                && e.button == stillkit::Button::Left
                && shown.canvas.node_at(p, rect).is_none() =>
        {
            let Some((wire, segment)) = shown.canvas.wire_at(p, rect) else {
                return false;
            };
            let (doc, at) = (shown.doc.clone(), shown.canvas.world(p, rect));
            edit_layout(s, &doc, |layout| {
                let points = layout.reroutes.entry(wire).or_default();
                points.insert(segment.min(points.len()), at);
            });
            return true;
        }
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
            let still = e.travel().x.abs() + e.travel().y.abs() < 4.0;
            // A wire dropped where no pin is: search for the node to feed (or be fed by).
            let dropped = shown
                .canvas
                .connecting()
                .filter(|_| !shown.canvas.port_at(p, rect));
            let world = shown.canvas.world(p, rect);
            let material = shown.domain == "material";
            let right = e.button != stillkit::Button::Left;
            if material && (dropped.is_some() || (right && still)) {
                shown.canvas.cancel();
                s.graph.adding = Some(Adding {
                    query: String::new(),
                    at: Some(world),
                    point: Some((e.at.x, e.at.y)),
                    from: dropped,
                });
                s.local.focus = Some("material/add/search");
                s.graph.dirty = true;
                return true;
            }
            let value = (still && !right)
                .then(|| shown.canvas.inline_at(p, rect))
                .flatten()
                .map(|(node, text, b)| Editing {
                    node: node.to_owned(),
                    text: text.to_owned(),
                    at: (e.at.x - p[0] + b.x, e.at.y - p[1] + b.y, b.w),
                });
            if value.is_some() {
                s.graph.editing = value;
                s.local.focus = Some("graph/value");
            }
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
            Edit::Connect(from, to) => {
                if let Some(shown) = s.graph.shown.as_ref() {
                    commands.extend(connect(
                        shown.canvas.graph(),
                        &from.node,
                        &to.node,
                        &to.port,
                    ));
                }
            }
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
                crate::animation_edit::connect(&mut s.editor, doc, &graph, from, to)
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
pub(super) fn frame(s: &mut Studio) {
    let rect = view_rect(s);
    if let Some(shown) = s.graph.shown.as_mut() {
        shown.canvas.camera = shown.canvas.framed_readable(rect);
        s.graph.dirty = true;
    }
}

/// Add Node: the material library, searched, by category; a pick lands at the view's centre.
fn palette(s: &Studio, doc: &str, adding: &Adding) -> El<Studio> {
    use crate::graph::style;
    let l = &s.look;
    let c = &l.c;
    let query = adding.query.as_str();
    let wire = wire_type(s, adding);
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
        .filter(|k| {
            wire.is_none_or(|(ty, direction)| {
                fits(k.name.trim_start_matches("material."), ty, direction)
            })
        })
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
                    |f| {
                        f.on_input(|s: &mut Studio, t| {
                            if let Some(adding) = s.graph.adding.as_mut() {
                                adding.query = t.to_owned();
                            }
                        })
                    },
                )
                .grow(1.0),
            ),
        )
        .children(wire.map(|(ty, direction)| {
            let what = if direction == ara_graph::Direction::Output {
                "take"
            } else {
                "give"
            };
            row().h(22.0).pad_x(14.0).align(Align::Center).child(
                text(format!(
                    "Nodes that {what} {}",
                    crate::graph::style::short_type(ty.name())
                ))
                .text_size(11.0)
                .color(c.muted_foreground.0),
            )
        }))
        .child(list)
}

/// The type of the wire a search was opened from, and which end it came from.
fn wire_type(s: &Studio, adding: &Adding) -> Option<(ara_material::Type, ara_graph::Direction)> {
    let (pin, direction) = adding.from.as_ref()?;
    let graph = s.graph.shown.as_ref()?.canvas.graph();
    let ty = &graph.nodes.get(&pin.node)?.ports.get(&pin.port)?.ty;
    let ty = ara_material::Type::ALL
        .iter()
        .copied()
        .find(|t| t.name() == ty)?;
    Some((ty, *direction))
}

/// Whether operation `op` connects to a wire of type `ty` dragged from an `end` (an output
/// feeds one of its inputs; an input takes its output), trying the usual shapes: the other
/// inputs of the same type, or scalars. Constants and parameters are made to fit.
fn fits(op: &str, ty: ara_material::Type, end: ara_graph::Direction) -> bool {
    use ara_material::{Type, library::infer};
    if matches!(op, "literal" | "parameter") {
        return !ty.is_texture() || end == ara_graph::Direction::Input;
    }
    let from_output = end == ara_graph::Direction::Output;
    (usize::from(from_output)..=4).any(|arity| {
        (0..1u32 << arity).any(|mask| {
            let types: Vec<Type> = (0..arity)
                .map(|i| if mask & 1 << i == 0 { ty } else { Type::Float })
                .collect();
            let takes = !from_output || types.contains(&ty);
            takes && infer(op, &types).is_some_and(|out| from_output || out == ty)
        })
    })
}

/// Adds a `kind` node where the search was opened (else the view's centre); dropped from a
/// wire, it is typed for the wire and connects to that pin: its output into the pin, or the
/// pin into its first input of the wire's type.
fn add_node(s: &mut Studio, doc: &str, kind: &str) {
    let adding = s.graph.adding.take().unwrap_or_default();
    let rect = view_rect(s);
    let position = adding.at.unwrap_or_else(|| {
        s.graph.shown.as_ref().map_or([240.0, 160.0], |g| {
            g.canvas.world([rect.w * 0.45, rect.h * 0.4], rect)
        })
    });
    let wire = wire_type(s, &adding).map(|(ty, _)| ty.name());
    let command = json!({"cmd": "editor.material.node.add", "args": {"kind": kind, "position": position, "ty": wire}});
    let added = match s.editor.apply_to(doc, vec![command], "Add material node") {
        Ok(result) => result["outputs"][0]["id"].as_str().map(str::to_owned),
        Err(error) => {
            s.editor.report_failure(&error);
            return;
        }
    };
    let (Some(id), Some((pin, direction))) = (added, adding.from) else {
        return;
    };
    let Some(graph) = s
        .editor
        .host
        .store()
        .workspace()
        .doc(doc)
        .and_then(|d| ara_graph::Graph::from_document(d).ok())
    else {
        return;
    };
    let target = if direction == ara_graph::Direction::Input {
        // Dragged from an input: the new node's output feeds it.
        Some((id, pin.node, pin.port))
    } else {
        // Dragged from an output: it feeds the new node's first input of the wire's type.
        graph.nodes.get(&id).and_then(|n| {
            let inputs = || {
                n.ports
                    .iter()
                    .filter(|(_, p)| p.direction == ara_graph::Direction::Input)
            };
            inputs()
                .find(|(_, p)| Some(p.ty.as_str()) == wire)
                .or_else(|| inputs().next())
                .map(|(port, _)| (pin.node.clone(), id.clone(), port.clone()))
        })
    };
    let Some((source, node, port)) = target else {
        return;
    };
    let commands = connect(&graph, &source, &node, &port);
    if let Err(error) = s.editor.apply_to(doc, commands, "Connect material node") {
        s.editor.report_failure(&error);
    }
}

/// Connects `source` into `node`'s input `port`, removing the generated default it replaces
/// (a constant named `…Input<n>` feeding only that input), so none is left behind.
fn connect(graph: &ara_graph::Graph, source: &str, node: &str, port: &str) -> Vec<Value> {
    let replaced = graph
        .edges
        .values()
        .find(|e| e.to.node == node && e.to.port == port)
        .map(|e| e.from.node.as_str())
        .filter(|from| *from != source && crate::graph::is_default(graph, from));
    std::iter::once(
        json!({"cmd": "material.connect", "args": {"source": source, "node": node, "port": port}}),
    )
    .chain(replaced.map(|from| json!({"cmd": "graph.node.remove", "args": {"node": from}})))
    .collect()
}

/// Sets a constant or parameter's value from typed text: JSON, a number, or lanes typed as
/// `0.18, 0.42, 0.9` (one undo step).
fn set_value(s: &mut Studio, doc: &str, node: &str, typed: &str) {
    let Ok(mut value) = serde_json::from_str::<Value>(typed)
        .or_else(|_| serde_json::from_str(&format!("[{}]", typed.trim())))
    else {
        return;
    };
    if value.is_number() {
        value = json!([value]);
    }
    let command =
        json!({"cmd": "editor.material.value.set", "args": {"node": node, "value": value}});
    if let Err(error) = s
        .editor
        .apply_to(doc, vec![command], "Edit material node value")
    {
        s.editor.report_failure(&error);
    }
}

/// Adds a reroute point midway along the selected wire (or the first wire into a selected
/// node), as one undo step; whether there was one.
pub(super) fn reroute_selection(s: &mut Studio) -> bool {
    let Some(shown) = s.graph.shown.as_ref() else {
        return false;
    };
    let graph = shown.canvas.graph();
    let wire = graph
        .edges
        .iter()
        .find(|(id, _)| shown.canvas.selection.contains(*id))
        .or_else(|| {
            graph
                .edges
                .iter()
                .find(|(_, e)| shown.canvas.selection.contains(&e.to.node))
        });
    let Some((id, edge)) = wire else {
        return false;
    };
    let at = |node: &str| {
        graph
            .editor
            .positions
            .get(node)
            .copied()
            .unwrap_or([0.0, 0.0])
    };
    let (a, b) = (at(&edge.from.node), at(&edge.to.node));
    // From the source node's right side (a node is about 216 wide) to the target's left.
    let mid = [(a[0] + 216.0 + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let (doc, id) = (shown.doc.clone(), id.clone());
    edit_layout(s, &doc, |layout| {
        layout.reroutes.entry(id).or_default().push(mid)
    });
    true
}

/// Changes the graph's layout (comments, groups, reroutes) as one undo step.
fn edit_layout(s: &mut Studio, doc: &str, change: impl FnOnce(&mut ara_graph::Editor)) {
    let Some(shown) = s.graph.shown.as_ref() else {
        return;
    };
    let mut layout = shown.canvas.graph().editor.clone();
    change(&mut layout);
    let command = json!({"cmd": "graph.editor.set", "args": {"editor": layout}});
    if let Err(error) = s.editor.apply_to(doc, vec![command], "Edit graph comments") {
        s.editor.report_failure(&error);
    }
}

/// Whether the platform's command modifier (⌘ on macOS, Ctrl elsewhere) is held.
fn command(tx: &stillkit::el::Tx) -> bool {
    let m = tx.modifiers();
    if cfg!(target_os = "macos") {
        m.meta
    } else {
        m.ctrl
    }
}

/// Duplicates the selected nodes (one undo step) and picks the copies.
pub(super) fn duplicate_selection(s: &mut Studio) {
    let Some(shown) = s.graph.shown.as_ref() else {
        return;
    };
    let graph = shown.canvas.graph();
    let nodes: Vec<&String> = shown
        .canvas
        .selection
        .iter()
        .filter(|id| graph.nodes.contains_key(*id))
        .collect();
    if nodes.is_empty() {
        return;
    }
    let doc = shown.doc.clone();
    let command = json!({"cmd": "editor.material.node.duplicate", "args": {"nodes": nodes}});
    match s
        .editor
        .apply_to(&doc, vec![command], "Duplicate material nodes")
    {
        Ok(result) => {
            let copies: std::collections::BTreeSet<String> = result["outputs"][0]["ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect();
            sync(s, "material");
            if let Some(shown) = s.graph.shown.as_mut() {
                shown.canvas.selection = copies;
                s.graph.dirty = true;
            }
        }
        Err(error) => s.editor.report_failure(&error),
    }
}

/// Wraps the selected nodes in a comment box and picks it, its title ready to type.
pub(super) fn comment_selection(s: &mut Studio) {
    let Some(shown) = s.graph.shown.as_ref() else {
        return;
    };
    let graph = shown.canvas.graph();
    let nodes: Vec<String> = shown
        .canvas
        .selection
        .iter()
        .filter(|id| graph.nodes.contains_key(*id))
        .cloned()
        .collect();
    if nodes.is_empty() {
        return;
    }
    let doc = shown.doc.clone();
    let key = (1..)
        .map(|n| format!("comment{n}"))
        .find(|k| !graph.editor.groups.contains_key(k))
        .unwrap_or_default();
    let picked = key.clone();
    edit_layout(s, &doc, |layout| {
        layout.groups.insert(
            key,
            ara_graph::Group {
                label: "Comment".into(),
                nodes,
            },
        );
    });
    // Loaded now, so its title field is there when focus lands on it.
    sync(s, "material");
    if let Some(shown) = s.graph.shown.as_mut() {
        shown.canvas.selection.insert(picked);
        s.graph.dirty = true;
    }
    s.local.focus = Some("material/comment/title");
}

/// Removes the selected nodes and their inline defaults (one undo step); with a comment box
/// picked, only the box.
pub(super) fn delete_selection(s: &mut Studio) {
    let Some(shown) = s.graph.shown.as_ref() else {
        return;
    };
    let doc = shown.doc.clone();
    let groups: Vec<String> = shown
        .canvas
        .graph()
        .editor
        .groups
        .keys()
        .filter(|k| shown.canvas.selection.contains(*k))
        .cloned()
        .collect();
    if !groups.is_empty() {
        edit_layout(s, &doc, |layout| {
            layout.groups.retain(|k, _| !groups.contains(k))
        });
        if let Some(shown) = s.graph.shown.as_mut() {
            shown.canvas.selection.clear();
        }
        return;
    }
    let graph = shown.canvas.graph();
    let selected = &shown.canvas.selection;
    // A node's inline input defaults go with it.
    let defaults = graph
        .edges
        .values()
        .filter(|e| selected.contains(&e.to.node) && shown.canvas.is_default(&e.from.node))
        .map(|e| &e.from.node);
    let commands: Vec<_> = selected
        .iter()
        .filter(|id| graph.nodes.contains_key(*id))
        .chain(defaults)
        .map(|node| json!({"cmd": "graph.node.remove", "args": {"node": node}}))
        .collect();
    if commands.is_empty() {
        return;
    }
    match s.editor.apply_to(&doc, commands, "Delete graph nodes") {
        Ok(_) => {
            if let Some(shown) = s.graph.shown.as_mut() {
                shown.canvas.selection.clear();
            }
        }
        Err(error) => s.editor.report_failure(&error),
    }
}

/// The open canvas's inline input values: constant node and box centre in the canvas
/// (tests click them without a window).
#[cfg(test)]
pub(super) fn inline_values(s: &Studio) -> Vec<(String, (f32, f32))> {
    let rect = view_rect(s);
    s.graph.shown.as_ref().map_or_else(Vec::new, |g| {
        g.canvas
            .inline_boxes(rect)
            .map(|(node, _, b)| (node.to_owned(), (b.x + b.w * 0.5, b.y + b.h * 0.5)))
            .collect()
    })
}

/// Picks every node on the open canvas (Select All).
pub(super) fn select_all(s: &mut Studio) {
    if let Some(shown) = s.graph.shown.as_mut() {
        shown.canvas.selection = shown.canvas.node_ids().into_iter().collect();
        s.graph.dirty = true;
    }
}

/// Opens the node search under Add Node (Space).
pub(super) fn add(s: &mut Studio) {
    s.graph.adding = Some(Adding::default());
    s.local.focus = Some("material/add/search");
}

/// Picks `ids` on the open canvas (tests select without a window).
#[cfg(test)]
pub(super) fn select(s: &mut Studio, ids: &[String]) {
    if let Some(shown) = s.graph.shown.as_mut() {
        shown.canvas.selection = ids.iter().cloned().collect();
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

#[cfg(test)]
mod tests {
    use super::{connect, fits};
    use ara_graph::Direction::{Input, Output};
    use ara_material::Type;

    #[test]
    fn the_search_offers_only_nodes_that_fit_the_wire() {
        // From a colour output: operations taking colours, and constants.
        assert!(fits("add", Type::ColorLinear, Output));
        assert!(fits("literal", Type::ColorLinear, Output));
        assert!(!fits("uv", Type::ColorLinear, Output), "uv takes nothing");
        // Into a vec2 input: what gives a vec2.
        assert!(fits("uv", Type::Vec2, Input));
        assert!(fits("add", Type::Vec2, Input));
        assert!(!fits("dot", Type::Vec2, Input), "dot gives a scalar");
    }

    #[test]
    fn connecting_over_an_inline_default_removes_it() {
        let text = "material M : surface budget 1 {\n  param r : float = 0.4;\n  let mInput0 : float = 0.5;\n  let m : float = sin(mInput0);\n  output roughness = m;\n}\n";
        let material = ara_material::dsl::parse(text, "gr_m").expect("parses");
        let graph = material.to_graph(Default::default()).expect("graph");
        let id = |name: &str| material.node_id(name).expect(name).to_owned();
        let commands = connect(&graph, &id("r"), &id("m"), "in0");
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert_eq!(commands[1]["cmd"], "graph.node.remove");
        assert_eq!(commands[1]["args"]["node"], id("mInput0"));
        // Wiring the default back in removes nothing; nor does replacing a real node.
        assert_eq!(connect(&graph, &id("mInput0"), &id("m"), "in0").len(), 1);
        let text = text.replace("sin(mInput0)", "sin(r)");
        let material = ara_material::dsl::parse(&text, "gr_m").expect("parses");
        let graph = material.to_graph(Default::default()).expect("graph");
        let id = |name: &str| material.node_id(name).expect(name).to_owned();
        assert_eq!(connect(&graph, &id("mInput0"), &id("m"), "in0").len(), 1);
    }
}
