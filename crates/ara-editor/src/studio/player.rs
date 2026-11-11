use crate::gpu::UiDevice as Gpu;
use ara_editor_gpu::{NativeUi, PlaySurface};
use std::time::{Duration, Instant};
use ara_play_host::embed::Surfaces;
use std::collections::VecDeque;

/// Quiet time after a change of the wanted size before the next resize goes out (the
/// first change after a quiet spell goes at once, a lone dock change waits for nothing):
/// each player resize reallocates its targets and restarts temporal history, so a
/// splitter drag resizes once it rests instead of several times a second. The frame
/// shown meanwhile is scaled to the panel (`viewport::draw_view_image`).
const RESIZE_REST: Duration = Duration::from_millis(90);
/// A wanted size that keeps changing (a long drag) still resizes this often.
const RESIZE_EVERY: Duration = Duration::from_millis(400);

/// One player's imported surfaces, the frame it shows and its resize requests.
#[derive(Default)]
pub(crate) struct PlayerView {
    shown: Option<PlaySurface>,
    pending: Option<PlaySurface>,
    /// Size and UI scale last requested from the player, and when.
    requested: Option<([u32; 2], f32, Instant)>,
    /// The wanted size and scale, and since when.
    wanted: Option<([u32; 2], f32, Instant)>,
    frames: VecDeque<Instant>,
    /// Pixels of the frame shown (its surface generation), never of a newer surface
    /// whose first frame has not arrived: the view draws the image it holds.
    size: [u32; 2],
    fps: u32,
    live: bool,
    texture: Option<u32>,
}
/// Distinct native UI image for an embedded player (0 authority, 1–16 clients).
pub fn player_texture(player: usize) -> u32 {
    ara_ui::source::stable_id(&format!("game-view-{player}")) as u32 & 0x7fff_ffff
}

/// What an embedded player's view shows: whether its frames are live, their size and
/// their rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlayerInfo {
    /// A frame from the player is on screen.
    pub live: bool,
    /// The frame size in pixels.
    pub size: [u32; 2],
    /// Frames presented in the last second.
    pub fps: u32,
}
impl PlayerView {
    /// What this view shows now.
    pub fn info(&self) -> PlayerInfo {
        PlayerInfo {
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
        scale: f32,
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
                self.size = surface.size();
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
        let current = self.pending.as_ref().or(self.shown.as_ref());
        let current = current.map(PlaySurface::size);
        if let Some((size, scale)) = self.resize_due(wanted, scale, current, Instant::now()) {
            surfaces.resize(world, player, size, scale);
        }
        Ok(changed)
    }
    /// The resize to request now, if any: the wanted size once it rests
    /// ([`RESIZE_REST`]), or every [`RESIZE_EVERY`] while it keeps changing. A request
    /// is never repeated while unanswered (each one recreates the player's ring).
    /// `current` is the newest imported surface's size: before any request, the
    /// surface Play opened with (its embed size and scale, the panel's at the time).
    fn resize_due(
        &mut self,
        wanted: [u32; 2],
        scale: f32,
        current: Option<[u32; 2]>,
        now: Instant,
    ) -> Option<([u32; 2], f32)> {
        if wanted.iter().any(|v| *v < 16) {
            return None;
        }
        let since = match self.wanted {
            Some((size, s, at)) if size == wanted && s == scale => at,
            _ => {
                self.wanted = Some((wanted, scale, now));
                now
            }
        };
        let satisfied = match self.requested {
            Some((size, s, _)) => (size, s) == (wanted, scale),
            None => current.is_none_or(|size| size == wanted),
        };
        let due = now.duration_since(since) >= RESIZE_REST
            || self
                .requested
                .is_none_or(|(_, _, at)| now.duration_since(at) >= RESIZE_EVERY);
        if satisfied || !due {
            return None;
        }
        self.requested = Some((wanted, scale, now));
        Some((wanted, scale))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizes_wait_for_the_size_to_rest_and_are_never_repeated_unanswered() {
        let mut view = PlayerView::default();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        // The embed size already matches: nothing to ask.
        assert_eq!(
            view.resize_due([800, 450], 1., Some([800, 450]), at(0)),
            None
        );
        // A stale embed size (the panel was never laid out) resizes at once.
        let mut stale = PlayerView::default();
        assert_eq!(
            stale.resize_due([1600, 900], 1., Some([1280, 720]), at(0)),
            Some(([1600, 900], 1.))
        );
        // Unanswered, the same request never goes out again.
        assert_eq!(
            stale.resize_due([1600, 900], 1., Some([1280, 720]), at(900)),
            None
        );
        // A drag: its first change goes out at once (a lone dock change waits for
        // nothing); the frames after it wait until the size rests...
        assert_eq!(
            stale.resize_due([1500, 900], 1., Some([1600, 900]), at(1000)),
            Some(([1500, 900], 1.))
        );
        for (ms, w) in [(1016, 1490), (1033, 1480)] {
            assert_eq!(
                stale.resize_due([w, 900], 1., Some([1600, 900]), at(ms)),
                None
            );
        }
        assert_eq!(
            stale.resize_due([1480, 900], 1., Some([1600, 900]), at(1033 + 90)),
            Some(([1480, 900], 1.))
        );
        // ...or every RESIZE_EVERY while it keeps moving.
        let mut ms = 1200;
        let mut sent = Vec::new();
        while ms < 2100 {
            if let Some((size, _)) = stale.resize_due([1000 + ms as u32, 900], 1., None, at(ms)) {
                sent.push(size[0]);
            }
            ms += 16;
        }
        assert_eq!(sent.len(), 2, "{sent:?}");
        // A new UI scale is a new request.
        assert!(stale.resize_due([3100, 900], 2., None, at(3000)).is_some());
        // Too small to render: nothing.
        assert_eq!(view.resize_due([8, 8], 1., None, at(5000)), None);
    }
}
