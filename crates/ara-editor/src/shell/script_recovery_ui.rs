//! Retained presentation of owned C# reload requests. Player work stays in PlayService.
use super::*;
use ara_play_host::{
    ScriptRecoveryOutcome, ScriptRecoveryRequest, ScriptRecoveryStatus, ScriptRecoveryTicket,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
mod state;
mod view;
pub(crate) use state::State;
#[cfg(test)]
mod tests;

impl Shell {
    /// Explicit reload of the current running world, all players. No restart fallback.
    pub(crate) fn reload_scripts(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if let Some(reason) = self.script_recovery_blocker(editor) {
            return Err(reason.into());
        }
        let world = editor
            .play_world
            .as_ref()
            .ok_or("Start Play before reloading C# scripts")?;
        let request = ScriptRecoveryRequest {
            session: editor.session.clone(),
            world: world.clone(),
            player: None,
            why: Some("Editor: Reload Scripts".into()),
        };
        self.script_recovery_ui
            .start(request, |request| editor.script_recovery.request(request));
        if let Some(note) = self.script_recovery_ui.note(editor.play_world.as_deref()) {
            self.status_note = Some(note);
        }
        self.rebuild(editor)
    }

    /// One bounded owner poll at 4 Hz; old-world tickets remain owned until finished.
    pub(crate) fn poll_script_recovery(&mut self, editor: &mut Editor) -> bool {
        let changed = self
            .script_recovery_ui
            .poll(Instant::now(), |ticket| editor.script_recovery.poll(ticket));
        if changed && let Some(note) = self.script_recovery_ui.note(editor.play_world.as_deref()) {
            self.status_note = Some(note);
        }
        // Status diagnostics are already on the bus before finished. Never emit
        // them again: that would create new seqs and resurrect retired faults.
        changed
    }

    pub(crate) fn script_recovery_blocker(&self, editor: &Editor) -> Option<&'static str> {
        let Some(world) = editor.play_world.as_deref() else {
            return Some(
                "Start Play to reload scripts; automatic source compilation remains active",
            );
        };
        if self.game_start.is_some() || editor.state.playing.is_none() {
            return Some("Wait for Play to finish starting or restarting before reloading scripts");
        }
        if self.script_recovery_ui.pending_world(world) {
            Some("A C# reload is already pending for this Play world")
        } else if self.script_recovery_ui.full() {
            Some("Wait for this editor's pending C# reload requests to finish")
        } else {
            None
        }
    }

    pub(crate) fn script_recovery_busy(&self) -> bool {
        self.script_recovery_ui.busy()
    }

    /// Tickets remain visible to resource draining even after navigation/world changes.
    pub(crate) fn script_recovery_resource_report(&self) -> Value {
        self.script_recovery_ui.report()
    }
}
