//! Shared retained graph canvas for materials and animation projections.
//! Semantic edits leave as typed commands; gestures retain only a transient preview.
mod interaction;
mod paint;
mod text;
use ara_graph::{Direction, Endpoint, Graph};
use ara_ui::{
    paint::{Box2, DisplayList},
    style::Color,
};
use std::collections::{BTreeMap, BTreeSet};
use text::Labels;

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
struct NodeView {
    id: String,
    title: String,
    subtitle: String,
    position: [f32; 2],
    height: f32,
    ports: Vec<(String, Direction, String)>,
}
struct Wire {
    id: String,
    from: Endpoint,
    to: Endpoint,
    points: Vec<[f32; 2]>,
}
/// Theme-derived semantic colors; port colors remain distinct in both themes.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Canvas background.
    pub background: Color,
    /// Node surface.
    pub card: Color,
    /// Subtle divider/grid color.
    pub border: Color,
    /// Primary readable text.
    pub foreground: Color,
    /// Secondary labels.
    pub muted: Color,
    /// Selection and current connection.
    pub accent: Color,
    /// Connection validation errors.
    pub error: Color,
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
                let message = diagnostic.message.chars().take(34).collect::<String>();
                self.labels.prepare(&message, text);
                self.errors.insert(id.clone(), message);
            }
        }
    }
    /// Semantic node/port geometry for agents, including culled nodes and annotations.
    pub fn report(&self, rect: Box2) -> serde_json::Value {
        serde_json::json!({"camera":self.camera,"selection":self.selection,"nodes":self.nodes.iter().map(|n|serde_json::json!({"id":n.id,"kind":n.subtitle,"box":self.node_rect(n,rect),"ports":n.ports.iter().map(|(port,direction,ty)|serde_json::json!({"port":port,"direction":direction,"type":ty,"position":self.port_position(&Endpoint{node:n.id.clone(),port:port.clone()},rect)})).collect::<Vec<_>>(),"error":self.errors.get(&n.id)})).collect::<Vec<_>>(),"editor":self.graph.editor})
    }
    fn refresh_views(&mut self, text: &mut ara_ui_text::TextSystem) {
        self.nodes.clear();
        self.index.clear();
        self.wires.clear();
        for (index, (id, node)) in self.graph.nodes.iter().enumerate() {
            let title = node
                .payload
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&node.kind)
                .to_owned();
            let position = self
                .graph
                .editor
                .positions
                .get(id)
                .copied()
                .unwrap_or_else(|| {
                    if self.graph.header.domain == "material" && self.graph.nodes.len() < 12 {
                        if id == "n_output" {
                            [300., 112.]
                        } else {
                            [
                                0.,
                                self.graph
                                    .nodes
                                    .keys()
                                    .filter(|key| key.as_str() != "n_output")
                                    .position(|key| key == id)
                                    .unwrap_or(index) as f32
                                    * 136.,
                            ]
                        }
                    } else {
                        [(index % 4) as f32 * 256., (index / 4) as f32 * 196.]
                    }
                });
            let inputs = node
                .ports
                .values()
                .filter(|p| p.direction == Direction::Input)
                .count();
            let outputs = node.ports.len() - inputs;
            let height = 64. + inputs.max(outputs) as f32 * 24.;
            self.labels.prepare(&title, text);
            self.labels.prepare(&node.kind, text);
            for (name, port) in &node.ports {
                self.labels.prepare(name, text);
                self.labels.prepare(&port.ty, text);
            }
            self.index.insert(id.clone(), self.nodes.len());
            self.nodes.push(NodeView {
                id: id.clone(),
                title,
                subtitle: node.kind.clone(),
                position,
                height,
                ports: node
                    .ports
                    .iter()
                    .map(|(k, p)| (k.clone(), p.direction, p.ty.clone()))
                    .collect(),
            });
        }
        for (id, edge) in &self.graph.edges {
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
            });
        }
        for comment in self.graph.editor.comments.values() {
            self.labels.prepare(&comment.text, text);
        }
        for group in self.graph.editor.groups.values() {
            self.labels.prepare(&group.label, text);
        }
        self.selection
            .retain(|id| self.graph.nodes.contains_key(id));
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
            w: 216. * self.camera.zoom,
            h: node.height * self.camera.zoom,
        }
    }
    /// Resolve a stable socket into host logical coordinates for semantic input/testing.
    pub fn endpoint_position(&self, endpoint: &Endpoint, rect: Box2) -> Option<[f32; 2]> {
        self.port_position(endpoint, rect)
    }
    fn port_position(&self, endpoint: &Endpoint, rect: Box2) -> Option<[f32; 2]> {
        let node = self.nodes.get(*self.index.get(&endpoint.node)?)?;
        let (_, direction, _) = node
            .ports
            .iter()
            .find(|(name, _, _)| *name == endpoint.port)?;
        let index = node
            .ports
            .iter()
            .filter(|(_, d, _)| d == direction)
            .position(|(name, _, _)| *name == endpoint.port)?;
        let r = self.node_rect(node, rect);
        Some([
            if *direction == Direction::Input {
                r.x
            } else {
                r.x + r.w
            },
            r.y + (64. + index as f32 * 24.) * self.camera.zoom,
        ])
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
