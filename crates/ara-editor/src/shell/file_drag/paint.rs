use super::*;
use ara_ui::{paint::Draw, source::stable_id};
impl Shell {
    pub(super) fn invalidate_file_drag(&mut self) {
        if let Some(root) = self.ui.nodes.first_mut() {
            root.dirty.paint = true;
        }
        for surface in &mut self.floats {
            if let Some(root) = surface.ui.nodes.first_mut() {
                root.dirty.paint = true;
            }
        }
    }
    pub(super) fn hide_file_drag_label(&mut self) -> ara_ui::Result<()> {
        if let Some(index) = self.ui.find("drop-label") {
            self.ui.set_visible(index, false)?;
        }
        for surface in &mut self.floats {
            if let Some(index) = surface.ui.find("drop-label") {
                surface.ui.set_visible(index, false)?;
            }
        }
        Ok(())
    }
    /// Only destination action/rejection text is drawn; the native source ghost stays OS-owned.
    pub(in crate::shell) fn sync_file_drag(&mut self) -> ara_ui::Result<()> {
        if !self.file_drag_pending() {
            return Ok(());
        }
        let shown =
            self.file_drag.session.as_ref().is_some_and(|session| {
                session.active() && session.pointer().surface == surface(self)
            });
        let Some(index) = self.ui.find("drop-label") else {
            return Ok(());
        };
        self.ui.set_visible(index, shown)?;
        if !shown {
            return Ok(());
        }
        let session = self
            .file_drag
            .session
            .as_ref()
            .ok_or("Native file feedback disappeared")?;
        let label = match session.feedback() {
            Some(Feedback::Accept { label, .. }) => label.as_str(),
            Some(Feedback::Reject { reason, .. }) => reason.as_str(),
            None => "Drop files onto a folder in Assets",
        };
        if let Some(text) = self.ui.find("drop-label-text") {
            self.ui.set_text(text, label)?;
        }
        if let Some(tooltip) = self.ui.find("tooltip") {
            self.ui.set_visible(tooltip, false)?;
        }
        self.tooltip_text = None;
        let at = session.pointer().local;
        let avoid = match session.feedback() {
            Some(Feedback::Accept { indicator, .. }) => Some(indicator.rect()),
            Some(Feedback::Reject { indicator, .. }) => indicator.map(|value| value.rect()),
            None => None,
        };
        self.ui.place_feedback(index, at, avoid)
    }
    pub(in crate::shell) fn paint_file_drag(&mut self) {
        let Some(session) = &self.file_drag.session else {
            return;
        };
        if !session.active() || session.pointer().surface != surface(self) {
            return;
        }
        let (indicator, invalid) = match session.feedback() {
            Some(Feedback::Accept { indicator, .. }) => (Some(*indicator), false),
            Some(Feedback::Reject { indicator, .. }) => (*indicator, true),
            None => return,
        };
        let Some(indicator) = indicator else {
            return;
        };
        let rect = indicator.rect();
        let color = self
            .ui
            .sheet
            .color_token(if invalid {
                "--destructive"
            } else {
                "--primary"
            })
            .unwrap_or(ara_ui::style::Color(128, 128, 128, 255));
        let mut fill = color;
        fill.3 = 30;
        let node = stable_id("native-file-drop-preview");
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
