//! Cached inspections and queued controls belong to one Play world.
use super::*;

impl State {
    /// Readiness consumes a matching startup intent once. A request from an old
    /// world is discarded even when the next world's player tiles are ready.
    pub(super) fn take_simulate(&mut self, world: Option<&str>, ready: bool) -> Option<bool> {
        if self
            .simulate
            .as_ref()
            .is_some_and(|request| Some(request.world.as_str()) != world)
        {
            self.simulate = None;
        }
        if ready {
            self.simulate.take().map(|request| request.resume)
        } else {
            None
        }
    }
}

impl Shell {
    /// Called after the typed Play start has returned its world. Reconcile first
    /// so entering a new world cannot erase this fresh, explicitly tagged intent.
    pub(crate) fn queue_simulate(
        &mut self,
        editor: &mut Editor,
        resume: bool,
    ) -> ara_ui::Result<()> {
        self.reconcile_play_world(editor);
        let world = editor
            .play_world
            .as_ref()
            .ok_or("Simulate needs a starting Play world")?;
        self.play_controls.simulate = Some(Simulate {
            world: world.clone(),
            resume,
        });
        Ok(())
    }

    /// Restart may retain a world ID while replacing all of its runtime state.
    /// The restart command calls this explicitly before replacing the players.
    pub(crate) fn reset_play_transients(&mut self, editor: &mut Editor) {
        self.replace_play_world(editor, true);
    }

    /// Run even with no pending job: an idle Stop/start still invalidates drafts,
    /// Keep choices, spectator labels and a cancelled startup's Simulate request.
    pub(super) fn reconcile_play_world(&mut self, editor: &mut Editor) -> bool {
        if self.play_controls.world == editor.play_world {
            return false;
        }
        self.replace_play_world(editor, false);
        true
    }

    fn replace_play_world(&mut self, editor: &mut Editor, force: bool) {
        if let (Some(world), Some(surfaces)) = (&self.play_controls.world, &editor.play_surfaces) {
            surfaces.device(
                world,
                self.game_client,
                vec![ara_play_host::embed::DeviceEvent::FocusLost],
            );
        }
        let simulate = self.play_controls.simulate.take().filter(|request| {
            !force && editor.play_world.as_deref() == Some(request.world.as_str())
        });
        let revision = self.play_controls.revision.wrapping_add(1);
        let options = self.play_controls.options;
        let pending = self.play_controls.pending.take();
        self.play_controls = State {
            world: editor.play_world.clone(),
            options,
            revision,
            simulate,
            ..Default::default()
        };
        if let Some(pending) = pending {
            pending.cancel(editor);
        }
        self.game_players.clear();
        self.game_client = 0;
        self.game_captured = false;
        self.game_live = false;
        self.game_running = false;
        self.game_start = None;
        self.game_start_failed = false;
        self.game_fault = None;
        self.game_commands_status = None;
    }
}
