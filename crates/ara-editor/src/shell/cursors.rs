//! Cursor state for editor gestures that bypass ordinary widget pointer capture.
use super::Shell;
impl Shell {
    pub(crate) fn resizing_column(&self) -> bool {
        if self.scrub.is_some() || matches!(self.drag, Some(super::Drag::Column { .. })) {
            return true;
        }
        self.shown_node("inspector-scroll").is_some_and(|i| {
            let r = self.ui.nodes[i].rect;
            r.contains(self.pointer[0], self.pointer[1])
                && (self.pointer[0] - r.x - self.label_px(r.w)).abs() <= 4.
        })
    }
}
