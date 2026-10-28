//! Content ghosts and drop indicators in the shared native display list.
use super::*;
impl Shell {
    pub(super) fn invalidate_content_drag(&mut self) {
        if let Some(root) = self.ui.nodes.first_mut() {
            root.dirty.paint = true;
        }
        for surface in &mut self.floats {
            if let Some(root) = surface.ui.nodes.first_mut() {
                root.dirty.paint = true;
            }
        }
    }
    pub(in crate::shell) fn sync_content_ghost(&mut self) -> ara_ui::Result<()> {
        let Some(Drag::Content(drag)) = &self.drag else {
            return Ok(());
        };
        let Some(i) = self.ui.find("drop-label") else {
            return Ok(());
        };
        let shown = drag.session.active() && drag.session.pointer().surface == surface_id(self);
        self.ui.set_visible(i, shown)?;
        if !shown {
            return Ok(());
        }
        if let Some(tooltip) = self.ui.find("tooltip") {
            self.ui.set_visible(tooltip, false)?;
        }
        self.tooltip_text = None;
        let label = match drag.session.feedback() {
            Some(Feedback::Accept { label, .. }) => label.as_str(),
            Some(Feedback::Reject { reason, .. }) => reason.as_str(),
            None => match drag.session.payload() {
                Payload::Entity(_) => "Move entity",
                Payload::Asset(asset) => asset.name(),
                Payload::Component(_) => "Reorder component",
                Payload::Collection(_) => "Reorder item",
            },
        };
        if let Some(text) = self.ui.find("drop-label-text") {
            self.ui.set_text(text, label)?;
        }
        let node = &mut self.ui.nodes[i];
        let viewport = Box2 {
            x: 0.,
            y: 0.,
            w: self.ui.size[0],
            h: self.ui.size[1],
        };
        if let Some(rect) = kit::ghost_rect(
            self.pointer,
            [node.rect.w.max(140.), node.rect.h.max(28.)],
            viewport,
        ) {
            let offset = [
                rect.x - (node.rect.x - node.paint_offset[0]),
                rect.y - (node.rect.y - node.paint_offset[1]),
            ];
            if node.paint_offset != offset {
                node.paint_offset = offset;
                node.dirty.paint = true;
            }
        }
        Ok(())
    }
    pub(in crate::shell) fn paint_content_drag(&mut self) {
        let Some(Drag::Content(drag)) = &self.drag else {
            return;
        };
        if !drag.session.active() || drag.session.pointer().surface != surface_id(self) {
            return;
        }
        let (indicator, invalid) = match drag.session.feedback() {
            Some(Feedback::Accept { indicator, .. }) => (Some(*indicator), false),
            Some(Feedback::Reject { indicator, .. }) => (*indicator, true),
            None => return,
        };
        let Some(indicator) = indicator else {
            return;
        };
        let color = self
            .ui
            .sheet
            .color_token(if invalid {
                "--destructive"
            } else {
                "--primary"
            })
            .unwrap_or(ara_ui::style::Color(128, 128, 128, 255));
        let node = stable_id("content-drop-preview");
        match indicator {
            Indicator::Between(rect) => {
                self.ui
                    .display
                    .commands
                    .push(Draw::Rect { node, rect, color })
            }
            Indicator::Inside(rect) => {
                let mut fill = color;
                fill.3 = 30;
                self.ui.display.commands.push(Draw::RoundedRect {
                    node,
                    rect,
                    radius: 4.,
                    color: fill,
                });
                self.ui.display.commands.push(Draw::Border {
                    node: node ^ 1,
                    rect,
                    radius: 4.,
                    color,
                    width: 1.5,
                });
            }
        }
    }
}
