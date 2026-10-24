//! Canvas painting: grid, groups, wires or state arrows, nodes, notes, gesture
//! previews and the minimap. Everything derives from the pre-shaped labels and the
//! node views, so a frame only transforms geometry (no shaping, no allocation per
//! glyph run beyond the display list).
use super::*;
use ara_ui::{paint::Draw, source::stable_id};

/// Caption and pin text size relative to the shaping size.
const SMALL: f32 = 11. / text::SIZE;

impl Canvas {
    pub(super) fn paint_impl(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        list.commands.push(Draw::PushClip { rect });
        fill(list, 0, rect, 0., theme.background);
        self.grid(list, rect, theme);
        self.groups(list, rect, theme);
        let states = self.nodes.iter().any(|n| n.look.shape == Shape::State);
        if states {
            self.arrows(list, rect, theme);
        } else {
            self.wires(list, rect);
        }
        for node in &self.nodes {
            let r = self.node_rect(node, rect);
            if !r.intersects(rect) {
                continue;
            }
            match node.look.shape {
                Shape::Pins => self.pin_node(list, node, r, theme),
                Shape::State => self.state_node(list, node, r, theme),
            }
        }
        self.comments(list, rect, theme);
        self.gesture(list, rect, theme);
        self.minimap_paint(list, rect, theme);
        list.commands.push(Draw::PopClip);
    }

    /// A quiet dot grid with faint major lines every eight cells.
    fn grid(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        let z = self.camera.zoom;
        let major = 128. * z;
        let mut line = theme.border;
        line.3 = 150;
        if major >= 40. {
            let offset = [
                self.camera.pan[0].rem_euclid(major),
                self.camera.pan[1].rem_euclid(major),
            ];
            for x in 0..=(rect.w / major) as usize {
                fill(
                    list,
                    1,
                    Box2 {
                        x: rect.x + offset[0] + x as f32 * major,
                        y: rect.y,
                        w: 1.,
                        h: rect.h,
                    },
                    0.,
                    line,
                );
            }
            for y in 0..=(rect.h / major) as usize {
                fill(
                    list,
                    1,
                    Box2 {
                        x: rect.x,
                        y: rect.y + offset[1] + y as f32 * major,
                        w: rect.w,
                        h: 1.,
                    },
                    0.,
                    line,
                );
            }
        }
        let step = 32. * z;
        if step < 14. {
            return;
        }
        let offset = [
            self.camera.pan[0].rem_euclid(step),
            self.camera.pan[1].rem_euclid(step),
        ];
        let mut dot = theme.separator;
        dot.3 = 170;
        for x in 0..(rect.w / step).ceil() as usize {
            for y in 0..(rect.h / step).ceil() as usize {
                fill(
                    list,
                    1,
                    Box2 {
                        x: rect.x + offset[0] + x as f32 * step - 0.75,
                        y: rect.y + offset[1] + y as f32 * step - 0.75,
                        w: 1.5,
                        h: 1.5,
                    },
                    0.75,
                    dot,
                );
            }
        }
    }

    fn groups(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        for group in self.graph.editor.groups.values() {
            let mut bounds = None::<Box2>;
            for node in self.nodes.iter().filter(|n| group.nodes.contains(&n.id)) {
                let r = self.node_rect(node, rect);
                bounds = Some(bounds.map_or(r, |b| union(b, r)));
            }
            if let Some(b) = bounds {
                let r = Box2 {
                    x: b.x - 16.,
                    y: b.y - 38.,
                    w: b.w + 32.,
                    h: b.h + 54.,
                };
                fill(list, 2, r, 10., alpha(theme.accent, 14));
                border(list, 2, r, 10., 1., theme.separator);
                self.labels.paint(
                    list,
                    2,
                    &group.label,
                    [r.x + 12., r.y + 10.],
                    1.,
                    theme.muted,
                );
            }
        }
    }

