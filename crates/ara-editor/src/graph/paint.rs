use super::*;
use ara_ui::{paint::Draw, source::stable_id};
impl Canvas {
    pub(super) fn paint_impl(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        list.commands.push(Draw::PushClip { rect });
        fill(list, 0, rect, 0., theme.background);
        let step = 32. * self.camera.zoom;
        let offset = [
            self.camera.pan[0].rem_euclid(step),
            self.camera.pan[1].rem_euclid(step),
        ];
        let mut grid = theme.border;
        grid.3 = 100;
        for x in 0..(rect.w / step).ceil() as usize {
            for y in 0..(rect.h / step).ceil() as usize {
                fill(
                    list,
                    1,
                    Box2 {
                        x: rect.x + offset[0] + x as f32 * step,
                        y: rect.y + offset[1] + y as f32 * step,
                        w: 1.5,
                        h: 1.5,
                    },
                    0.75,
                    grid,
                );
            }
        }
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
                let mut color = theme.accent;
                color.3 = 15;
                fill(list, 2, r, 10., color);
                border(list, 2, r, 10., 1., theme.border);
                self.labels.paint(
                    list,
                    2,
                    &group.label,
                    [r.x + 12., r.y + 8.],
                    1.,
                    theme.muted,
                );
            }
        }
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
            let color = port_color(ty);
            let id = stable_id(&wire.id);
            let mut previous = a;
            for p in &wire.points {
                let p = self.screen(*p, rect);
                curve(list, id, previous, p, 2., color);
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
            curve(list, id, previous, b, 2., color);
        }
        for node in &self.nodes {
            let r = self.node_rect(node, rect);
            if !r.intersects(rect) {
                continue;
            }
            let id = stable_id(&node.id);
            let selected = self.selection.contains(&node.id);
            fill(
                list,
                id,
                Box2 {
                    x: r.x + 2.,
                    y: r.y + 4.,
                    ..r
                },
                8. * self.camera.zoom,
                Color(0, 0, 0, 40),
            );
            fill(list, id, r, 8. * self.camera.zoom, theme.card);
            border(
                list,
                id,
                r,
                8. * self.camera.zoom,
                if selected { 2. } else { 1. },
                if selected { theme.accent } else { theme.border },
            );
            self.labels.paint(
                list,
                id,
                &node.title,
                [r.x + 14. * self.camera.zoom, r.y + 9. * self.camera.zoom],
                self.camera.zoom,
                theme.foreground,
            );
            self.labels.paint(
                list,
                id,
                &node.subtitle,
                [r.x + 14. * self.camera.zoom, r.y + 29. * self.camera.zoom],
                self.camera.zoom * 0.82,
                theme.muted,
            );
            line(
                list,
                id,
                [r.x, r.y + 53. * self.camera.zoom],
                [r.x + r.w, r.y + 53. * self.camera.zoom],
                1.,
                theme.border,
            );
            for (name, direction, ty) in &node.ports {
                let endpoint = Endpoint {
                    node: node.id.clone(),
                    port: name.clone(),
                };
                if let Some(p) = self.port_position(&endpoint, rect) {
                    let color = port_color(ty);
                    let radius = 4. * self.camera.zoom;
                    fill(
                        list,
                        id,
                        Box2 {
                            x: p[0] - radius,
                            y: p[1] - radius,
                            w: 2. * radius,
                            h: 2. * radius,
                        },
                        radius,
                        color,
                    );
                    let label = if *direction == Direction::Input {
                        [p[0] + 12. * self.camera.zoom, p[1] - 9. * self.camera.zoom]
                    } else {
                        [p[0] - 64. * self.camera.zoom, p[1] - 9. * self.camera.zoom]
                    };
                    self.labels
                        .paint(list, id, name, label, self.camera.zoom, theme.muted);
                }
            }
        }
        for comment in self.graph.editor.comments.values() {
            let p = self.screen(comment.position, rect);
            let mut color = theme.accent;
            color.3 = 24;
            fill(
                list,
                3,
                Box2 {
                    x: p[0],
                    y: p[1],
                    w: 220. * self.camera.zoom,
                    h: 42. * self.camera.zoom,
                },
                6.,
                color,
            );
            self.labels.paint(
                list,
                3,
                &comment.text,
                [p[0] + 10., p[1] + 10.],
                self.camera.zoom,
                theme.foreground,
            );
        }
        if let Some(interaction::Drag::Connect { endpoint, .. }) = &self.drag
            && let Some(a) = self.port_position(endpoint, rect)
        {
            curve(
                list,
                4,
                a,
                self.pointer,
                2.,
                if self.diagnostic.is_some() {
                    theme.error
                } else {
                    theme.accent
                },
            );
        }
        if let Some(interaction::Drag::Box { start, .. }) = &self.drag {
            let r = Box2 {
                x: start[0].min(self.pointer[0]),
                y: start[1].min(self.pointer[1]),
                w: (self.pointer[0] - start[0]).abs(),
                h: (self.pointer[1] - start[1]).abs(),
            };
            let mut color = theme.accent;
            color.3 = 25;
            fill(list, 5, r, 0., color);
            border(list, 5, r, 0., 1., theme.accent);
        }
        self.minimap(list, rect, theme);
        list.commands.push(Draw::PopClip);
    }
    fn minimap(&self, list: &mut DisplayList, rect: Box2, theme: Theme) {
        let map = Box2 {
            x: rect.x + rect.w - 150.,
            y: rect.y + rect.h - 104.,
            w: 138.,
            h: 92.,
        };
        fill(list, 6, map, 6., theme.card);
        border(list, 6, map, 6., 1., theme.border);
        let bounds = self.bounds();
        let scale = (120. / bounds.w).min(74. / bounds.h);
        let point = |p: [f32; 2]| {
            [
                map.x + 9. + (p[0] - bounds.x) * scale,
                map.y + 9. + (p[1] - bounds.y) * scale,
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
                    w: (216. * scale).max(2.),
                    h: (node.height * scale).max(2.),
                },
                1.,
                if self.selection.contains(&node.id) {
                    theme.accent
                } else {
                    theme.muted
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
            },
            1.,
            1.,
            theme.accent,
        );
    }
    pub(super) fn bounds(&self) -> Box2 {
        self.nodes
            .iter()
            .map(|n| Box2 {
                x: n.position[0],
                y: n.position[1],
                w: 216.,
                h: n.height,
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
fn curve(list: &mut DisplayList, node: u64, a: [f32; 2], b: [f32; 2], width: f32, color: Color) {
    let bend = ((b[0] - a[0]).abs() * 0.45).max(48.);
    let c = [a[0] + bend, a[1]];
    let d = [b[0] - bend, b[1]];
    let mut previous = a;
    for step in 1..=20 {
        let t = step as f32 / 20.;
        let u = 1. - t;
        let point = [
            u * u * u * a[0] + 3. * u * u * t * c[0] + 3. * u * t * t * d[0] + t * t * t * b[0],
            u * u * u * a[1] + 3. * u * u * t * c[1] + 3. * u * t * t * d[1] + t * t * t * b[1],
        ];
        line(list, node, previous, point, width, color);
        previous = point;
    }
}
