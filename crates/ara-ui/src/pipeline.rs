//! Separate incremental restyle, Taffy/Parley layout and display-list emission.
use crate::{
    Result,
    paint::Box2,
    runtime::{Ui, Work},
};
use ara_ui_text::TextSystem;
use taffy::prelude::*;
#[path = "text_fit.rs"]
mod text_fit;
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
            let computed = if self.nodes[index].dirty.style {
                let mut computed = self
                    .sheet
                    .compute(&self.nodes[index].element, self.nodes[index].state)?;
                if self.nodes[index].element.tag == "text"
                    && let Some(parent) = self.nodes[index].element.parent
                    && matches!(
                        self.nodes[parent].element.tag.as_str(),
                        "feedback" | "banner"
                    )
                {
                    // Status prose follows its container's typography and wraps,
                    // including labels that formerly inherited chrome nowrap.
                    computed.font_size = self.nodes[parent].computed.font_size;
                    computed
                        .font_family
                        .clone_from(&self.nodes[parent].computed.font_family);
                    computed.wrap_text = true;
                    computed.inherit_color = true;
                }
                if self.nodes[index].element.tag == "banner" {
                    computed.layout.size.height = Dimension::auto();
                    computed.layout.flex_shrink = 0.0;
                }
                Some(computed)
            } else {
                None
            };
            let node = &mut self.nodes[index];
            if let Some(mut computed) = computed {
                if computed.inherit_color
                    && let Some(color) = parent_color
                {
                    computed.color = color;
                }
                let color_changed = computed.color != node.computed.color;
                let typography_changed = computed.font_size != node.computed.font_size
                    || computed.font_family != node.computed.font_family;
                let message = matches!(node.element.tag.as_str(), "feedback" | "banner");
                // Taffy's effective style also contains runtime placement and
                // fitted auto-text height. A paint-only restyle must retain
                // those results instead of resetting them to authored `auto`.
                if computed.layout != node.computed.layout {
                    let mut layout = computed.layout.clone();
                    if !node.visible {
                        layout.display = Display::None;
                    }
                    if let Some(placed) =
                        self.placed.get(&crate::source::stable_id(&node.element.id))
                    {
                        placed.apply(&mut layout);
                    }
                    if layout != *self.tree.style(node.layout_id)? {
                        self.tree.set_style(node.layout_id, layout)?;
                        node.dirty.layout = true;
                    }
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
                let message_typography = message && (typography_changed || node.dirty.layout);
                if color_changed || message_typography {
                    let count = node.element.children.len();
                    for position in 0..count {
                        let child = self.nodes[index].element.children[position];
                        if (color_changed && self.nodes[child].computed.inherit_color)
                            || (message_typography && self.nodes[child].element.tag == "text")
                        {
                            self.nodes[child].dirty.style = true;
                        }
                    }
                }
            }
        }
        self.sync_choices()?;
        self.prepare_popovers()?;
        // Hints share font caches but never take part in Taffy measurement.
        // A color-only restyle repaints their cached glyphs without reshaping.
        for node in &mut self.nodes {
            if let Some(hint) = &mut node.placeholder
                && (node.dirty.paint || node.dirty.layout || hint.dirty)
            {
                if hint.family != node.computed.font_family {
                    hint.family.clone_from(&node.computed.font_family);
                    hint.dirty = true;
                }
                text_system.measure(hint, None, node.computed.font_size);
            }
        }
        let layout = self.nodes.iter().any(|n| n.dirty.layout);
        let paint = layout || self.nodes.iter().any(|n| n.dirty.paint);
        if layout {
            self.compute_layout(text_system)?;
            work.layout_passes = 1;
        }
        if paint {
            if !layout {
                self.place_feedback_nodes();
            }
            work.paint_nodes = self.emit_display(text_system)?;
            // Width clamps rewrap copy and submenus follow a menu that just flipped.
            // Settle that dependency in this frame; ordinary popovers need one pass,
            // idle frames none. Bound the work even for pathological anchor cycles.
            for _ in 0..4 {
                let text_changed = work.layout_passes > 0 && self.fit_auto_text()?;
                let overlays_changed = self.place_popovers()?;
                if !text_changed && !overlays_changed {
                    break;
                }
                self.compute_layout(text_system)?;
                work.layout_passes += 1;
                work.paint_nodes += self.emit_display(text_system)?;
            }
            if work.layout_passes > 0 && self.place_feedback_nodes() {
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
                                // Taffy rounds the final box to pixels. Rounding a
                                // fractional intrinsic width down would rebreak a
                                // naturally single-line label onto an extra line.
                                width: known.width.unwrap_or(measured.width.ceil()),
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
        self.clamp_scrolls()?;
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