    /// Typed curves between pins. Wires touching the selection draw thicker; with a
    /// selection, the rest step back.
    fn wires(&self, list: &mut DisplayList, rect: Box2) {
        let selecting = !self.selection.is_empty();
        for wire in &self.wires {
            let (Some(a), Some(b)) = (
                self.port_position(&wire.from, rect),
                self.port_position(&wire.to, rect),
            ) else {
                continue;
            };
            let bounds = Box2 {
                x: a[0].min(b[0]) - 120.,
                y: a[1].min(b[1]) - 32.,
                w: (b[0] - a[0]).abs() + 240.,
                h: (b[1] - a[1]).abs() + 64.,
            };
            if !bounds.intersects(rect) && wire.points.is_empty() {
                continue;
            }
            let ty = self
                .graph
                .nodes
                .get(&wire.from.node)
                .and_then(|n| n.ports.get(&wire.from.port))
                .map(|p| p.ty.as_str())
                .unwrap_or("float");
            let related = self.selection.contains(&wire.from.node)
                || self.selection.contains(&wire.to.node)
                || self.selection.contains(&wire.id);
            let mut color = port_color(ty);
            if selecting && !related {
                color.3 = 120;
            }
            let width = if related { 2.75 } else { 2. };
            let id = stable_id(&wire.id);
            let mut previous = a;
            for (index, p) in wire.points.iter().enumerate() {
                let p = if let Some(super::interaction::Drag::Reroute {
                    edge,
                    index: dragged,
                    start,
                }) = &self.drag
                {
                    if edge == &wire.id && index == *dragged {
                        self.drag_point(*p, *start)
                    } else {
                        *p
                    }
                } else {
                    *p
                };
                let p = self.screen(p, rect);
                curve(list, id, previous, p, width, color);
                fill(
                    list,
                    id,
                    Box2 {
                        x: p[0] - 4.,
                        y: p[1] - 4.,
                        w: 8.,
                        h: 8.,
                    },
                    4.,
                    color,
                );
                previous = p;
            }
            curve(list, id, previous, b, width, color);
        }
    }

    /// State-machine transitions: straight arrows edge to edge, reverse pairs side by
    /// side, each with its condition summary at the midpoint; plus the Entry marker.
    fn arrows(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        let z = self.camera.zoom;
        let pairs = self
            .wires
            .iter()
            .map(|w| (w.from.node.as_str(), w.to.node.as_str()))
            .collect::<BTreeSet<_>>();
        for wire in &self.wires {
            let (Some(a), Some(b)) = (
                self.index.get(&wire.from.node).map(|i| &self.nodes[*i]),
                self.index.get(&wire.to.node).map(|i| &self.nodes[*i]),
            ) else {
                continue;
            };
            let (ra, rb) = (self.node_rect(a, rect), self.node_rect(b, rect));
            if !union(ra, rb).intersects(rect) {
                continue;
            }
            let related = self.selection.contains(&a.id)
                || self.selection.contains(&b.id)
                || self.highlight.contains(&wire.id);
            let color = if related { theme.accent } else { theme.muted };
            let id = stable_id(&wire.id);
            if a.id == b.id {
                // A self-transition loops over the top edge.
                let top = [ra.x + ra.w * 0.5, ra.y];
                let from = [top[0] + 18. * z, top[1]];
                let to = [top[0] - 18. * z, top[1]];
                let lift = 30. * z;
                bezier(
                    list,
                    id,
                    [
                        from,
                        [from[0] + 12. * z, from[1] - lift],
                        [to[0] - 12. * z, to[1] - lift],
                        to,
                    ],
                    1.75,
                    color,
                );
                head(list, id, to, [0., 1.], color);
                continue;
            }
            let ca = centre(ra);
            let cb = centre(rb);
            let length = (cb[0] - ca[0]).hypot(cb[1] - ca[1]).max(1e-3);
            let d = [(cb[0] - ca[0]) / length, (cb[1] - ca[1]) / length];
            let n = [-d[1], d[0]];
            let shift = if pairs.contains(&(b.id.as_str(), a.id.as_str())) {
                6. * z.max(0.6)
            } else {
                0.
            };
            let oa = [ca[0] + n[0] * shift, ca[1] + n[1] * shift];
            let ob = [cb[0] + n[0] * shift, cb[1] + n[1] * shift];
            let start = edge_point(ra, oa, d, 3.);
            let end = edge_point(rb, ob, [-d[0], -d[1]], 4.);
            line(list, id, start, end, 1.75, color);
            head(list, id, end, d, color);
            if let Some(label) = wire.label.as_deref().filter(|_| z >= 0.5) {
                let scale = z * SMALL;
                let width = self.labels.width(label) * scale + 12. * z;
                let height = 18. * z;
                // Beside its own arrow: a reverse pair's labels sit on opposite sides.
                let side = if shift > 0. { 1. } else { -1. };
                let reach = (n[0].abs() * width + n[1].abs() * height) * 0.5 + 6. * z;
                let mid = [
                    (start[0] + end[0]) * 0.5 + n[0] * side * reach,
                    (start[1] + end[1]) * 0.5 + n[1] * side * reach,
                ];
                let pill = Box2 {
                    x: mid[0] - width * 0.5,
                    y: mid[1] - height * 0.5,
                    w: width,
                    h: height,
                };
                fill(list, id, pill, height * 0.5, theme.card);
                border(
                    list,
                    id,
                    pill,
                    height * 0.5,
                    1.,
                    if related {
                        theme.accent
                    } else {
                        theme.separator
                    },
                );
                self.labels.paint(
                    list,
                    id,
                    label,
                    [pill.x + 6. * z, pill.y + 2.5 * z],
                    scale,
                    if related {
                        theme.foreground
                    } else {
                        theme.muted
                    },
                );
            }
        }
        if let Some(initial) = self.nodes.iter().find(|n| n.look.initial) {
            let r = self.node_rect(initial, rect);
            let pill = Box2 {
                x: r.x - 92. * z,
                y: r.y + r.h * 0.5 - 11. * z,
                w: 58. * z,
                h: 22. * z,
            };
            if pill.intersects(rect) {
                let id = stable_id("entry");
                fill(
                    list,
                    id,
                    pill,
                    pill.h * 0.5,
                    mix(theme.card, theme.success, 0.16),
                );
                border(list, id, pill, pill.h * 0.5, 1., theme.success);
                let scale = z * SMALL;
                self.labels.paint(
                    list,
                    id,
                    "Entry",
                    [
                        pill.x + (pill.w - self.labels.width("Entry") * scale) * 0.5,
                        pill.y + 3.5 * z,
                    ],
                    scale,
                    theme.success,
                );
                let from = [pill.x + pill.w + 3., pill.y + pill.h * 0.5];
                let to = [r.x - 4., from[1]];
                line(list, id, from, to, 1.75, theme.success);
                head(list, id, to, [1., 0.], theme.success);
            }
        }
    }

