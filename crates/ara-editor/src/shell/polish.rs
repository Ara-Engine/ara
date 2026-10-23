//! Bounded virtual-row decorations. Model size never affects paint work.
use super::*;
use ara_ui::{paint::Draw, source::stable_id};

pub(super) struct RowStyle {
    pub depth: usize,
    pub prefab: bool,
    pub changed: bool,
    pub icon: &'static str,
    pub children: bool,
    pub collapsed: bool,
    pub locked: bool,
    pub hidden: bool,
}
impl Shell {
    pub(super) fn paint_polish(&mut self) {
        let color = |name: &str| {
            self.ui
                .sheet
                .color_token(name)
                .unwrap_or(ara_ui::style::Color(0, 0, 0, 0))
        };
        let primary = color("--primary");
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
                        x: r.x + 8.,
                        y: r.y + r.h - 2.,
                        w: (r.w - 16.).max(0.),
                        h: 2.,
                    },
                    color: primary,
                });
            }
        }
        if let Some(view) = &self.hierarchy_view {
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
                draws.push(Draw::PushClip {
                    rect: n.clip.intersection(r),
                });
                if self.selected_rows.contains(&key) {
                    draws.push(Draw::RoundedRect {
                        node: id ^ 0xfffe_0000_0000,
                        rect: Box2 {
                            x: r.x + 1.,
                            y: r.y + 4.,
                            w: 2.,
                            h: r.h - 8.,
                        },
                        radius: 1.,
                        color: primary,
                    });
                }
                if let Some(s) = self.hierarchy_style.get(&key) {
                    let x = r.x + 8. + s.depth.min(32) as f32 * 14.;
                    if s.children {
                        image(
                            &mut draws,
                            id,
                            Box2 {
                                x,
                                y: r.y + 5.,
                                w: 16.,
                                h: 16.,
                            },
                            if s.collapsed {
                                "chevron-right"
                            } else {
                                "chevron-down"
                            },
                        );
                    }
                    image(
                        &mut draws,
                        id,
                        Box2 {
                            x: x + 16.,
                            y: r.y + 5.,
                            w: 16.,
                            h: 16.,
                        },
                        s.icon,
                    );
                    if s.prefab {
                        image(
                            &mut draws,
                            id,
                            Box2 {
                                x: r.x + r.w - 54.,
                                y: r.y + 5.,
                                w: 16.,
                                h: 16.,
                            },
                            "package",
                        );
                    }
                    if s.changed {
                        draws.push(Draw::RoundedRect {
                            node: id ^ 0xffff_1111_0000,
                            rect: Box2 {
                                x: r.x + r.w - 10.,
                                y: r.y + 10.,
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
                    if s.locked || s.hidden {
                        image(
                            &mut draws,
                            id,
                            Box2 {
                                x: r.x + r.w - 32.,
                                y: r.y + 5.,
                                w: 16.,
                                h: 16.,
                            },
                            if s.locked { "lock" } else { "eye-off" },
                        );
                    }
                }
                draws.push(Draw::PopClip);
            }
        }
        if let Some(view) = &self.asset_view {
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
                            y: n.rect.y + 5.,
                            w: 16.,
                            h: 16.,
                        },
                        match a.kind.as_str() {
                            "scene" | "prefab" => "box",
                            "glb" | "gltf" => "package",
                            "png" | "jpg" => "file-image",
                            "cs" | "json" => "file-code",
                            _ => "file",
                        },
                    );
                    draws.push(Draw::PopClip);
                }
            }
        }
        self.ui.display.commands.extend(draws);
    }
}
fn image(draws: &mut Vec<Draw>, node: u64, rect: Box2, name: &str) {
    draws.push(Draw::Image {
        node,
        rect,
        image: stable_id(&format!("icon-{name}")) as u32 & 0x7fff_ffff,
    });
}
