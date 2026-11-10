//! The legacy shell's Pilot Selected Camera, over `crate::pilot`.
#[cfg(test)]
mod tests;
use super::{Editor, Shell, commands::Target};
pub(super) use crate::pilot::Pilot;
#[cfg(test)]
use crate::{
    pilot::{Lookup, restore},
    state::Camera,
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

impl Shell {
    /// Explicitly enter/leave authoring through a real perspective Camera.
    pub(crate) fn toggle_pilot(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let entity = target.entity(&editor.state).map(str::to_owned);
        let mut pilot = std::mem::take(&mut self.pilot);
        let result = pilot.toggle(editor, entity.as_deref(), &mut |editor| {
            self.resolved(editor)
        });
        if let Some(note) = pilot.take_note() {
            self.status_note = Some(note);
        }
        self.pilot = pilot;
        result
    }
    /// A checked command state; selection changes do not retarget the piloted Camera.
    pub(crate) fn pilot_active(&self) -> bool {
        self.pilot.active()
    }
    /// Stable target for a Scene badge or explicit Unpilot action.
    pub(crate) fn pilot_target(&self) -> Option<&str> {
        self.pilot.target()
    }
    /// Capture ownership evidence without authoring or opening a window.
    pub(crate) fn pilot_report(&self) -> Value {
        self.pilot.report()
    }
    /// Call after native routing and fly integration; idle/agent polls pass false
    /// for native_camera_change so unrelated view updates can never author a Camera.
    pub(crate) fn poll_pilot(
        &mut self,
        editor: &mut Editor,
        navigation_active: bool,
        native_camera_change: bool,
    ) -> ara_ui::Result<bool> {
        let mut pilot = std::mem::take(&mut self.pilot);
        let result = pilot.poll(
            editor,
            navigation_active,
            native_camera_change,
            &mut |editor| self.resolved(editor),
        );
        if let Some(note) = pilot.take_note() {
            self.status_note = Some(note);
        }
        self.pilot = pilot;
        result
    }
    /// Escape/focus loss restores only the in-progress transient Scene view.
    pub(crate) fn cancel_pilot_navigation(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let result = self.pilot.cancel(editor);
        if let Some(note) = self.pilot.take_note() {
            self.status_note = Some(note);
        }
        result
    }
}
