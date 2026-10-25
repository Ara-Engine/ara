//! Shared retained graph canvas for materials and animation projections.
//! Semantic edits leave as typed commands; gestures retain only a transient preview.
//! Presentation (titles, accents, pin labels) comes from `style`; geometry helpers
//! here are shared by painting, hit testing and the agent report so they never drift.
mod interaction;
mod paint;
pub(crate) mod style;
mod text;
use ara_graph::{Direction, Endpoint, Graph};
use ara_ui::{
    paint::{Box2, DisplayList},
    style::Color,
};
use std::collections::{BTreeMap, BTreeSet};
use style::{Accent, Shape};
use text::Labels;

/// Width of a pin node, in graph units.
const WIDTH: f32 = 208.;
/// Header band, including the 3 px category strip.
const HEADER: f32 = 30.;
/// Inline value line under the header (parameters and constants).
const VALUE: f32 = 24.;
/// One pin row.
const ROW: f32 = 22.;
/// Padding above the first and below the last pin row.
const PAD: f32 = 6.;
/// State-machine state size.
const STATE: [f32; 2] = [168., 44.];
/// The lowest zoom a graph opens at: node titles stay readable (12 px titles at
/// 10 px or more).
pub const READABLE: f32 = 0.85;
/// In-node preview square (rendered at 2x by the thumbnail queue).
pub const PREVIEW: f32 = 144.;

