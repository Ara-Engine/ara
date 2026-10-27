use super::*;
use ara_play_host::embed::Surfaces;
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct PlayerView {
    shown: Option<PlaySurface>,
    pending: Option<PlaySurface>,
    resized: Option<Instant>,
    frames: VecDeque<Instant>,
    size: [u32; 2],
    fps: u32,
    live: bool,
    texture: Option<u32>,
}
impl PlayerView {
    pub fn info(&self) -> crate::shell::play_controls::PlayerInfo {
        crate::shell::play_controls::PlayerInfo {
            live: self.live,
            size: self.size,
            fps: self.fps,
        }
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "one retained player owns its mailbox and GPU imports"
    )]
    pub fn update(
        &mut self,
        world: &str,
        player: usize,
        wanted: [u32; 2],
        surfaces: &mut Surfaces,
        ui: &mut NativeUi,
        gpu: &Gpu,
    ) -> ara_ui::Result<bool> {
        let Some(state) = surfaces
            .surfaces(world)
            .into_iter()
            .find(|state| state.player == player)
        else {
            return Ok(false);
        };
        let generation = state.generation;
        self.size = state.size;
        let known = |surface: &Option<PlaySurface>| {
            surface
                .as_ref()
                .is_some_and(|surface| surface.generation() == generation)
        };
        if !known(&self.shown) && !known(&self.pending) {
            // SAFETY: The UI owns this device; the imported surface is released before that device.
            let imported = unsafe { ui.import_play_surface(gpu, state.surface) }?;
            if let Some(old) = self.pending.replace(imported) {
                ui.close_play_surface(old)?;
            }
        }
        let mut changed = false;
        if let Some(frame) = surfaces.take(world, player) {
            if self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.generation() == frame.generation)
                && let Some(next) = self.pending.take()
                && let Some(old) = self.shown.replace(next)
                && let Some(previous) = ui.close_play_surface(old)?
            {
                surfaces.release(world, player, previous);
            }
            if let Some(surface) = self
                .shown
                .as_mut()
                .filter(|surface| surface.generation() == frame.generation)
            {
                let texture = *self.texture.get_or_insert_with(|| player_texture(player));
                if let Some(previous) = surface.show(ui, texture, frame)? {
                    surfaces.release(world, player, previous);
                }
                changed = true;
                self.live = true;
                let now = Instant::now();
                self.frames.push_back(now);
                while self
                    .frames
                    .front()
                    .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(1))
                {
                    self.frames.pop_front();
                }
                let span = self
                    .frames
                    .front()
                    .map_or(0., |at| now.duration_since(*at).as_secs_f64());
                self.fps = if span < 0.25 {
                    0
                } else {
                    ((self.frames.len() - 1) as f64 / span).round() as u32
                };
            } else {
                surfaces.release(world, player, frame);
            }
        }
        if wanted[0] >= 16
            && wanted[1] >= 16
            && wanted != self.size
            && self
                .resized
                .is_none_or(|at| at.elapsed() > Duration::from_millis(150))
        {
            surfaces.resize(world, player, wanted[0], wanted[1]);
            self.resized = Some(Instant::now());
        }
        Ok(changed)
    }
    pub fn close(
        &mut self,
        world: Option<&str>,
        player: usize,
        surfaces: Option<&Surfaces>,
        ui: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        for surface in [self.shown.take(), self.pending.take()]
            .into_iter()
            .flatten()
        {
            if let Some(frame) = ui.close_play_surface(surface)?
                && let (Some(world), Some(surfaces)) = (world, surfaces)
            {
                surfaces.release(world, player, frame);
            }
        }
        self.frames.clear();
        self.live = false;
        Ok(())
    }
}
