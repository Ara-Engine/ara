use super::*;
pub(super) enum Drag {
    Pan {
        start: [f32; 2],
        pan: [f32; 2],
    },
    Box {
        start: [f32; 2],
        additive: bool,
    },
    Nodes {
        start: [f32; 2],
        positions: BTreeMap<String, [f32; 2]>,
    },
    Connect {
        endpoint: Endpoint,
        direction: Direction,
    },
    Comment {
        key: String,
        start: [f32; 2],
    },
    Reroute {
        edge: String,
        index: usize,
        start: [f32; 2],
    },
}
impl Canvas {
    /// Begin pan (middle/right), node move, box select or a socket connection.
    pub fn begin(&mut self, p: [f32; 2], button: u8, additive: bool, rect: Box2) {
        self.pointer = p;
        self.diagnostic = None;
        if button == 2 || button == 3 {
            self.drag = Some(Drag::Pan {
                start: p,
                pan: self.camera.pan,
            });
            return;
        }
        if p[0] > rect.x + rect.w - 150. && p[1] > rect.y + rect.h - 104. {
            let bounds = self.bounds();
            let scale = (120. / bounds.w).min(74. / bounds.h);
            let world = [
                bounds.x + (p[0] - (rect.x + rect.w - 141.)) / scale,
                bounds.y + (p[1] - (rect.y + rect.h - 95.)) / scale,
            ];
            self.camera.pan = [
                rect.w * 0.5 - world[0] * self.camera.zoom,
                rect.h * 0.5 - world[1] * self.camera.zoom,
            ];
            return;
        }
        if let Some((endpoint, direction)) = self.hit_port(p, rect) {
            self.drag = Some(Drag::Connect {
                endpoint,
                direction,
            });
            return;
        }
        for (edge, points) in &self.graph.editor.reroutes {
            if let Some(index) = points.iter().position(|point| {
                let point = self.screen(*point, rect);
                (point[0] - p[0]).hypot(point[1] - p[1]) < 9.
            }) {
                self.selection.clear();
                self.selection.insert(edge.clone());
                self.drag = Some(Drag::Reroute {
                    edge: edge.clone(),
                    index,
                    start: p,
                });
                return;
            }
        }
        for (key, comment) in &self.graph.editor.comments {
            let point = self.screen(comment.position, rect);
            if (Box2 {
                x: point[0],
                y: point[1],
                w: 220. * self.camera.zoom,
                h: 42. * self.camera.zoom,
            })
            .contains(p[0], p[1])
            {
                self.selection.clear();
                self.selection.insert(key.clone());
                self.drag = Some(Drag::Comment {
                    key: key.clone(),
                    start: p,
                });
                return;
            }
        }
        for (key, group) in &self.graph.editor.groups {
            let mut bounds = None::<Box2>;
            for node in self.nodes.iter().filter(|n| group.nodes.contains(&n.id)) {
                let r = self.node_rect(node, rect);
                bounds = Some(bounds.map_or(r, |b| Box2 {
                    x: b.x.min(r.x),
                    y: b.y.min(r.y),
                    w: (b.x + b.w).max(r.x + r.w) - b.x.min(r.x),
                    h: (b.y + b.h).max(r.y + r.h) - b.y.min(r.y),
                }));
            }
            if bounds.is_some_and(|r| {
                (Box2 {
                    x: r.x - 16.,
                    y: r.y - 38.,
                    w: r.w + 32.,
                    h: 38.,
                })
                .contains(p[0], p[1])
            }) {
                self.selection = group.nodes.iter().cloned().collect();
                self.selection.insert(key.clone());
                self.drag = Some(Drag::Nodes {
                    start: p,
                    positions: self
                        .nodes
                        .iter()
                        .filter(|n| group.nodes.contains(&n.id))
                        .map(|n| (n.id.clone(), n.position))
                        .collect(),
                });
                return;
            }
        }
        if let Some(node) = self
            .nodes
            .iter()
            .rev()
            .find(|n| self.node_rect(n, rect).contains(p[0], p[1]))
        {
            let id = node.id.clone();
            if additive {
                if !self.selection.remove(&id) {
                    self.selection.insert(id.clone());
                }
            } else if !self.selection.contains(&id) {
                self.selection.clear();
                self.selection.insert(id);
            }
            self.drag = Some(Drag::Nodes {
                start: p,
                positions: self
                    .nodes
                    .iter()
                    .filter(|n| self.selection.contains(&n.id))
                    .map(|n| (n.id.clone(), n.position))
                    .collect(),
            });
        } else {
            self.drag = Some(Drag::Box { start: p, additive });
        }
    }
    /// Update transient gesture geometry; the authoring graph remains unchanged.
    pub fn motion(&mut self, p: [f32; 2]) {
        self.pointer = p;
        if let Some(Drag::Pan { start, pan }) = &self.drag {
            self.camera.pan = [pan[0] + p[0] - start[0], pan[1] + p[1] - start[1]];
        }
    }
    /// Cancel a preview, restoring the pan camera and keeping authoring untouched.
    pub fn cancel(&mut self) {
        if let Some(Drag::Pan { pan, .. }) = self.drag.take() {
            self.camera.pan = pan;
        }
    }
    /// Finish one gesture. Node moves are returned as one atomic command batch.
    pub fn end(&mut self, p: [f32; 2], rect: Box2) -> Result<Vec<Edit>, String> {
        self.pointer = p;
        let Some(drag) = self.drag.take() else {
            return Ok(Vec::new());
        };
        match drag {
            Drag::Pan { .. } => Ok(Vec::new()),
            Drag::Comment { key, start } => {
                let mut layout = self.graph.editor.clone();
                if let Some(comment) = layout.comments.get_mut(&key) {
                    comment.position = self.drag_point(comment.position, start);
                }
                Ok(vec![Edit::Layout(layout)])
            }
            Drag::Reroute { edge, index, start } => {
                let mut layout = self.graph.editor.clone();
                if let Some(point) = layout
                    .reroutes
                    .get_mut(&edge)
                    .and_then(|points| points.get_mut(index))
                {
                    *point = self.drag_point(*point, start);
                }
                Ok(vec![Edit::Layout(layout)])
            }
            Drag::Nodes { start, positions } => {
                if (p[0] - start[0]).hypot(p[1] - start[1]) < 2. {
                    return Ok(Vec::new());
                }
                Ok(vec![Edit::Move(
                    positions
                        .into_iter()
                        .map(|(id, base)| {
                            (
                                id,
                                [
                                    base[0] + (p[0] - start[0]) / self.camera.zoom,
                                    base[1] + (p[1] - start[1]) / self.camera.zoom,
                                ],
                            )
                        })
                        .collect(),
                )])
            }
            Drag::Box { start, additive } => {
                if !additive {
                    self.selection.clear();
                }
                let box_rect = Box2 {
                    x: p[0].min(start[0]),
                    y: p[1].min(start[1]),
                    w: (p[0] - start[0]).abs(),
                    h: (p[1] - start[1]).abs(),
                };
                for node in &self.nodes {
                    if self.node_rect(node, rect).intersects(box_rect) {
                        self.selection.insert(node.id.clone());
                    }
                }
                Ok(Vec::new())
            }
            Drag::Connect {
                endpoint,
                direction,
            } => {
                let Some((other, other_direction)) = self.hit_port(p, rect) else {
                    return Ok(Vec::new());
                };
                let (from, to) = if direction == Direction::Output {
                    (endpoint, other)
                } else {
                    (other, endpoint)
                };
                let source = self
                    .graph
                    .nodes
                    .get(&from.node)
                    .and_then(|n| n.ports.get(&from.port));
                let target = self
                    .graph
                    .nodes
                    .get(&to.node)
                    .and_then(|n| n.ports.get(&to.port));
                let error = if direction == other_direction {
                    Some("Connect an output to an input.")
                } else if source.zip(target).is_none_or(|(a, b)| a.ty != b.ty) {
                    Some("Port types do not match.")
                } else {
                    None
                };
                if let Some(error) = error {
                    self.diagnostic = Some(error.into());
                    return Err(error.into());
                }
                Ok(vec![Edit::Connect(from, to)])
            }
        }
    }
    pub(super) fn drag_point(&self, base: [f32; 2], start: [f32; 2]) -> [f32; 2] {
        [
            base[0] + (self.pointer[0] - start[0]) / self.camera.zoom,
            base[1] + (self.pointer[1] - start[1]) / self.camera.zoom,
        ]
    }
    fn hit_port(&self, p: [f32; 2], rect: Box2) -> Option<(Endpoint, Direction)> {
        for node in self.nodes.iter().rev() {
            for (port, direction, _) in &node.ports {
                let endpoint = Endpoint {
                    node: node.id.clone(),
                    port: port.clone(),
                };
                if let Some(at) = self.port_position(&endpoint, rect)
                    && (at[0] - p[0]).hypot(at[1] - p[1]) < 9.
                {
                    return Some((endpoint, *direction));
                }
            }
        }
        None
    }
}
