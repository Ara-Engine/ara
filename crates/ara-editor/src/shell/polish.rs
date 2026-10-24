//! Bounded virtual-row decorations. Model size never affects paint work.
use super::*;
use ara_ui::{paint::Draw, source::stable_id, style::Color};

pub(super) struct RowStyle {
    pub depth: usize,
    pub changed: bool,
    pub icon: &'static str,
    /// Icon tint class (`tint-light`, …); empty is muted.
    pub tint: &'static str,
    pub children: bool,
    pub collapsed: bool,
    pub locked: bool,
    pub hidden: bool,
}
/// Hover gutter: the eye sits 44 px and the lock 24 px from the row's right edge.
pub(super) const EYE: f32 = 44.;
pub(super) const LOCK: f32 = 24.;
impl Shell {
    pub(super) fn paint_polish(&mut self) {
        let color = |name: &str| self.ui.sheet.color_token(name).unwrap_or(Color(0, 0, 0, 0));
        let primary = color("--primary");
        let muted = color("--muted-foreground");
        let on_selection = color("--selection-foreground");
        let mut draws = Vec::new();
        for node in &self.ui.nodes {
            if !node.visible || !node.rect.intersects(node.clip) {
                continue;
            }
            let class = node
                .element
                .attrs
                .get("class")
                .map(String::as_str)
                .unwrap_or("");
            if class.contains("tab-selected") {
                let r = node.rect;
                draws.push(Draw::Rect {
                    node: stable_id(&format!("{}-underline", node.element.id)),
                    rect: Box2 {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: 2.,
                    },
                    color: primary,
                });
            }
        }
        if self.hierarchy_surface == self.current
            && let Some(view) = &self.hierarchy_view
        {
            let tint = |name: &str| match name {
                "tint-light" => color("--warning-foreground"),
                "tint-camera" => color("--chart-4"),
                "tint-vfx" => color("--chart-5"),
                "tint-audio" => color("--chart-2"),
                "tint-prefab" => color("--info-foreground"),
                _ => muted,
            };
            for index in view.visible_nodes() {
                let n = &self.ui.nodes[*index];
                let Some(key) = n
                    .element
                    .id
                    .rsplit_once("key-")
                    .and_then(|(_, s)| s.parse::<u64>().ok())
                else {
                    continue;
                };
                let r = n.rect;
                let id = stable_id(&n.element.id);
                let selected = self.selected_rows.contains(&key);
                let hovered = r.contains(self.pointer[0], self.pointer[1]);
                let icon_y = r.y + (r.h - 16.) * 0.5;
                draws.push(Draw::PushClip {
                    rect: n.clip.intersection(r),
                });
                if let Some(s) = self.hierarchy_style.get(&key) {
                    let x = r.x + 6. + s.depth.min(32) as f32 * 16.;
                    let base = if selected { on_selection } else { muted };
                    if s.children {
                        image(
                            &mut draws,
                            id,
                            Box2 {
                                x,
                                y: icon_y,
                                w: 16.,
                                h: 16.,
                            },
                            if s.collapsed {
                                "chevron-right"
                            } else {
                                "chevron-down"
                            },
                            base,
                        );
                    }
                    image(
                        &mut draws,
                        id ^ 0x1,
                        Box2 {
                            x: x + 18.,
                            y: icon_y,
                            w: 16.,
                            h: 16.,
                        },
                        s.icon,
                        if selected { on_selection } else { tint(s.tint) },
                    );
                    if s.changed {
                        draws.push(Draw::RoundedRect {
                            node: id ^ 0xffff_1111_0000,
                            rect: Box2 {
                                x: r.x + r.w - 58.,
                                y: r.y + r.h * 0.5 - 3.,
                                w: 6.,
                                h: 6.,
                            },
                            radius: 3.,
                            color: color(&format!(
                                "--{}",
                                self.hierarchy_tints
                                    .get(&key)
                                    .copied()
                                    .unwrap_or("warning-foreground")
                            )),
                        });
                    }
                    // Visibility and lock: always shown when set, otherwise on hover only.
                    if s.hidden || hovered {
                        image(
                            &mut draws,
                            id ^ 0x2,
                            Box2 {
                                x: r.x + r.w - EYE,
                                y: icon_y,
                                w: 16.,
                                h: 16.,
                            },
                            if s.hidden { "eye-off" } else { "eye" },
                            base,
                        );
                    }
                    if s.locked || hovered {
                        image(
                            &mut draws,
                            id ^ 0x3,
                            Box2 {
                                x: r.x + r.w - LOCK,
                                y: icon_y,
                                w: 16.,
                                h: 16.,
                            },
                            if s.locked { "lock" } else { "lock-open" },
                            base,
                        );
                    }
                }
                draws.push(Draw::PopClip);
            }
        }
        if self.asset_surface == self.current
            && let Some(view) = &self.asset_view
        {
            for index in view.visible_nodes() {
                let n = &self.ui.nodes[*index];
                let Some(key) = n
                    .element
                    .id
                    .rsplit_once("key-")
                    .and_then(|(_, s)| s.parse::<u64>().ok())
                else {
                    continue;
                };
                if let Some(a) = self.asset_entries.get(&key) {
                    draws.push(Draw::PushClip {
                        rect: n.clip.intersection(n.rect),
                    });
                    image(
                        &mut draws,
                        stable_id(&n.element.id),
                        Box2 {
                            x: n.rect.x + 8.,
                            y: n.rect.y + (n.rect.h - 16.) * 0.5,
                            w: 16.,
                            h: 16.,
                        },
                        assets_panel::asset_icon(a),
                        muted,
                    );
                    draws.push(Draw::PopClip);
                }
            }
        }
        self.ui.display.commands.extend(draws);
    }
}
fn image(draws: &mut Vec<Draw>, node: u64, rect: Box2, name: &str, tint: Color) {
    draws.push(Draw::Image {
        node,
        rect,
        image: stable_id(&format!("icon-{name}")) as u32 & 0x7fff_ffff,
        tint,
    });
}

