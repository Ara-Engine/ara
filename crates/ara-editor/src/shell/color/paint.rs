//! Painting the picker on the GPU: the hue ring and saturation/value square, gradient
//! slider tracks, checkerboards behind translucent colours, swatches and the markers
//! (all [`Draw::Fill`] and rounded rects), and the eyedropper's loupe.
//!
//! Each part's draws are spliced in right after its own box paints, so its children
//! (button labels) and anything stacked above the picker (menus, tooltips) stay on top.
//! Painted parts carry no CSS background or border; their outlines are drawn here.
use super::*;
use ara_ui::paint::{Draw, Fill};
use ara_ui::style::Color;
use std::f32::consts::TAU;

/// The checkerboard behind translucent colours.
const CHECKER: Fill = Fill::Checker {
    cell: 4.,
    light: Color(204, 204, 204, 255),
    dark: Color(150, 150, 150, 255),
};
const OUTLINE: Color = Color(0, 0, 0, 90);
const WHITE: Color = Color(255, 255, 255, 255);
const SHADOW: Color = Color(0, 0, 0, 150);
/// Segments the Kelvin track's curved gradient is drawn in.
const KELVIN_SEGMENTS: usize = 24;

/// Display bytes of a display-space (sRGB, 0–1) colour.
fn display(rgb: [f64; 3], alpha: f64) -> Color {
    let byte = |c: f64| (c.clamp(0., 1.) * 255.).round() as u8;
    Color(byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), byte(alpha))
}
/// Display bytes of a linear colour, clipped at 1.
fn linear(rgb: [f64; 3], alpha: f64) -> Color {
    display(rgb.map(|c| math::linear_to_srgb(c.clamp(0., 1.))), alpha)
}
fn inset(r: Box2, d: f32) -> Box2 {
    Box2 {
        x: r.x + d,
        y: r.y + d,
        w: (r.w - 2. * d).max(0.),
        h: (r.h - 2. * d).max(0.),
    }
}
/// Paints a part's draws for one node.
struct Painter {
    node: u64,
    draws: Vec<Draw>,
}
impl Painter {
    fn fill(&mut self, rect: Box2, radius: f32, fill: Fill) {
        self.draws.push(Draw::Fill {
            node: self.node,
            rect,
            radius,
            fill,
        });
    }
    fn solid(&mut self, rect: Box2, radius: f32, color: Color) {
        self.draws.push(Draw::RoundedRect {
            node: self.node,
            rect,
            radius,
            color,
        });
    }
    fn outline(&mut self, rect: Box2, radius: f32, width: f32, color: Color) {
        self.draws.push(Draw::Border {
            node: self.node,
            rect,
            radius,
            width,
            color,
        });
    }
    /// A round marker: `fill` inside a white ring with a dark edge, readable on any colour.
    fn marker(&mut self, centre: [f32; 2], radius: f32, fill: Color) {
        let disc = Box2 {
            x: centre[0] - radius,
            y: centre[1] - radius,
            w: radius * 2.,
            h: radius * 2.,
        };
        self.solid(inset(disc, -1.), radius + 1., SHADOW);
        self.solid(disc, radius, WHITE);
        self.solid(inset(disc, 2.), radius - 2., fill);
    }
    /// A slider thumb at `t` along a track.
    fn thumb(&mut self, track: Box2, t: f64) {
        let x = track.x + track.w * (t.clamp(0., 1.) as f32);
        let rect = Box2 {
            x: (x - 3.).round(),
            y: track.y - 3.,
            w: 6.,
            h: track.h + 6.,
        };
        self.solid(inset(rect, -1.), 3., SHADOW);
        self.solid(rect, 2., WHITE);
    }
    /// A colour over a checkerboard: opaque in the left half, with its alpha in the right.
    fn swatch(&mut self, rect: Box2, radius: f32, color: Color) {
        if color.3 < 255 {
            self.fill(rect, radius, CHECKER);
            let half = (rect.w / 2.).round();
            self.draws.push(Draw::PushClip {
                rect: Box2 { w: half, ..rect },
            });
            self.solid(rect, radius, Color(color.0, color.1, color.2, 255));
            self.draws.push(Draw::PopClip);
            self.draws.push(Draw::PushClip {
                rect: Box2 {
                    x: rect.x + half,
                    w: rect.w - half,
                    ..rect
                },
            });
            self.solid(rect, radius, color);
            self.draws.push(Draw::PopClip);
        } else {
            self.solid(rect, radius, color);
        }
        self.outline(rect, radius, 1., OUTLINE);
    }
}

