//! Retained tree, independent invalidation lanes, Taffy measurement and display-list emission.
use crate::{
    display::{Box2, DisplayList, Draw, Glyph},
    source::{self, Element, Result, stable_id},
    style::{Computed, Sheet, State},
    text::{Text, TextSystem},
};
use parley::PositionedLayoutItem;
use serde::Serialize;
use taffy::prelude::*;

#[derive(Default)]
pub struct Dirty {
    pub style: bool,
    pub layout: bool,
    pub paint: bool,
}
pub struct Node {
    pub element: Element,
    pub computed: Computed,
    pub state: State,
    pub layout_id: NodeId,
    pub text: Option<Text>,
    pub dirty: Dirty,
    pub rect: Box2,
    pub clip: Box2,
}
pub struct Ui {
    pub nodes: Vec<Node>,
    pub tree: TaffyTree<usize>,
    pub sheet: Sheet,
    pub display: DisplayList,
    pub size: [f32; 2],
    pub paint_order: Vec<usize>,
}
#[derive(Default, Debug, Serialize)]
pub struct Work {
    pub restyled: usize,
    pub layout_passes: usize,
    pub shaped: usize,
    pub paint_nodes: usize,
}
/// UI snapshot copied at a simulation boundary; the future C# generator supplies this contract.
pub struct HudModel {
    pub health: u32,
}
impl Ui {
    pub fn parse(markup: &str, css: &str, size: [f32; 2]) -> Result<Self> {
        let elements = source::parse(markup)?;
        let sheet = Sheet::parse(css)?;
        let mut tree = TaffyTree::new();
        let mut nodes = Vec::new();
        for (i, element) in elements.into_iter().enumerate() {
            if element
                .attrs
                .get("bind")
                .is_some_and(|name| name != "Health")
            {
                return Err(format!(
                    "BIND001: {} references an unknown typed model field",
                    element.id
                )
                .into());
            }
            let state = State {
                disabled: element.attrs.get("disabled").is_some_and(|v| v == "true"),
                ..State::default()
            };
            let computed = sheet.compute(&element, state)?;
            let text = element.attrs.get("text").map(|v| Text::new(v.to_owned()));
            let layout_id = tree.new_leaf_with_context(computed.layout.clone(), i)?;
            nodes.push(Node {
                element,
                computed,
                state,
                layout_id,
                text,
                dirty: Dirty {
                    style: false,
                    layout: true,
                    paint: true,
                },
                rect: Box2::default(),
                clip: Box2::default(),
            });
        }
        for node in &nodes {
            let children: Vec<_> = node
                .element
                .children
                .iter()
                .map(|i| nodes[*i].layout_id)
                .collect();
            tree.set_children(node.layout_id, &children)?;
        }
        Ok(Self {
            nodes,
            tree,
            sheet,
            display: DisplayList::default(),
            size,
            paint_order: Vec::new(),
        })
    }
    pub fn index(&self, id: &str) -> usize {
        self.nodes
            .iter()
            .position(|n| n.element.id == id)
            .expect("fixture references a declared node")
    }
    pub fn set_text(&mut self, index: usize, value: &str) -> Result<()> {
        let node = &mut self.nodes[index];
        let text = node.text.as_mut().ok_or("node is not a text control")?;
        if text.set(value) {
            node.dirty.layout = true;
            node.dirty.paint = true;
            self.tree.mark_dirty(node.layout_id)?;
        }
        Ok(())
    }
    pub fn set_state(&mut self, index: usize, state: State) {
        if self.nodes[index].state != state {
            self.nodes[index].state = state;
            self.nodes[index].dirty.style = true;
        }
    }
    /// Changed bindings push values. No UI polling of an ECS world or a managed object is required.
    pub fn model_changed(&mut self, model: &HudModel) -> Result<()> {
        for index in 0..self.nodes.len() {
            if self.nodes[index]
                .element
                .attrs
                .get("bind")
                .is_some_and(|name| name == "Health")
            {
                self.set_text(index, &format!("HEALTH  {:03} / 100", model.health))?;
            }
        }
        Ok(())
    }
    pub fn frame(&mut self, text_system: &mut TextSystem) -> Result<Work> {
        let before = text_system.shapes;
        let mut work = Work::default();
        for node in &mut self.nodes {
            if node.dirty.style {
                let mut computed = self.sheet.compute(&node.element, node.state)?;
                if computed.layout != *self.tree.style(node.layout_id)? {
                    self.tree
                        .set_style(node.layout_id, std::mem::take(&mut computed.layout))?;
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
                        Some(
                            (l.size.width
                                - l.padding.left
                                - l.padding.right
                                - l.border.left
                                - l.border.right)
                                .max(1.0),
                        ),
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
            let clip = Box2 {
                x: 0.0,
                y: 0.0,
                w: self.size[0],
                h: self.size[1],
            };
            self.paint_node(0, [0.0, 0.0], [0.0, 0.0], clip, text_system)?;
            work.paint_nodes = self.nodes.len();
        }
        for node in &mut self.nodes {
            node.dirty.layout = false;
            node.dirty.paint = false;
        }
        work.shaped = text_system.shapes - before;
        Ok(work)
    }
    fn paint_node(
        &mut self,
        index: usize,
        parent: [f32; 2],
        translation: [f32; 2],
        clip: Box2,
        text_system: &TextSystem,
    ) -> Result<()> {
        self.paint_order.push(index);
        let node = &mut self.nodes[index];
        let l = *self.tree.layout(node.layout_id)?;
        let local = node.computed.translate;
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
            self.display.commands.push(Draw::PushClip { rect });
        }
        if visual.intersects(clip) {
            if node.element.tag == "image" {
                self.display.commands.push(Draw::Image {
                    node: id,
                    rect,
                    image: 1,
                });
            }
            if let Some(text) = &node.text {
                let origin = [
                    rect.x + l.padding.left + l.border.left,
                    rect.y + l.padding.top + l.border.top,
                ];
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
                // IME preedit is a separately styled range; the fixture field contains only preedit text.
                if node
                    .element
                    .attrs
                    .get("class")
                    .is_some_and(|s| s.split_whitespace().any(|c| c == "preedit"))
                {
                    self.display.commands.push(Draw::Rect {
                        node: id,
                        rect: Box2 {
                            x: origin[0],
                            y: origin[1] + text.layout.height() - 2.0,
                            w: text.layout.width(),
                            h: 1.5,
                        },
                        color: c.color,
                    });
                }
            }
        }
        let child_clip = if clipped {
            clip.intersection(visual)
        } else {
            clip
        };
        let child_count = node.element.children.len();
        for child_index in 0..child_count {
            let child = self.nodes[index].element.children[child_index];
            self.paint_node(child, [rect.x, rect.y], total, child_clip, text_system)?;
        }
        if clipped {
            self.display.commands.push(Draw::PopClip);
        }
        if transformed {
            self.display.commands.push(Draw::PopTransform);
        }
        Ok(())
    }
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        self.paint_order.iter().copied().rev().find(|index| {
            let n = &self.nodes[*index];
            is_focusable(n) && n.rect.contains(x, y) && n.clip.contains(x, y)
        })
    }
    pub fn inspect(&self) -> serde_json::Value {
        let nodes:Vec<_>=self.nodes.iter().map(|n| {
            let mut warnings=Vec::new();
            if let Some(parent)=n.element.parent {let p=self.nodes[parent].rect;if n.rect.x+n.rect.w>p.x+p.w+0.5||n.rect.y+n.rect.h>p.y+p.h+0.5 {warnings.push("overflow-parent");}}
            let layout=self.tree.layout(n.layout_id).expect("retained nodes have layouts");
            if n.text.as_ref().is_some_and(|t|t.layout.height()>n.rect.h-layout.padding.top-layout.padding.bottom-layout.border.top-layout.border.bottom+0.5) {warnings.push("clipped-text-height");}
            serde_json::json!({"id":n.element.id,"stable_id":stable_id(&n.element.id),"box":n.rect,"clip":n.clip,"warnings":warnings,"text":n.text.as_ref().map(crate::text::inspect)})
        }).collect();
        serde_json::json!({"viewport":self.size,"nodes":nodes,"focus_graph":crate::input::focus_graph(self),"batch_breaks":self.display.breaks(),"commands":self.display.commands.len(),"glyphs":self.display.glyphs.len()})
    }
}
pub fn is_focusable(n: &Node) -> bool {
    matches!(n.element.tag.as_str(), "button" | "field")
        && !n.state.disabled
        && n.rect.intersects(n.clip)
}
