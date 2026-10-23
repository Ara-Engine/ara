//! Separate incremental restyle, Taffy/Parley layout and display-list emission.
use crate::{
    Result,
    paint::Box2,
    runtime::{Ui, Work},
};
use ara_ui_text::TextSystem;
use taffy::prelude::*;
impl Ui {
    /// Run restyle, cached Taffy/Parley layout and retained display-list emission.
    pub fn frame(&mut self, text_system: &mut TextSystem) -> Result<Work> {
        let before = text_system.shapes;
        let mut work = Work::default();
        for node in &mut self.nodes {
            if node.dirty.style {
                let computed = self.sheet.compute(&node.element, node.state)?;
                let mut layout = computed.layout.clone();
                if !node.visible {
                    layout.display = Display::None;
                }
                if layout != *self.tree.style(node.layout_id)? {
                    self.tree.set_style(node.layout_id, layout)?;
                    node.dirty.layout = true;
                }
                if computed.font_size != node.computed.font_size {
                    if let Some(text) = &mut node.text {
                        text.dirty = true;
                    }
                    self.tree.mark_dirty(node.layout_id)?;
                    node.dirty.layout = true;
                }
                node.computed = computed;
                node.dirty.style = false;
                node.dirty.paint = true;
                work.restyled += 1;
            }
        }
        self.sync_choices()?;
        let layout = self.nodes.iter().any(|n| n.dirty.layout);
        let paint = layout || self.nodes.iter().any(|n| n.dirty.paint);
        if layout {
            let root = self.nodes[0].layout_id;
            let nodes = &mut self.nodes;
            self.tree.compute_layout_with_measure(
                root,
                Size {
                    width: AvailableSpace::Definite(self.size[0]),
                    height: AvailableSpace::Definite(self.size[1]),
                },
                |input, _, context, style| {
                    taffy::compute_leaf_layout(
                        input,
                        style,
                        |_, _| 0.0,
                        |known, available| {
                            let Some(index) = context else {
                                return Size::ZERO;
                            };
                            let node = &mut nodes[*index];
                            let Some(text) = &mut node.text else {
                                return Size::ZERO;
                            };
                            let width = known.width.or(match available.width {
                                AvailableSpace::Definite(w) => Some(w),
                                AvailableSpace::MinContent => Some(1.0),
                                _ => None,
                            });
                            let width = if matches!(node.element.tag.as_str(), "field" | "input") {
                                None
                            } else {
                                width
                            };
                            let measured =
                                text_system.measure(text, width, node.computed.font_size);
                            Size {
                                width: known.width.unwrap_or(measured.width),
                                height: known.height.unwrap_or(measured.height),
                            }
                        },
                    )
                },
            )?;
            // Fully constrained leaves may skip intrinsic measurement; finalize text at the actual content width.
            for node in &mut self.nodes {
                if let Some(text) = &mut node.text {
                    let l = self.tree.layout(node.layout_id)?;
                    text_system.measure(
                        text,
                        if matches!(node.element.tag.as_str(), "field" | "input") {
                            None
                        } else {
                            Some(
                                (l.size.width
                                    - l.padding.left
                                    - l.padding.right
                                    - l.border.left
                                    - l.border.right)
                                    .max(1.0),
                            )
                        },
                        node.computed.font_size,
                    );
                }
            }
            work.layout_passes = 1;
        }
        if paint {
            self.display.commands.clear();
            self.display.glyphs.clear();
            self.paint_order.clear();
            for n in &mut self.nodes {
                n.rect = Box2::default();
                n.clip = Box2::default();
            }
            let clip = Box2 {
                x: 0.0,
                y: 0.0,
                w: self.size[0],
                h: self.size[1],
            };
            self.paint_node(0, [0.0, 0.0], [0.0, 0.0], clip, text_system)?;
            work.paint_nodes = self.paint_order.len();
        }
        for node in &mut self.nodes {
            node.dirty.layout = false;
            node.dirty.paint = false;
        }
        work.shaped = text_system.shapes - before;
        Ok(work)
    }
}
