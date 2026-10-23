//! The Game view's GPU side (D19): the selected Play client's shared surface,
//! imported on the editor's device and shown as UI texture `game-view`. Each frame:
//! poll the play host, import a new surface generation when one appears (keep the
//! old one on screen until the new one has a frame), show the newest frame,
//! release the one it replaces, and ask for a resize when the panel changed size.
use std::time::{Duration, Instant};

use ara_editor_gpu::{NativeUi, PlaySurface};
use ara_gpu::vulkan::Context as Gpu;

use crate::{Editor, shell::Shell};

/// UI texture of the Game view image (`<image src='game-view'>`).
pub fn texture() -> u32 {
    ara_ui::source::stable_id("game-view") as u32 & 0x7fff_ffff
}

/// The surfaces this window shows, per Play world and client.
#[derive(Default)]
pub struct GameView {
    world: Option<String>,
    player: usize,
    shown: Option<PlaySurface>,
    pending: Option<PlaySurface>,
    resized: Option<Instant>,
    frames: Vec<Instant>,
}

impl GameView {
    /// Advance one editor frame; true when the Game view shows a new frame.
    pub fn update(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        ui: &mut NativeUi,
        gpu: &Gpu,
    ) -> ara_ui::Result<bool> {
        let Some(surfaces) = editor.play_surfaces.as_mut() else {
            return Ok(false);
        };
        surfaces.poll();
        let world = editor
            .play_world
            .clone()
            .filter(|w| surfaces.worlds().any(|s| s == w))
            .or_else(|| surfaces.worlds().next().map(str::to_owned));
        let players: Vec<usize> = world
            .as_deref()
            .map(|w| surfaces.surfaces(w).iter().map(|s| s.player).collect())
            .unwrap_or_default();
        shell.game_players = players.clone();
        let player = players
            .iter()
            .copied()
            .find(|p| *p == shell.game_client)
            .or_else(|| players.first().copied());
        // A different world or client: let go of everything shown.
        if world != self.world || player != Some(self.player) || player.is_none() {
            self.close(editor, ui)?;
            self.world = world.clone();
            self.player = player.unwrap_or(0);
            shell.game_live = false;
        }
        let (Some(world), Some(player)) = (world, player) else {
            return Ok(false);
        };
        let Some(surfaces) = editor.play_surfaces.as_mut() else {
            return Ok(false);
        };
        let Some(state) = surfaces
            .surfaces(&world)
            .into_iter()
            .find(|s| s.player == player)
        else {
            return Ok(false);
        };
        let generation = state.generation;
        let size = state.size;
        let known =
            |s: &Option<PlaySurface>| s.as_ref().is_some_and(|s| s.generation() == generation);
        if !known(&self.shown) && !known(&self.pending) {
            // SAFETY: `gpu` is the device `ui` was created on and outlives the
            // surface; this runs on the window thread between UI frames.
            let imported = unsafe { ui.import_play_surface(gpu, state.surface) }?;
            if let Some(old) = self.pending.replace(imported) {
                ui.close_play_surface(old)?;
            }
        }
        let mut showed = false;
        if let Some(frame) = surfaces.take(&world, player) {
            if self
                .pending
                .as_ref()
                .is_some_and(|p| p.generation() == frame.generation)
                && let Some(next) = self.pending.take()
                && let Some(old) = self.shown.replace(next)
                && let Some(previous) = ui.close_play_surface(old)?
            {
                surfaces.release(&world, player, previous);
            }
            if let Some(surface) = self
                .shown
                .as_mut()
                .filter(|s| s.generation() == frame.generation)
            {
                if let Some(previous) = surface.show(ui, texture(), frame)? {
                    surfaces.release(&world, player, previous);
                }
                showed = true;
                let now = Instant::now();
                self.frames.push(now);
                self.frames
                    .retain(|t| now.duration_since(*t) < Duration::from_secs(1));
                shell.game_fps = self.frames.len() as u32;
                shell.game_live = true;
                // Stats update in place, without rebuilding the panel every frame.
                if let Some(i) = shell.ui.find("game-stats-text") {
                    shell.ui.set_text(
                        i,
                        &format!("{} fps · {} × {}", shell.game_fps, size[0], size[1]),
                    )?;
                }
            } else {
                surfaces.release(&world, player, frame);
            }
        }
        shell.game_size = size;
        // Follow the panel's size (physical pixels), at most a few times a second.
        let wanted = shell.game_target;
        if wanted[0] >= 16
            && wanted[1] >= 16
            && wanted != size
            && self
                .resized
                .is_none_or(|t| t.elapsed() > Duration::from_millis(150))
        {
            surfaces.resize(&world, player, wanted[0], wanted[1]);
            self.resized = Some(Instant::now());
        }
        Ok(showed)
    }

    /// Stops showing every surface and returns their frames to the players.
    pub fn close(&mut self, editor: &mut Editor, ui: &mut NativeUi) -> ara_ui::Result<()> {
        for surface in [self.shown.take(), self.pending.take()]
            .into_iter()
            .flatten()
        {
            if let Some(frame) = ui.close_play_surface(surface)?
                && let (Some(world), Some(surfaces)) =
                    (self.world.as_deref(), editor.play_surfaces.as_ref())
            {
                surfaces.release(world, self.player, frame);
            }
        }
        self.frames.clear();
        Ok(())
    }
}