    /// A titled card: tinted category header with an accent rule, optional value
    /// line, then typed pins (hollow when unconnected) with their labels.
    fn pin_node(&self, list: &mut DisplayList, node: &NodeView, r: Box2, theme: Theme) {
        let z = self.camera.zoom;
        let id = stable_id(&node.id);
        let radius = 6. * z;
        let selected = self.selection.contains(&node.id);
        let error = self.errors.get(&node.id);
        let accent = theme.accent(node.look.accent);
        fill(list, id, offset(r, 0., 2. * z), radius, theme.shadow);
        if selected {
            border(list, id, grow(r, 3.), radius + 3., 2., theme.accent);
        }
        fill(list, id, r, radius, theme.card);
        // Header: rounded top, flat bottom, tinted by category, ruled in its accent.
        let header = Box2 { h: HEADER * z, ..r };
        let tint = mix(theme.raised, accent, 0.14);
        fill(list, id, header, radius, tint);
        fill(
            list,
            id,
            Box2 {
                y: header.y + header.h - radius,
                h: radius,
                ..header
            },
            0.,
            tint,
        );
        fill(
            list,
            id,
            Box2 {
                y: header.y + header.h - 2. * z,
                h: 2. * z,
                ..header
            },
            0.,
            accent,
        );
        border(
            list,
            id,
            r,
            radius,
            if error.is_some() { 1.5 } else { 1. },
            if error.is_some() {
                theme.error
            } else {
                theme.separator
            },
        );
        let detailed = z >= 0.45;
        let small = z * SMALL;
        let title_width = self.labels.width(&node.look.title) * z;
        let caption_width = self.labels.width(&node.look.caption) * small;
        self.labels.paint(
            list,
            id,
            &node.look.title,
            [r.x + 10. * z, r.y + 6.5 * z],
            z,
            theme.foreground,
        );
        if detailed && !node.look.caption.is_empty() && title_width + caption_width + 30. * z < r.w
        {
            self.labels.paint(
                list,
                id,
                &node.look.caption,
                [r.x + r.w - 10. * z - caption_width, r.y + 7.5 * z],
                small,
                theme.muted,
            );
        }
        if let Some(value) = node.look.value.as_deref().filter(|_| detailed) {
            let y = r.y + HEADER * z;
            let mut x = r.x + 10. * z;
            if let Some(swatch) = node.look.swatch {
                let chip = Box2 {
                    x,
                    y: y + 6. * z,
                    w: 12. * z,
                    h: 12. * z,
                };
                fill(list, id, chip, 3. * z, swatch);
                border(list, id, chip, 3. * z, 1., theme.separator);
                x += 18. * z;
            }
            self.labels
                .paint(list, id, value, [x, y + 4. * z], z, theme.foreground);
        }
        let mut rows = [0usize; 2];
        for pin in &node.ports {
            let input = pin.direction == Direction::Input;
            let row = &mut rows[usize::from(!input)];
            let y = r.y + (node.pins_top() + (*row as f32 + 0.5) * ROW) * z;
            *row += 1;
            let x = if input { r.x } else { r.x + r.w };
            let color = port_color(&pin.ty);
            let outer = 4. * z.max(0.6);
            fill(
                list,
                id,
                Box2 {
                    x: x - outer,
                    y: y - outer,
                    w: outer * 2.,
                    h: outer * 2.,
                },
                outer,
                color,
            );
            if !pin.connected {
                let inner = outer * 0.55;
                fill(
                    list,
                    id,
                    Box2 {
                        x: x - inner,
                        y: y - inner,
                        w: inner * 2.,
                        h: inner * 2.,
                    },
                    inner,
                    theme.card,
                );
            }
            if detailed {
                let width = self.labels.width(&pin.label) * small;
                let at = if input {
                    x + 12. * z
                } else {
                    x - 12. * z - width
                };
                self.labels.paint(
                    list,
                    id,
                    &pin.label,
                    [at, y - 7. * z],
                    small,
                    if pin.connected {
                        theme.foreground
                    } else {
                        theme.muted
                    },
                );
            }
        }
        if let Some(top) = node.preview_top {
            // The value as Shader Graph shows it; a quiet tile until the render lands.
            let size = PREVIEW * z;
            let tile = Box2 {
                x: r.x + (r.w - size) * 0.5,
                y: r.y + (top + PAD) * z,
                w: size,
                h: size,
            };
            match self.previews.get(&node.id) {
                Some(image) => list.commands.push(Draw::Image {
                    node: id,
                    rect: tile,
                    image: *image,
                    tint: Color(255, 255, 255, 255),
                }),
                _ => fill(list, id, tile, size / 16., theme.raised),
            }
        }
        if let Some(error) = error {
            self.labels.paint(
                list,
                id,
                error,
                [r.x + 2., r.y + r.h + 5. * z],
                small,
                theme.error,
            );
        }
    }

