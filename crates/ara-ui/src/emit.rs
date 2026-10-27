//! Painter-order retained traversal, clips, world translations and glyph arena emission.
use crate::{
    Result,
    paint::{Box2, Draw, Glyph},
    runtime::Ui,
    source::stable_id,
};
use ara_ui_text::TextSystem;
use parley::PositionedLayoutItem;
use taffy::Display;
impl Ui {
    pub(crate) fn paint_node(
        &mut self,
        index: usize,
        parent: [f32; 2],
        translation: [f32; 2],
        clip: Box2,
        text_system: &TextSystem,
    ) -> Result<()> {
        self.paint_order.push(index);
        self.paint_starts.push(self.display.commands.len());
        let node = &mut self.nodes[index];
        let l = *self.tree.layout(node.layout_id)?;
        if self.tree.style(node.layout_id)?.display == Display::None {
            self.paint_order.pop();
            self.paint_starts.pop();
            return Ok(());
        }
        let local = [
            node.computed.translate[0] + node.paint_offset[0],
            node.computed.translate[1] + node.paint_offset[1],
        ];
        let total = [translation[0] + local[0], translation[1] + local[1]];
        let rect = Box2 {
            x: parent[0] + l.location.x,
            y: parent[1] + l.location.y,
            w: l.size.width,
            h: l.size.height,
        };
        let visual = Box2 {
            x: rect.x + total[0],
            y: rect.y + total[1],
            ..rect
        };
        node.rect = visual;
        node.clip = clip;
        let id = stable_id(&node.element.id);
        let numeric = crate::input::numeric::declared(&node.element);
        let grip_width = if numeric {
            crate::input::numeric::GRIP
        } else {
            0.
        };
        let c = &node.computed;
        let transformed = local != [0.0, 0.0];
        let clipped = c.clip;
        if transformed {
            self.display
                .commands
                .push(Draw::PushTransform { translation: local });
        }
        if visual.intersects(clip) {
            if c.background.3 > 0 {
                self.display.commands.push(if c.radius > 0.0 {
                    Draw::RoundedRect {
                        node: id,
                        rect,
                        radius: c.radius,
                        color: c.background,
                    }
                } else {
                    Draw::Rect {
                        node: id,
                        rect,
                        color: c.background,
                    }
                });
            }
            if c.border > 0.0 {
                self.display.commands.push(Draw::Border {
                    node: id,
                    rect,
                    radius: c.radius,
                    width: c.border,
                    color: c.border_color,
                });
            }
        }
        if clipped {
            self.display.commands.push(if c.radius > 0.0 {
                Draw::PushRoundedClip {
                    rect,
                    radius: c.radius,
                }
            } else {
                Draw::PushClip { rect }
            });
        }
        if visual.intersects(clip) {
            if node.element.tag == "image" {
                self.display.commands.push(Draw::Image {
                    node: id,
                    rect,
                    image: node
                        .element
                        .attrs
                        .get("src")
                        .map(|s| stable_id(s) as u32 & 0x7fff_ffff)
                        .unwrap_or(0),
                    // `tint` images (white icons) take their element's text color.
                    tint: if node.element.attrs.contains_key("tint") {
                        node.computed.color
                    } else {
                        crate::style::Color(255, 255, 255, 255)
                    },
                });
            }
            if let Some(text) = &node.text {
                let mut origin = [
                    rect.x + l.padding.left + l.border.left,
                    rect.y + l.padding.top + l.border.top,
                ];
                // Only an input being edited scrolls to its caret; unfocused inputs
                // show their text from the start (`-6.455`, not `455`).
                if node.state.focus
                    && let Some(crate::widgets::Widget::Input(input)) = self.widgets.get(&id)
                {
                    origin[0] -= crate::widgets::input_scroll(
                        input,
                        text,
                        (l.size.width
                            - l.padding.left
                            - l.padding.right
                            - l.border.left
                            - l.border.right
                            - grip_width)
                            .max(1.0),
                    );
                }
                if c.text_end && !node.state.focus {
                    origin[0] += (l.size.width
                        - l.padding.left
                        - l.padding.right
                        - l.border.left
                        - l.border.right
                        - grip_width
                        - text.layout.width())
                    .max(0.);
                }
                for line in text.layout.lines() {
                    for item in line.items() {
                        if let PositionedLayoutItem::GlyphRun(run) = item {
                            let start = self.display.glyphs.len();
                            self.display
                                .glyphs
                                .extend(run.positioned_glyphs().map(|g| Glyph {
                                    id: g.id,
                                    x: g.x,
                                    y: g.y,
                                }));
                            let end = self.display.glyphs.len();
                            self.display.commands.push(Draw::GlyphRun {
                                node: id,
                                font: text_system.handle(run.run().font()),
                                size: run.run().font_size(),
                                origin,
                                glyphs: start..end,
                                color: c.color,
                            });
                        }
                    }
                }
                crate::widget_paint::decorations(
                    self.widgets.get(&id),
                    Some(text),
                    node.state.focus,
                    numeric,
                    rect,
                    origin,
                    id,
                    self.sheet.color_token("--accent").unwrap_or(c.border_color),
                    &mut self.display,
                );
            }
        }
        if node.text.is_none() {
            crate::widget_paint::decorations(
                self.widgets.get(&id),
                None,
                node.state.focus,
                numeric,
                rect,
                [rect.x, rect.y],
                id,
                self.sheet.color_token("--accent").unwrap_or(c.color),
                &mut self.display,
            );
        }
        let scroll = match self.widgets.get(&id) {
            Some(crate::widgets::Widget::Scroll(p)) => [
                -p[0],
                if node.virtual_extent.is_some() {
                    0.
                } else {
                    -p[1]
                },
            ],
            _ => [0.0, 0.0],
        };
        let scrolled = scroll != [0.0, 0.0];
        if scrolled {
            self.display.commands.push(Draw::PushTransform {
                translation: scroll,
            });
        }
        let child_clip = if clipped {
            clip.intersection(visual)
        } else {
            clip
        };
        let child_count = node.element.children.len();
        for child_index in 0..child_count {
            let child = self.nodes[index].element.children[child_index];
            self.paint_node(
                child,
                [rect.x, rect.y],
                [total[0] + scroll[0], total[1] + scroll[1]],
                child_clip,
                text_system,
            )?;
        }
        if scrolled {
            self.display.commands.push(Draw::PopTransform);
        }
        if self.scroll_position(index).is_some() {
            self.paint_scrollbars(index, total)?;
        }
        if clipped {
            self.display.commands.push(Draw::PopClip);
        }
        if transformed {
            self.display.commands.push(Draw::PopTransform);
        }
        Ok(())
    }
}
