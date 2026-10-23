use ara_ui::{
    Ui,
    paint::{Box2, DisplayList, Draw},
};

fn mirror(r: Box2, width: f32) -> Box2 {
    Box2 {
        x: width - r.x - r.w,
        ..r
    }
}
fn translated(r: Box2, t: [f32; 2]) -> Box2 {
    Box2 {
        x: r.x + t[0],
        y: r.y + t[1],
        ..r
    }
}
fn scaled(r: Box2, scale: f32) -> Box2 {
    Box2 {
        x: r.x * scale,
        y: r.y * scale,
        w: r.w * scale,
        h: r.h * scale,
    }
}

/// Flatten paint translations and mirror layout geometry without reversing glyphs or semantic order.
pub(crate) fn mirror_rtl(ui: &mut Ui) {
    let width = ui.size[0];
    let mut translations = vec![[0.0, 0.0]];
    let mut commands = Vec::with_capacity(ui.display.commands.len());
    for command in ui.display.commands.drain(..) {
        let t = *translations
            .last()
            .expect("a validated display list retains its root transform");
        let rect = |r| mirror(translated(r, t), width);
        let command = match command {
            Draw::PushTransform { translation } => {
                translations.push([t[0] + translation[0], t[1] + translation[1]]);
                continue;
            }
            Draw::PopTransform => {
                translations.pop();
                continue;
            }
            Draw::Rect {
                node,
                rect: r,
                color,
            } => {
                let original = translated(r, t);
                // Carets and selections follow the paragraph's logical glyph coordinates.
                let field = ui.nodes.iter().find(|n| {
                    ara_ui::source::stable_id(&n.element.id) == node
                        && matches!(n.element.tag.as_str(), "field" | "input")
                });
                let r = if let Some(field) =
                    field.filter(|n| original.w < n.rect.w || original.h < n.rect.h)
                {
                    Box2 {
                        x: original.x + width - 2.0 * field.rect.x - field.rect.w,
                        ..original
                    }
                } else {
                    rect(r)
                };
                Draw::Rect {
                    node,
                    rect: r,
                    color,
                }
            }
            Draw::RoundedRect {
                node,
                rect: r,
                color,
                radius,
            } => Draw::RoundedRect {
                node,
                rect: rect(r),
                color,
                radius,
            },
            Draw::Border {
                node,
                rect: r,
                color,
                radius,
                width,
            } => Draw::Border {
                node,
                rect: rect(r),
                color,
                radius,
                width,
            },
            Draw::Image {
                node,
                rect: r,
                image,
            } => Draw::Image {
                node,
                rect: rect(r),
                image,
            },
            Draw::PushClip { rect: r } => Draw::PushClip { rect: rect(r) },
            Draw::PushRoundedClip { rect: r, radius } => Draw::PushRoundedClip {
                rect: rect(r),
                radius,
            },
            Draw::GlyphRun {
                node,
                font,
                size,
                origin,
                glyphs,
                color,
            } => {
                let shift = ui
                    .nodes
                    .iter()
                    .find(|n| ara_ui::source::stable_id(&n.element.id) == node)
                    .map_or(0.0, |n| width - 2.0 * n.rect.x - n.rect.w);
                Draw::GlyphRun {
                    node,
                    font,
                    size,
                    origin: [origin[0] + t[0] + shift, origin[1] + t[1]],
                    glyphs,
                    color,
                }
            }
            other => other,
        };
        commands.push(command);
    }
    ui.display.commands = commands;
    for node in &mut ui.nodes {
        node.rect = mirror(node.rect, width);
        node.clip = mirror(node.clip, width);
    }
}

/// Scale the display-list geometry before rasterization so glyphs use physical-size atlas entries.
pub(crate) fn physical_list(list: &DisplayList, scale: f32) -> DisplayList {
    DisplayList {
        glyphs: list
            .glyphs
            .iter()
            .map(|g| ara_ui::paint::Glyph {
                id: g.id,
                x: g.x * scale,
                y: g.y * scale,
            })
            .collect(),
        commands: list
            .commands
            .iter()
            .map(|draw| match draw {
                Draw::Line {
                    node,
                    from,
                    to,
                    width,
                    color,
                } => Draw::Line {
                    node: *node,
                    from: from.map(|v| v * scale),
                    to: to.map(|v| v * scale),
                    width: width * scale,
                    color: *color,
                },
                Draw::Rect { node, rect, color } => Draw::Rect {
                    node: *node,
                    rect: scaled(*rect, scale),
                    color: *color,
                },
                Draw::RoundedRect {
                    node,
                    rect,
                    color,
                    radius,
                } => Draw::RoundedRect {
                    node: *node,
                    rect: scaled(*rect, scale),
                    color: *color,
                    radius: radius * scale,
                },
                Draw::Border {
                    node,
                    rect,
                    color,
                    radius,
                    width,
                } => Draw::Border {
                    node: *node,
                    rect: scaled(*rect, scale),
                    color: *color,
                    radius: radius * scale,
                    width: width * scale,
                },
                Draw::Image { node, rect, image } => Draw::Image {
                    node: *node,
                    rect: scaled(*rect, scale),
                    image: *image,
                },
                Draw::GlyphRun {
                    node,
                    font,
                    size,
                    origin,
                    glyphs,
                    color,
                } => Draw::GlyphRun {
                    node: *node,
                    font: *font,
                    size: size * scale,
                    origin: [origin[0] * scale, origin[1] * scale],
                    glyphs: glyphs.clone(),
                    color: *color,
                },
                Draw::ScalableGlyphRun {
                    node,
                    font,
                    size,
                    origin,
                    glyphs,
                    color,
                } => Draw::ScalableGlyphRun {
                    node: *node,
                    font: *font,
                    size: size * scale,
                    origin: [origin[0] * scale, origin[1] * scale],
                    glyphs: glyphs.clone(),
                    color: *color,
                },
                Draw::PushClip { rect } => Draw::PushClip {
                    rect: scaled(*rect, scale),
                },
                Draw::PushRoundedClip { rect, radius } => Draw::PushRoundedClip {
                    rect: scaled(*rect, scale),
                    radius: radius * scale,
                },
                Draw::PushTransform { translation } => Draw::PushTransform {
                    translation: [translation[0] * scale, translation[1] * scale],
                },
                Draw::PopClip => Draw::PopClip,
                Draw::PopTransform => Draw::PopTransform,
            })
            .collect(),
    }
}