    /// A state: a rounded box with its name and pose; the default state is tinted.
    /// Hovered or selected states show the drag handle that starts a transition.
    fn state_node(&self, list: &mut DisplayList, node: &NodeView, r: Box2, theme: Theme) {
        let z = self.camera.zoom;
        let id = stable_id(&node.id);
        let radius = 8. * z;
        let selected = self.selection.contains(&node.id);
        fill(list, id, offset(r, 0., 2. * z), radius, theme.shadow);
        if selected {
            border(list, id, grow(r, 3.), radius + 3., 2., theme.accent);
        }
        let (body, outline) = if node.look.initial {
            (
                mix(theme.card, theme.accent, 0.16),
                alpha(theme.accent, 170),
            )
        } else {
            (theme.card, theme.separator)
        };
        fill(list, id, r, radius, body);
        border(list, id, r, radius, 1., outline);
        let small = z * SMALL;
        let caption = !node.look.caption.is_empty() && z >= 0.45;
        let title = self.labels.width(&node.look.title) * z;
        self.labels.paint(
            list,
            id,
            &node.look.title,
            [
                r.x + (r.w - title) * 0.5,
                r.y + if caption { 6. } else { 13. } * z,
            ],
            z,
            theme.foreground,
        );
        if caption {
            let width = self.labels.width(&node.look.caption) * small;
            self.labels.paint(
                list,
                id,
                &node.look.caption,
                [r.x + (r.w - width) * 0.5, r.y + 24. * z],
                small,
                theme.muted,
            );
        }
        if selected || r.contains(self.pointer[0], self.pointer[1]) {
            let handle = 5. * z.max(0.6);
            let at = [r.x + r.w, r.y + r.h * 0.5];
            let dot = Box2 {
                x: at[0] - handle,
                y: at[1] - handle,
                w: handle * 2.,
                h: handle * 2.,
            };
            fill(list, id, dot, handle, theme.accent);
            fill(
                list,
                id,
                grow(dot, -handle * 0.45),
                handle * 0.55,
                theme.card,
            );
        }
    }