/// Presentation camera, in graph coordinates independent of the host viewport.
#[derive(
    Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ara_schema::Schema,
)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Camera {
    /// Pan offset in logical pixels.
    pub pan: [f32; 2],
    /// Positive zoom; ports and text remain crisp at physical DPI.
    pub zoom: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: [36., 48.],
            zoom: 0.85,
        }
    }
}
struct Pin {
    name: String,
    label: String,
    direction: Direction,
    ty: String,
    connected: bool,
}
struct NodeView {
    id: String,
    kind: String,
    look: style::Look,
    position: [f32; 2],
    width: f32,
    height: f32,
    ports: Vec<Pin>,
    /// Top of the preview square, in graph units from the node top.
    preview_top: Option<f32>,
}
impl NodeView {
    /// Top of the first pin row, below the header and any value line.
    fn pins_top(&self) -> f32 {
        HEADER + if self.look.value.is_some() { VALUE } else { 0. } + PAD
    }
}
struct Wire {
    id: String,
    from: Endpoint,
    to: Endpoint,
    points: Vec<[f32; 2]>,
    /// Transition condition summary, painted at the arrow's midpoint.
    label: Option<String>,
}
/// Theme-derived semantic colors; port colors remain distinct in both themes.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Canvas background.
    pub background: Color,
    /// Node surface.
    pub card: Color,
    /// Node headers and raised chips.
    pub raised: Color,
    /// Subtle divider/grid color.
    pub border: Color,
    /// Node outlines and region dividers.
    pub separator: Color,
    /// Primary readable text.
    pub foreground: Color,
    /// Secondary labels.
    pub muted: Color,
    /// Selection and current connection.
    pub accent: Color,
    /// Connection validation errors.
    pub error: Color,
    /// Entry markers and healthy states.
    pub success: Color,
    /// Category accents (`chart-1`..`chart-6`).
    pub chart: [Color; 6],
    /// Drop shadow under cards (alpha carries the strength).
    pub shadow: Color,
}
impl Theme {
    fn accent(&self, accent: Accent) -> Color {
        match accent {
            Accent::Neutral => self.muted,
            Accent::Primary => self.accent,
            Accent::Chart(index) => self.chart[index.min(5)],
        }
    }
}
/// A state machine's live state during Play.
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// The playing state's node.
    pub node: String,
    /// Normalized time through the playing state's cycle.
    pub progress: f32,
    /// A cross-fade in progress: the target state's node and the fade's progress.
    pub fade: Option<(String, f32)>,
}
/// Graph edit produced only at a gesture boundary. Submit through the host command path.
#[derive(Debug)]
pub enum Edit {
    /// A coalesced node move, with absolute graph coordinates.
    Move(Vec<(String, [f32; 2])>),
    /// A validated connection intent; the domain chooses its command representation.
    Connect(Endpoint, Endpoint),
    /// Nonsemantic comments, groups and reroutes, through graph.editor.set.
    Layout(ara_graph::Editor),
}
/// Bounded paint and hit-test projection. Offscreen nodes never enter the display list.
pub struct Canvas {
    /// Graph-space view transform; navigation has a separate undo history.
    pub camera: Camera,
    /// Selected stable node IDs.
    pub selection: BTreeSet<String>,
    /// Connection error retained inline until the next gesture.
    pub diagnostic: Option<String>,
    /// Edges drawn as selected (a transition chosen in the Animation sidebar).
    pub highlight: BTreeSet<String>,
    /// In-node preview images by node ID (UI image handles; the last good one stays).
    pub previews: BTreeMap<String, u32>,
    /// The state machine's live state during Play (playing state, cross-fade).
    pub live: Option<Live>,
    show_previews: bool,
    /// Nodes that have, or are rendering, an in-node preview; others stay compact.
    previewed: std::collections::BTreeSet<String>,
    graph: Graph,
    nodes: Vec<NodeView>,
    index: BTreeMap<String, usize>,
    wires: Vec<Wire>,
    labels: Labels,
    errors: BTreeMap<String, String>,
    drag: Option<interaction::Drag>,
    pointer: [f32; 2],
}
impl Canvas {
    /// Build derived geometry once on a semantic/layout revision.
    pub fn new(graph: Graph, text: &mut ara_ui_text::TextSystem) -> Self {
        let mut canvas = Self {
            camera: Camera::default(),
            selection: BTreeSet::new(),
            diagnostic: None,
            highlight: BTreeSet::new(),
            previews: BTreeMap::new(),
            live: None,
            show_previews: true,
            previewed: Default::default(),
            graph,
            nodes: Vec::new(),
            index: BTreeMap::new(),
            wires: Vec::new(),
            labels: Labels::default(),
            errors: BTreeMap::new(),
            drag: None,
            pointer: [0.; 2],
        };
        canvas.refresh_views(text);
        canvas
    }
    /// Show or hide in-node previews; node heights change, so views are rebuilt.
    pub fn show_previews(&mut self, shown: bool, text: &mut ara_ui_text::TextSystem) {
        if self.show_previews != shown {
            self.show_previews = shown;
            self.refresh_views(text);
        }
    }
    /// Nodes whose look can show an in-node preview, in graph order.
    pub fn preview_nodes(&self) -> impl Iterator<Item = &str> {
        self.nodes
            .iter()
            .filter(|n| n.look.preview)
            .map(|n| n.id.as_str())
    }
    /// The nodes that get a preview tile: those with a preview rendered or on its way.
    /// A node whose preview cannot be made keeps a compact body, never an empty tile.
    pub fn set_previewed(
        &mut self,
        nodes: std::collections::BTreeSet<String>,
        text: &mut ara_ui_text::TextSystem,
    ) {
        if self.previewed != nodes {
            self.previewed = nodes;
            self.refresh_views(text);
        }
    }
    /// Replace a read-only graph snapshot after the host publishes ops.
    pub fn refresh(&mut self, graph: Graph, text: &mut ara_ui_text::TextSystem) {
        self.graph = graph;
        self.refresh_views(text);
    }
    /// Attach compiler diagnostics to their stable node records without changing the graph.
    pub fn diagnostics(
        &mut self,
        diagnostics: &[ara_diag::Diagnostic],
        text: &mut ara_ui_text::TextSystem,
    ) {
        self.errors.clear();
        for diagnostic in diagnostics {
            if let Some(id) = &diagnostic.location.record {
                let mut message = diagnostic.message.chars().take(40).collect::<String>();
                if diagnostic.message.chars().count() > 40 {
                    message.push('…');
                }
                self.labels.prepare(&message, text);
                self.errors.insert(id.clone(), message);
            }
        }
    }
    /// Semantic node/port geometry for agents, including culled nodes and annotations.
    pub fn report(&self, rect: Box2) -> serde_json::Value {
        serde_json::json!({"camera":self.camera,"selection":self.selection,"nodes":self.nodes.iter().map(|n|serde_json::json!({"id":n.id,"kind":n.kind,"title":n.look.title,"box":self.node_rect(n,rect),"ports":n.ports.iter().map(|pin|serde_json::json!({"port":pin.name,"label":pin.label,"direction":pin.direction,"type":pin.ty,"connected":pin.connected,"position":self.port_position(&Endpoint{node:n.id.clone(),port:pin.name.clone()},rect)})).collect::<Vec<_>>(),"error":self.errors.get(&n.id)})).collect::<Vec<_>>(),"editor":self.graph.editor,"live":self.live.as_ref().map(|l|serde_json::json!({"node":l.node,"progress":l.progress,"fade":l.fade}))})
    }
    fn refresh_views(&mut self, text: &mut ara_ui_text::TextSystem) {
        self.nodes.clear();
        self.index.clear();
        self.wires.clear();
        let connected = self
            .graph
            .edges
            .values()
            .flat_map(|edge| [&edge.from, &edge.to])
            .map(|endpoint| (endpoint.node.as_str(), endpoint.port.as_str()))
            .collect::<BTreeSet<_>>();
        for (id, node) in &self.graph.nodes {
            let look = style::look(&self.graph, id, node);
            // Unplaced nodes (text-authored) are laid out below, once heights are known.
            let position = self
                .graph
                .editor
                .positions
                .get(id)
                .copied()
                .unwrap_or([f32::NAN; 2]);
            let ports = node
                .ports
                .iter()
                .map(|(name, port)| Pin {
                    name: name.clone(),
                    label: style::pin_label(node, name, port.direction, &port.ty),
                    direction: port.direction,
                    ty: port.ty.clone(),
                    connected: connected.contains(&(id.as_str(), name.as_str())),
                })
                .collect::<Vec<_>>();
            let inputs = ports
                .iter()
                .filter(|p| p.direction == Direction::Input)
                .count();
            let rows = inputs.max(ports.len() - inputs) as f32;
            let (width, mut height) = match look.shape {
                Shape::State => (STATE[0], STATE[1]),
                Shape::Pins => (
                    WIDTH,
                    HEADER + if look.value.is_some() { VALUE } else { 0. } + PAD * 2. + rows * ROW,
                ),
            };
            let preview_top = (look.preview && self.show_previews && self.previewed.contains(id))
                .then(|| {
                    let top = height;
                    height += PREVIEW + PAD * 2.;
                    top
                });
            for label in [&look.title, &look.caption]
                .into_iter()
                .chain(look.value.as_ref())
                .chain(ports.iter().map(|p| &p.label))
            {
                self.labels.prepare(label, text);
            }
            self.index.insert(id.clone(), self.nodes.len());
            self.nodes.push(NodeView {
                id: id.clone(),
                kind: node.kind.clone(),
                look,
                position,
                width,
                height,
                ports,
                preview_top,
            });
        }
        self.layout_unplaced();
        let transitions = self.graph.header.payload.get("transitions");
        for (id, edge) in &self.graph.edges {
            let label = transitions
                .and_then(|t| t.get(id))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if let Some(label) = &label {
                self.labels.prepare(label, text);
            }
            self.wires.push(Wire {
                id: id.clone(),
                from: edge.from.clone(),
                to: edge.to.clone(),
                points: self
                    .graph
                    .editor
                    .reroutes
                    .get(id)
                    .cloned()
                    .unwrap_or_default(),
                label,
            });
        }
        self.labels.prepare("Entry", text);
        for comment in self.graph.editor.comments.values() {
            self.labels.prepare(&comment.text, text);
        }
        for group in self.graph.editor.groups.values() {
            self.labels.prepare(&group.label, text);
        }
        self.selection
            .retain(|id| self.graph.nodes.contains_key(id));
        self.previews
            .retain(|id, _| self.graph.nodes.contains_key(id));
    }
    /// Lay out nodes without a stored position (text-authored graphs) left to right:
    /// each node sits one column before its nearest consumer, so inputs and constants
    /// land next to what uses them, and lines up with its consumers' average height.
    /// Stored positions are never moved.
    fn layout_unplaced(&mut self) {
        if self.nodes.iter().all(|n| n.position[0].is_finite()) {
            return;
        }
        let count = self.nodes.len();
        let mut consumers = vec![Vec::new(); count];
        for edge in self.graph.edges.values() {
            if let (Some(&from), Some(&to)) = (
                self.index.get(&edge.from.node),
                self.index.get(&edge.to.node),
            ) && from != to
            {
                consumers[from].push(to);
            }
        }
        // Longest distance to a sink; bounded rounds also terminate on cycles.
        let mut reach = vec![0usize; count];
        for _ in 0..count.min(64) {
            let mut changed = false;
            for (from, tos) in consumers.iter().enumerate() {
                for &to in tos {
                    if reach[from] < reach[to] + 1 {
                        reach[from] = reach[to] + 1;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let last = reach.iter().copied().max().unwrap_or(0);
        // Right to left: a column's consumers are already placed.
        for column in (0..=last).rev() {
            let mut members = (0..count)
                .filter(|&i| last - reach[i] == column && !self.nodes[i].position[0].is_finite())
                .map(|i| {
                    let centres = consumers[i]
                        .iter()
                        .map(|&c| &self.nodes[c])
                        .filter(|c| c.position[1].is_finite())
                        .map(|c| c.position[1] + c.height * 0.5)
                        .collect::<Vec<_>>();
                    let target = if centres.is_empty() {
                        f32::NAN
                    } else {
                        centres.iter().sum::<f32>() / centres.len() as f32
                    };
                    (i, target)
                })
                .collect::<Vec<_>>();
            members.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            let mut floor = f32::NEG_INFINITY;
            for (i, target) in members {
                let node = &mut self.nodes[i];
                let wanted = if target.is_finite() {
                    target - node.height * 0.5
                } else {
                    floor.max(0.)
                };
                let y = wanted.max(floor);
                node.position = [column as f32 * (WIDTH + 72.), y];
                floor = y + node.height + 24.;
            }
        }
    }
    /// Borrow the current model for domain inspectors and commands.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }
    /// Convert graph coordinates to host logical pixels.
    pub fn screen(&self, p: [f32; 2], rect: Box2) -> [f32; 2] {
        [
            rect.x + self.camera.pan[0] + p[0] * self.camera.zoom,
            rect.y + self.camera.pan[1] + p[1] * self.camera.zoom,
        ]
    }
    /// Convert a host logical position into graph coordinates.
    pub fn world(&self, p: [f32; 2], rect: Box2) -> [f32; 2] {
        [
            (p[0] - rect.x - self.camera.pan[0]) / self.camera.zoom,
            (p[1] - rect.y - self.camera.pan[1]) / self.camera.zoom,
        ]
    }
    /// Zoom around the pointer, preserving its graph-space anchor.
    pub fn zoom(&mut self, at: [f32; 2], amount: f32, rect: Box2) {
        let before = self.world(at, rect);
        self.camera.zoom = (self.camera.zoom * (amount * 0.12).exp()).clamp(0.2, 2.5);
        self.camera.pan = [
            at[0] - rect.x - before[0] * self.camera.zoom,
            at[1] - rect.y - before[1] * self.camera.zoom,
        ];
    }
    /// Paint only visible nodes/connections; labels are pre-shaped on model changes.
    pub fn paint(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        self.paint_impl(list, rect, theme);
    }
    /// Number of visible node cards, useful for perf evidence and accessibility projections.
    pub fn visible_nodes(&self, rect: Box2) -> usize {
        self.nodes
            .iter()
            .filter(|n| self.node_rect(n, rect).intersects(rect))
            .count()
    }
    /// Frame all cards with a gutter, leaving room for the minimap when it shows.
    pub fn framed(&self, rect: Box2) -> Camera {
        let bounds = self.bounds();
        let width = (rect.w - 64.).max(1.);
        let height = (rect.h - 96.).max(1.);
        let zoom = (width / bounds.w.max(1.))
            .min(height / bounds.h.max(1.))
            .clamp(0.15, 1.);
        Camera {
            zoom,
            pan: [
                32. + (width - bounds.w * zoom) * 0.5 - bounds.x * zoom,
                40. + (height - bounds.h * zoom) * 0.5 - bounds.y * zoom,
            ],
        }
    }
    /// Framing for a graph that just opened: like [`Self::framed`], but never below
    /// [`READABLE`]. Along an axis the graph does not fit at that zoom, its output end
    /// stays in view: the nodes nothing consumes sit at the right, centred vertically,
    /// since graphs read into their outputs.
    pub fn framed_readable(&self, rect: Box2) -> Camera {
        let fit = self.framed(rect);
        if fit.zoom >= READABLE {
            return fit;
        }
        let zoom = READABLE;
        let bounds = self.bounds();
        let consumed: std::collections::BTreeSet<&str> = self
            .graph
            .edges
            .values()
            .map(|e| e.from.node.as_str())
            .collect();
        let sinks = self
            .nodes
            .iter()
            .filter(|n| !consumed.contains(n.id.as_str()))
            .map(|n| Box2 {
                x: n.position[0],
                y: n.position[1],
                w: n.width,
                h: n.height,
            })
            .reduce(paint::union)
            .unwrap_or(bounds);
        let width = (rect.w - 64.).max(1.);
        let height = (rect.h - 96.).max(1.);
        Camera {
            zoom,
            pan: [
                if bounds.w * zoom <= width {
                    32. + (width - bounds.w * zoom) * 0.5 - bounds.x * zoom
                } else {
                    32. + width - (sinks.x + sinks.w) * zoom
                },
                if bounds.h * zoom <= height {
                    40. + (height - bounds.h * zoom) * 0.5 - bounds.y * zoom
                } else {
                    40. + height * 0.5 - (sinks.y + sinks.h * 0.5) * zoom
                },
            ],
        }
    }
    /// The topmost node under a screen point, for context menus.
    pub fn node_at(&self, p: [f32; 2], rect: Box2) -> Option<String> {
        self.nodes
            .iter()
            .rev()
            .find(|n| self.node_rect(n, rect).contains(p[0], p[1]))
            .map(|n| n.id.clone())
    }
    /// Every node id, for Select All.
    pub fn node_ids(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.id.clone()).collect()
    }
    fn node_rect(&self, node: &NodeView, rect: Box2) -> Box2 {
        let position = if let Some(interaction::Drag::Nodes {
            positions, start, ..
        }) = &self.drag
        {
            positions
                .get(&node.id)
                .map(|p| {
                    [
                        p[0] + (self.pointer[0] - start[0]) / self.camera.zoom,
                        p[1] + (self.pointer[1] - start[1]) / self.camera.zoom,
                    ]
                })
                .unwrap_or(node.position)
        } else {
            node.position
        };
        let p = self.screen(position, rect);
        Box2 {
            x: p[0],
            y: p[1],
            w: node.width * self.camera.zoom,
            h: node.height * self.camera.zoom,
        }
    }
    /// Resolve a stable socket into host logical coordinates for semantic input/testing.
    pub fn endpoint_position(&self, endpoint: &Endpoint, rect: Box2) -> Option<[f32; 2]> {
        self.port_position(endpoint, rect)
    }
    fn port_position(&self, endpoint: &Endpoint, rect: Box2) -> Option<[f32; 2]> {
        let node = self.nodes.get(*self.index.get(&endpoint.node)?)?;
        let pin = node.ports.iter().find(|pin| pin.name == endpoint.port)?;
        let r = self.node_rect(node, rect);
        let x = if pin.direction == Direction::Input {
            r.x
        } else {
            r.x + r.w
        };
        if node.look.shape == Shape::State {
            // States connect edge to edge; their sockets are the side midpoints.
            return Some([x, r.y + r.h * 0.5]);
        }
        let index = node
            .ports
            .iter()
            .filter(|p| p.direction == pin.direction)
            .position(|p| p.name == endpoint.port)?;
        Some([
            x,
            r.y + (node.pins_top() + (index as f32 + 0.5) * ROW) * self.camera.zoom,
        ])
    }
    /// The minimap's screen rectangle, when the graph does not fit the view.
    fn minimap(&self, rect: Box2) -> Option<Box2> {
        let bounds = self.bounds();
        let a = self.screen([bounds.x, bounds.y], rect);
        let b = self.screen([bounds.x + bounds.w, bounds.y + bounds.h], rect);
        let fits =
            a[0] >= rect.x && a[1] >= rect.y && b[0] <= rect.x + rect.w && b[1] <= rect.y + rect.h;
        (!fits && rect.w > 360. && rect.h > 240.).then_some(Box2 {
            x: rect.x + rect.w - 156.,
            y: rect.y + rect.h - 108.,
            w: 144.,
            h: 96.,
        })
    }
}
/// Nominal port types share the same color everywhere the widget is used.
pub fn port_color(ty: &str) -> Color {
    if ty.contains("Color") {
        Color(230, 151, 208, 255)
    } else if ty.contains("Texture") {
        Color(216, 170, 95, 255)
    } else if ty.contains("vec") || ty.contains("pose") {
        Color(105, 188, 154, 255)
    } else if ty == "bool" {
        Color(199, 118, 137, 255)
    } else {
        Color(125, 168, 224, 255)
    }
}