impl Shell {
    /// Splice the picker's draws in after each painted part's own box. Runs first in
    /// the decoration pass, while `paint_starts` still index the UI's own commands.
    pub(in crate::shell) fn paint_color_picker(&mut self) {
        let Some(picker) = &self.color_picker else {
            return;
        };
        if self.ui.find("color-picker").is_none() {
            return;
        }
        let hovered = self
            .ui
            .hit_test(self.pointer[0], self.pointer[1])
            .map(|i| self.ui.nodes[i].element.id.clone());
        let mut parts: Vec<(usize, Vec<Draw>)> = Vec::new();
        let ui = &self.ui;
        let mut part = |id: &str, paint: &mut dyn FnMut(&mut Painter, Box2)| {
            let Some(i) = ui.find(id).filter(|i| ui.shown(*i)) else {
                return;
            };
            let Some(k) = ui.paint_order.iter().position(|n| *n == i) else {
                return;
            };
            let mut painter = Painter {
                node: ara_ui::source::stable_id(id),
                draws: vec![Draw::PushClip {
                    rect: ui.nodes[i].clip,
                }],
            };
            paint(&mut painter, ui.nodes[i].rect);
            let clickable = ["cp-old", "cp-swatch-", "cp-recent-", "cp-exposure-", "cp-preset-"]
                .iter()
                .any(|prefix| id.starts_with(prefix));
            if clickable && hovered.as_deref() == Some(id) {
                painter.outline(inset(ui.nodes[i].rect, -1.), 4., 1.5, WHITE);
            }
            painter.draws.push(Draw::PopClip);
            let at = ui
                .paint_starts
                .get(k + 1)
                .copied()
                .unwrap_or(ui.display.commands.len());
            parts.push((at, painter.draws));
        };
        let hue = display(math::hsv_to_rgb([picker.hsv[0], 1., 1.]), 1.);
        let current = display(math::hsv_to_rgb(picker.hsv), 1.);
        let value = picker.value();
        let alpha = if picker.count > 3 { value[3] } else { 1. };
        // The ring, the square and their markers.
        part("cp-wheel", &mut |p, r| {
            p.fill(r, 0., Fill::Hue { ring: Some(RING) });
            let s = square(r);
            p.fill(
                s,
                2.,
                Fill::Linear {
                    from: WHITE,
                    to: hue,
                    vertical: false,
                    linear_light: false,
                },
            );
            p.fill(
                s,
                2.,
                Fill::Linear {
                    from: Color(0, 0, 0, 0),
                    to: Color(0, 0, 0, 255),
                    vertical: true,
                    linear_light: false,
                },
            );
            p.outline(s, 2., 1., OUTLINE);
            let angle = picker.hsv[0] as f32 * TAU;
            let radius = r.w.min(r.h) / 2. - RING / 2.;
            let centre = [r.x + r.w / 2., r.y + r.h / 2.];
            p.marker(
                [centre[0] + angle.cos() * radius, centre[1] - angle.sin() * radius],
                RING / 2. + 1.,
                hue,
            );
            p.marker(
                [
                    s.x + s.w * picker.hsv[1] as f32,
                    s.y + s.h * (1. - picker.hsv[2] as f32),
                ],
                6.,
                current,
            );
        });
        // New and Old.
        part("cp-new", &mut |p, r| {
            p.swatch(r, 4., linear([value[0], value[1], value[2]], alpha));
        });
        let old = picker.old;
        part("cp-old", &mut |p, r| {
            p.swatch(r, 4., linear([old[0], old[1], old[2]], if picker.count > 3 { old[3] } else { 1. }));
        });
        // Channel tracks.
        let base_display = math::hsv_to_rgb(picker.hsv);
        let base_linear = picker.base();
        for i in 0..picker.count {
            let id = format!("cp-track-{i}");
            part(&id, &mut |p, r| {
                let t = picker.channel(i);
                let with = |rgb: [f64; 3], c: f64| {
                    let mut rgb = rgb;
                    rgb[i] = c;
                    rgb
                };
                let fill = match (picker.mode, i) {
                    (_, 3) => {
                        p.fill(r, 3., CHECKER);
                        Fill::Linear {
                            from: display(base_display, 0.),
                            to: display(base_display, 1.),
                            vertical: false,
                            linear_light: false,
                        }
                    }
                    (Mode::Hsv, 0) => Fill::Hue { ring: None },
                    (Mode::Hsv, _) => Fill::Linear {
                        from: display(math::hsv_to_rgb(with(picker.hsv, 0.)), 1.),
                        to: display(math::hsv_to_rgb(with(picker.hsv, 1.)), 1.),
                        vertical: false,
                        linear_light: false,
                    },
                    (Mode::Rgb, _) => Fill::Linear {
                        from: display(with(base_display, 0.), 1.),
                        to: display(with(base_display, 1.), 1.),
                        vertical: false,
                        linear_light: false,
                    },
                    (Mode::Linear, _) => Fill::Linear {
                        from: linear(with(base_linear, 0.), 1.),
                        to: linear(with(base_linear, 1.), 1.),
                        vertical: false,
                        linear_light: true,
                    },
                };
                p.fill(r, 3., fill);
                p.outline(r, 3., 1., OUTLINE);
                p.thumb(r, t);
            });
        }
        // HDR intensity: dark through the base colour to over-exposed white.
        if picker.hints.hdr {
            let base = display(base_display, 1.);
            part("cp-stops-track", &mut |p, r| {
                let half = (r.w / 2.).round();
                p.draws.push(Draw::PushRoundedClip { rect: r, radius: 3. });
                p.fill(
                    Box2 { w: half, ..r },
                    0.,
                    Fill::Linear {
                        from: Color(0, 0, 0, 255),
                        to: base,
                        vertical: false,
                        linear_light: false,
                    },
                );
                p.fill(
                    Box2 {
                        x: r.x + half,
                        w: r.w - half,
                        ..r
                    },
                    0.,
                    Fill::Linear {
                        from: base,
                        to: WHITE,
                        vertical: false,
                        linear_light: false,
                    },
                );
                p.draws.push(Draw::PopClip);
                p.outline(r, 3., 1., OUTLINE);
                p.thumb(r, (picker.stops - STOPS.0) / (STOPS.1 - STOPS.0));
            });
            for (n, step) in (-2i8..=2).enumerate() {
                let scale = (picker.stops + f64::from(step)).exp2();
                let color = linear(base_linear.map(|c| c * scale), 1.);
                part(&format!("cp-exposure-{n}"), &mut |p, r| {
                    p.solid(r, 4., color);
                    p.outline(r, 4., if step == 0 { 2. } else { 1. }, if step == 0 { WHITE } else { OUTLINE });
                });
            }
        }
        // Colour temperature: the blackbody curve, and the presets in their colours.
        if picker.hints.temperature {
            let (low, high) = math::KELVIN;
            part("cp-kelvin-track", &mut |p, r| {
                p.draws.push(Draw::PushRoundedClip { rect: r, radius: 3. });
                let at = |n: usize| low + (high - low) * n as f64 / KELVIN_SEGMENTS as f64;
                for n in 0..KELVIN_SEGMENTS {
                    let x0 = r.x + r.w * n as f32 / KELVIN_SEGMENTS as f32;
                    let x1 = r.x + r.w * (n + 1) as f32 / KELVIN_SEGMENTS as f32;
                    p.fill(
                        Box2 {
                            x: x0.floor(),
                            w: (x1.ceil() - x0.floor()).max(1.),
                            ..r
                        },
                        0.,
                        Fill::Linear {
                            from: linear(math::kelvin_to_linear(at(n)), 1.),
                            to: linear(math::kelvin_to_linear(at(n + 1)), 1.),
                            vertical: false,
                            linear_light: true,
                        },
                    );
                }
                p.draws.push(Draw::PopClip);
                p.outline(r, 3., 1., OUTLINE);
                if let Some(kelvin) = picker.kelvin {
                    p.thumb(r, (kelvin - low) / (high - low));
                }
            });
            for (n, (_, kelvin)) in math::KELVIN_PRESETS.iter().enumerate() {
                let current = picker.kelvin == Some(*kelvin);
                part(&format!("cp-preset-{n}"), &mut |p, r| {
                    p.solid(r, 4., linear(math::kelvin_to_linear(*kelvin), 1.));
                    p.outline(r, 4., if current { 2. } else { 1. }, if current { WHITE } else { OUTLINE });
                });
            }
        }
        // Project swatches and recent colours.
        for (prefix, list) in [("swatch", &picker.swatches), ("recent", &picker.recent)] {
            for (n, c) in list.iter().enumerate() {
                let color = linear([c[0], c[1], c[2]], c[3]);
                part(&format!("cp-{prefix}-{n}"), &mut |p, r| p.swatch(r, 3., color));
            }
        }
        drop(part);
        // Later indices first, so earlier ones stay valid.
        parts.sort_by(|a, b| b.0.cmp(&a.0));
        let commands = &mut self.ui.display.commands;
        for (at, draws) in parts {
            let at = at.min(commands.len());
            commands.splice(at..at, draws);
        }
    }
    /// The eyedropper's loupe: the pixels around the sample, magnified beside the
    /// pointer, on top of everything. Runs last in the decoration pass.
    pub(in crate::shell) fn paint_loupe(&mut self) {
        let Some((pixels, at)) = self
            .color_picker
            .as_ref()
            .and_then(|p| p.eyedropper.as_ref())
            .and_then(|e| e.loupe.as_ref())
        else {
            return;
        };
        const CELL: f32 = 8.;
        let side = eyedropper::LOUPE as f32 * CELL;
        let mut x = at[0] + 22.;
        let mut y = at[1] + 22.;
        if x + side + 4. > self.ui.size[0] {
            x = at[0] - 22. - side;
        }
        if y + side + 4. > self.ui.size[1] {
            y = at[1] - 22. - side;
        }
        let frame = Box2 {
            x,
            y,
            w: side,
            h: side,
        };
        let mut p = Painter {
            node: ara_ui::source::stable_id("cp-loupe"),
            draws: Vec::new(),
        };
        p.solid(inset(frame, -3.), 7., Color(0, 0, 0, 220));
        p.draws.push(Draw::PushRoundedClip {
            rect: frame,
            radius: 5.,
        });
        let n = eyedropper::LOUPE as usize;
        for (k, pixel) in pixels.iter().enumerate() {
            let [r, g, b, _] = pixel.unwrap_or([24, 24, 24, 255]);
            p.draws.push(Draw::Rect {
                node: p.node,
                rect: Box2 {
                    x: x + (k % n) as f32 * CELL,
                    y: y + (k / n) as f32 * CELL,
                    w: CELL,
                    h: CELL,
                },
                color: Color(r, g, b, 255),
            });
        }
        p.draws.push(Draw::PopClip);
        let centre = Box2 {
            x: x + (n / 2) as f32 * CELL,
            y: y + (n / 2) as f32 * CELL,
            w: CELL,
            h: CELL,
        };
        p.outline(inset(centre, -2.), 1., 1., Color(0, 0, 0, 255));
        p.outline(inset(centre, -1.), 1., 1.5, WHITE);
        p.outline(frame, 5., 1., Color(255, 255, 255, 60));
        self.ui.display.commands.extend(p.draws);
    }
}