    /// Sticky notes in the warm chart accent.
    fn comments(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        for (key, comment) in &self.graph.editor.comments {
            let position = if let Some(super::interaction::Drag::Comment {
                key: dragged,
                start,
            }) = &self.drag
            {
                if key == dragged {
                    self.drag_point(comment.position, *start)
                } else {
                    comment.position
                }
            } else {
                comment.position
            };
            let p = self.screen(position, rect);
            let note = Box2 {
                x: p[0],
                y: p[1],
                w: 220. * self.camera.zoom,
                h: 42. * self.camera.zoom,
            };
            let warm = theme.chart[2];
            fill(list, 3, note, 6., mix(theme.card, warm, 0.18));
            border(
                list,
                3,
                note,
                6.,
                if self.selection.contains(key) { 2. } else { 1. },
                if self.selection.contains(key) {
                    theme.accent
                } else {
                    alpha(warm, 150)
                },
            );
            self.labels.paint(
                list,
                3,
                &comment.text,
                [p[0] + 10. * self.camera.zoom, p[1] + 12. * self.camera.zoom],
                self.camera.zoom,
                theme.foreground,
            );
        }
    }

    /// Connection and box-selection previews.
    fn gesture(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        if let Some(interaction::Drag::Connect { endpoint, .. }) = &self.drag
            && let Some(a) = self.port_position(endpoint, rect)
        {
            let color = if self.diagnostic.is_some() {
                theme.error
            } else {
                theme.accent
            };
            let state = self
                .index
                .get(&endpoint.node)
                .is_some_and(|i| self.nodes[*i].look.shape == Shape::State);
            if state {
                line(list, 4, a, self.pointer, 1.75, color);
                let d = [self.pointer[0] - a[0], self.pointer[1] - a[1]];
                let length = d[0].hypot(d[1]);
                if length > 8. {
                    head(list, 4, self.pointer, [d[0] / length, d[1] / length], color);
                }
            } else {
                curve(list, 4, a, self.pointer, 2., color);
            }
        }
        if let Some(interaction::Drag::Box { start, .. }) = &self.drag {
            let r = Box2 {
                x: start[0].min(self.pointer[0]),
                y: start[1].min(self.pointer[1]),
                w: (self.pointer[0] - start[0]).abs(),
                h: (self.pointer[1] - start[1]).abs(),
            };
            fill(list, 5, r, 0., alpha(theme.accent, 25));
            border(list, 5, r, 0., 1., theme.accent);
        }
    }

    /// Overview of the whole graph, only when it does not fit the view.
    fn minimap_paint(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        let Some(map) = self.minimap(rect) else {
            return;
        };
        list.commands.push(Draw::PushClip { rect: map });
        fill(list, 6, map, 6., alpha(theme.card, 235));
        border(list, 6, map, 6., 1., theme.separator);
        let bounds = self.bounds();
        let scale = ((map.w - 16.) / bounds.w).min((map.h - 16.) / bounds.h);
        let point = |p: [f32; 2]| {
            [
                map.x + 8. + (p[0] - bounds.x) * scale,
                map.y + 8. + (p[1] - bounds.y) * scale,
            ]
        };
        for node in &self.nodes {
            let p = point(node.position);
            fill(
                list,
                6,
                Box2 {
                    x: p[0],
                    y: p[1],
                    w: (node.width * scale).max(2.),
                    h: (node.height * scale).max(2.),
                },
                1.,
                if self.selection.contains(&node.id) {
                    theme.accent
                } else {
                    alpha(theme.accent(node.look.accent), 170)
                },
            );
        }
        let a = self.world([rect.x, rect.y], rect);
        let p = point(a);
        border(
            list,
            6,
            Box2 {
                x: p[0],
                y: p[1],
                w: rect.w / self.camera.zoom * scale,
                h: rect.h / self.camera.zoom * scale,
            }
            .intersection(map),
            1.,
            1.,
            theme.accent,
        );
        list.commands.push(Draw::PopClip);
    }

    /// Graph-space extent of every node, including the Entry marker left of the
    /// default state, for framing and the minimap.
    pub(super) fn bounds(&self) -> Box2 {
        self.nodes
            .iter()
            .map(|n| {
                let entry = if n.look.initial { 96. } else { 0. };
                Box2 {
                    x: n.position[0] - entry,
                    y: n.position[1],
                    w: n.width + entry,
                    h: n.height,
                }
            })
            .reduce(union)
            .unwrap_or(Box2 {
                x: 0.,
                y: 0.,
                w: 1000.,
                h: 600.,
            })
    }
}

