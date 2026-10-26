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
        // Parents precede children, so an inherited color is final when a child restyles.
        for index in 0..self.nodes.len() {
            let parent_color = self.nodes[index]
                .element
                .parent
                .map(|p| self.nodes[p].computed.color);
            let node = &mut self.nodes[index];
            if node.dirty.style {
                let mut computed = self.sheet.compute(&node.element, node.state)?;
                if computed.inherit_color
                    && let Some(color) = parent_color
                {
                    computed.color = color;
                }
                let color_changed = computed.color != node.computed.color;
                let mut layout = computed.layout.clone();
                if !node.visible {
                    layout.display = Display::None;
                }
                if let Some(placed) = self.placed.get(&crate::source::stable_id(&node.element.id)) {
                    placed.apply(&mut layout);
                }
                if layout != *self.tree.style(node.layout_id)? {
                    self.tree.set_style(node.layout_id, layout)?;
                    node.dirty.layout = true;
                }
                if computed.font_family != node.computed.font_family {
                    if let Some(text) = &mut node.text {
                        text.family.clone_from(&computed.font_family);
                        text.dirty = true;
                    }
                    self.tree.mark_dirty(node.layout_id)?;
                    node.dirty.layout = true;
                }
                if computed.font_size != node.computed.font_size
                    || computed.wrap_text != node.computed.wrap_text
                {
                    if let Some(text) = &mut node.text
                        && computed.font_size != node.computed.font_size
                    {
                        text.dirty = true;
                    }
                    self.tree.mark_dirty(node.layout_id)?;
                    node.dirty.layout = true;
                }
                node.computed = computed;
                node.dirty.style = false;
                node.dirty.paint = true;
                work.restyled += 1;
                if color_changed {
                    let children = node.element.children.clone();
                    for child in children {
                        if self.nodes[child].computed.inherit_color {
                            self.nodes[child].dirty.style = true;
                        }
                    }
                }
            }
        }
        self.sync_choices()?;
        let layout = self.nodes.iter().any(|n| n.dirty.layout);
        let paint = layout || self.nodes.iter().any(|n| n.dirty.paint);
        if layout {
            self.compute_layout(text_system)?;
            work.layout_passes = 1;
        }
        if paint {
            work.paint_nodes = self.emit_display(text_system)?;
            // Popovers are placed against painted anchors; one that moved lays out and
            // paints once more.
            if self.place_popovers()? {
                self.compute_layout(text_system)?;
                work.layout_passes += 1;
                work.paint_nodes += self.emit_display(text_system)?;
            }
        }
        for node in &mut self.nodes {
            node.dirty.layout = false;
            node.dirty.paint = false;
        }
        work.shaped = text_system.shapes - before;
        Ok(work)
    }
    /// Taffy layout with Parley text measurement, then text finalized at its laid-out width.
    fn compute_layout(&mut self, text_system: &mut TextSystem) -> Result<()> {
        {
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
                            if text.family != node.computed.font_family {
                                text.family.clone_from(&node.computed.font_family);
                                text.dirty = true;
                            }
                            let width = known.width.or(match available.width {
                                AvailableSpace::Definite(w) => Some(w),
                                AvailableSpace::MinContent => Some(1.0),
                                _ => None,
                            });
                            let width = if !node.computed.wrap_text
                                || matches!(node.element.tag.as_str(), "field" | "input")
                            {
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
                    if text.family != node.computed.font_family {
                        text.family.clone_from(&node.computed.font_family);
                        text.dirty = true;
                    }
                    let l = self.tree.layout(node.layout_id)?;
                    text_system.measure(
                        text,
                        if !node.computed.wrap_text
                            || matches!(node.element.tag.as_str(), "field" | "input")
                        {
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
        }
        Ok(())
    }
    /// Rebuild the display list, rects and clips; returns the nodes visited.
    fn emit_display(&mut self, text_system: &mut TextSystem) -> Result<usize> {
        {
            self.display.commands.clear();
            self.display.glyphs.clear();
            self.paint_order.clear();
            self.paint_starts.clear();
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
        }
        Ok(self.paint_order.len())
    }
}