impl Shell {
    pub(super) fn paint_swatches(&mut self) {
        for (id, color) in &self.value_swatches {
            if let Some(i) = self.ui.find(id) {
                let r = self.ui.nodes[i].rect;
                let clip = self.ui.nodes[i].clip.intersection(r);
                if !r.intersects(clip) || clip.w <= 0. || clip.h <= 0. {
                    continue;
                }
                let display = &mut self.ui.display.commands;
                display.push(ara_ui::paint::Draw::PushClip { rect: clip });
                display.push(ara_ui::paint::Draw::RoundedRect {
                    node: ara_ui::source::stable_id(id),
                    rect: Box2 {
                        x: r.x + 2.,
                        y: r.y + 2.,
                        w: (r.w - 4.).max(0.),
                        h: (r.h - 4.).max(0.),
                    },
                    radius: 3.,
                    color: *color,
                });
                display.push(ara_ui::paint::Draw::PopClip);
            }
        }
        for field in self.fields.values() {
            if !matches!(
                field.widget,
                crate::inspector::Widget::Vector { color: true, .. }
            ) || field.mixed
            {
                continue;
            }
            if let Some(i) = self
                .ui
                .find(&format!("swatch-{}", field.path.replace('.', "-")))
            {
                let r = self.ui.nodes[i].rect;
                let clip = self.ui.nodes[i].clip.intersection(r);
                if !r.intersects(clip) || clip.w <= 0. || clip.h <= 0. {
                    continue;
                }
                self.ui
                    .display
                    .commands
                    .push(ara_ui::paint::Draw::PushClip { rect: clip });
                let channel = |n: usize| field.value[n].as_f64().unwrap_or(1.).clamp(0., 1.);
                self.ui
                    .display
                    .commands
                    .push(ara_ui::paint::Draw::RoundedRect {
                        node: ara_ui::source::stable_id(&self.ui.nodes[i].element.id),
                        rect: Box2 {
                            x: r.x + 2.,
                            y: r.y + 2.,
                            w: (r.w - 4.).max(0.),
                            h: (r.h - 4.).max(0.),
                        },
                        radius: 3.,
                        color: Color(
                            (channel(0).powf(1. / 2.2) * 255.) as u8,
                            (channel(1).powf(1. / 2.2) * 255.) as u8,
                            (channel(2).powf(1. / 2.2) * 255.) as u8,
                            (channel(3) * 255.) as u8,
                        ),
                    });
                self.ui.display.commands.push(ara_ui::paint::Draw::PopClip);
            }
        }
    }
}