fn union(a: Box2, b: Box2) -> Box2 {
    Box2 {
        x: a.x.min(b.x),
        y: a.y.min(b.y),
        w: (a.x + a.w).max(b.x + b.w) - a.x.min(b.x),
        h: (a.y + a.h).max(b.y + b.h) - a.y.min(b.y),
    }
}
fn centre(r: Box2) -> [f32; 2] {
    [r.x + r.w * 0.5, r.y + r.h * 0.5]
}
fn grow(r: Box2, by: f32) -> Box2 {
    Box2 {
        x: r.x - by,
        y: r.y - by,
        w: r.w + by * 2.,
        h: r.h + by * 2.,
    }
}
fn offset(r: Box2, x: f32, y: f32) -> Box2 {
    Box2 {
        x: r.x + x,
        y: r.y + y,
        ..r
    }
}
/// Where a ray from `origin` (inside `r`) along `d` leaves the box, pulled back by `gap`.
fn edge_point(r: Box2, origin: [f32; 2], d: [f32; 2], gap: f32) -> [f32; 2] {
    let half = [
        (r.w * 0.5 - (origin[0] - r.x - r.w * 0.5).abs()).max(0.),
        (r.h * 0.5 - (origin[1] - r.y - r.h * 0.5).abs()).max(0.),
    ];
    let tx = if d[0].abs() > 1e-4 {
        half[0] / d[0].abs()
    } else {
        f32::MAX
    };
    let ty = if d[1].abs() > 1e-4 {
        half[1] / d[1].abs()
    } else {
        f32::MAX
    };
    let t = tx.min(ty) + gap;
    [origin[0] + d[0] * t, origin[1] + d[1] * t]
}
fn alpha(mut color: Color, a: u8) -> Color {
    color.3 = a;
    color
}
/// `base` moved `amount` of the way toward `tint` (opaque result).
fn mix(base: Color, tint: Color, amount: f32) -> Color {
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    Color(
        lerp(base.0, tint.0),
        lerp(base.1, tint.1),
        lerp(base.2, tint.2),
        base.3,
    )
}
fn fill(list: &mut DisplayList, node: u64, rect: Box2, radius: f32, color: Color) {
    list.commands.push(Draw::RoundedRect {
        node,
        rect,
        radius,
        color,
    });
}
fn border(list: &mut DisplayList, node: u64, rect: Box2, radius: f32, width: f32, color: Color) {
    list.commands.push(Draw::Border {
        node,
        rect,
        radius,
        width,
        color,
    });
}
fn line(list: &mut DisplayList, node: u64, from: [f32; 2], to: [f32; 2], width: f32, color: Color) {
    list.commands.push(Draw::Line {
        node,
        from,
        to,
        width,
        color,
    });
}
/// A filled arrowhead with its tip at `tip`, pointing along unit `d`.
fn head(list: &mut DisplayList, node: u64, tip: [f32; 2], d: [f32; 2], color: Color) {
    let n = [-d[1], d[0]];
    let base = [tip[0] - d[0] * 9., tip[1] - d[1] * 9.];
    for step in 0..=6 {
        let s = step as f32 / 3. - 1.;
        let at = [base[0] + n[0] * 4.5 * s, base[1] + n[1] * 4.5 * s];
        line(list, node, at, tip, 1.5, color);
    }
}
fn curve(list: &mut DisplayList, node: u64, a: [f32; 2], b: [f32; 2], width: f32, color: Color) {
    let bend = ((b[0] - a[0]).abs() * 0.45).max(48.);
    bezier(
        list,
        node,
        [a, [a[0] + bend, a[1]], [b[0] - bend, b[1]], b],
        width,
        color,
    );
}
fn bezier(list: &mut DisplayList, node: u64, p: [[f32; 2]; 4], width: f32, color: Color) {
    let mut previous = p[0];
    for step in 1..=20 {
        let t = step as f32 / 20.;
        let u = 1. - t;
        let point = [
            u * u * u * p[0][0]
                + 3. * u * u * t * p[1][0]
                + 3. * u * t * t * p[2][0]
                + t * t * t * p[3][0],
            u * u * u * p[0][1]
                + 3. * u * u * t * p[1][1]
                + 3. * u * t * t * p[2][1]
                + t * t * t * p[3][1],
        ];
        line(list, node, previous, point, width, color);
        previous = point;
    }
}
