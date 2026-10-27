//! D19 imports and displays every embedded rendering player at once. Each player
//! keeps its own surface generation, newest frame, resize cadence and UI texture.
use crate::{Editor, gpu::UiDevice as Gpu, shell::Shell};
use ara_editor_gpu::{NativeUi, PlaySurface};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
mod player;
use player::PlayerView;

/// Legacy single Game image, retained for the preview/placeholder registration contract.
pub fn texture() -> u32 {
    ara_ui::source::stable_id("game-view") as u32 & 0x7fff_ffff
}
/// Distinct native UI image for an embedded player (0 authority, 1–16 clients).
pub fn player_texture(player: usize) -> u32 {
    ara_ui::source::stable_id(&format!("game-view-{player}")) as u32 & 0x7fff_ffff
}

/// Surfaces belong to a world and player; selecting input never discards another player's view.
#[derive(Default)]
pub struct GameView {
    world: Option<String>,
    views: BTreeMap<usize, PlayerView>,
    checked: Option<Instant>,
    checked_world: Option<String>,
}
impl GameView {
    /// Poll the shared mailbox once and show each client's newest completed frame.
    pub fn update(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        ui: &mut NativeUi,
        gpu: &Gpu,
    ) -> ara_ui::Result<bool> {
        self.check_status(editor, shell);
        let Some(surfaces) = editor.play_surfaces.as_mut() else {
            return Ok(false);
        };
        surfaces.poll();
        let world = editor
            .play_world
            .as_deref()
            .filter(|world| surfaces.worlds().any(|known| known == *world));
        if world != self.world.as_deref() {
            let next = world.map(str::to_owned);
            self.close_views(Some(surfaces), ui)?;
            self.world = next;
            shell.game_live = false;
            shell.game_captured = false;
        }
        let players: Vec<_> = self
            .world
            .as_deref()
            .map(|world| {
                surfaces
                    .surfaces(world)
                    .iter()
                    .map(|state| state.player)
                    .collect()
            })
            .unwrap_or_default();
        if shell.game_players != players {
            shell.game_players = players;
            shell.play_controls.revision += 1;
        }
        if !shell.game_players.contains(&shell.game_client) {
            shell.game_client = shell.game_players.first().copied().unwrap_or(0);
            shell.game_captured = false;
        }
        let removed: Vec<_> = self
            .views
            .keys()
            .copied()
            .filter(|player| !shell.game_players.contains(player))
            .collect();
        for player in removed {
            if let Some(mut view) = self.views.remove(&player) {
                view.close(self.world.as_deref(), player, Some(surfaces), ui)?;
            }
            shell.play_controls.players.remove(&player);
        }
        let Some(world) = self.world.as_deref() else {
            if !shell.play_controls.players.is_empty() {
                shell.play_controls.players.clear();
                shell.play_controls.revision += 1;
            }
            shell.game_live = false;
            return Ok(false);
        };
        let mut changed = false;
        for &player in &shell.game_players {
            let view = self.views.entry(player).or_default();
            changed |= view.update(
                world,
                player,
                shell.game_target,
                shell.game_scale,
                surfaces,
                ui,
                gpu,
            )?;
            let info = view.info();
            let previous = shell.play_controls.players.insert(player, info);
            if previous.is_none_or(|old| old.live != info.live) {
                shell.play_controls.revision += 1;
            }
            if shell.game_stats
                && previous != Some(info)
                && let Some(index) = shell.ui.find(&format!("game-stats-text-{player}"))
            {
                shell
                    .ui
                    .set_text(index, &crate::shell::game_stats_text(info.fps, info.size))?;
            }
        }
        let shown = shell.play_controls.players.get(&shell.game_client);
        shell.game_live = shown.is_some_and(|view| view.live);
        shell.game_fps = shown.map_or(0, |view| view.fps);
        shell.game_size = shown.map_or([0; 2], |view| view.size);
        Ok(changed)
    }
    /// Startup/status is polled on a cadence, never once per client or display frame.
    fn check_status(&mut self, editor: &mut Editor, shell: &mut Shell) {
        if editor.state.playing.is_none() {
            shell.game_fault = None;
            shell.game_start = None;
            shell.game_start_failed = false;
            shell.game_running = false;
            return;
        }
        if self.checked_world != editor.play_world {
            self.checked_world = editor.play_world.clone();
            self.checked = None;
            shell.game_running = false;
        }
        let every = if shell.game_start.is_some() { 100 } else { 500 };
        if self
            .checked
            .is_some_and(|at| at.elapsed() < Duration::from_millis(every))
        {
            return;
        }
        self.checked = Some(Instant::now());
        shell.read_play_status(editor);
    }
    fn close_views(
        &mut self,
        surfaces: Option<&ara_play_host::embed::Surfaces>,
        ui: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        for (player, mut view) in std::mem::take(&mut self.views) {
            view.close(self.world.as_deref(), player, surfaces, ui)?;
        }
        Ok(())
    }
    /// Release every player before the editor drops its UI device.
    pub fn close(&mut self, editor: &mut Editor, ui: &mut NativeUi) -> ara_ui::Result<()> {
        self.close_views(editor.play_surfaces.as_ref(), ui)?;
        self.world = None;
        Ok(())
    }
}
