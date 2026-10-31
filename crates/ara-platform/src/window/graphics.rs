//! Display enumeration and exact exclusive modes for a game's graphics menu.
use super::*;

/// One supported exclusive mode on a connected monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonitorMode {
    /// SDL display identity (stable until disconnect).
    pub monitor: u32,
    /// Physical width in pixels.
    pub width: u32,
    /// Physical height in pixels.
    pub height: u32,
    /// Refresh rate in millihertz.
    pub refresh_rate_millihz: u32,
}

impl Window {
    /// The connected display currently containing the window; zero if unavailable.
    pub fn current_monitor(&self) -> u32 {
        // SAFETY: this owner retains its live window on the SDL video thread.
        unsafe { SDL_GetDisplayForWindow(self.handle).0 }
    }
    /// Enumerate connected monitor modes on the SDL main thread. IDs are not
    /// array indexes; callers must re-query after a display disconnect.
    pub fn supported_modes(&self) -> Result<Vec<MonitorMode>> {
        let mut count = 0;
        let mut result = Vec::new();
        // SAFETY: this owner retains SDL video; SDL owns all returned mode records.
        // Both allocated pointer arrays are freed with SDL_free after copying.
        unsafe {
            let displays = SDL_GetDisplays(&mut count);
            ensure!(!displays.is_null(), "SDL displays: {}", error());
            for id in std::slice::from_raw_parts(displays, count.max(0) as usize) {
                let mut mode_count = 0;
                let modes = SDL_GetFullscreenDisplayModes(*id, &mut mode_count);
                if modes.is_null() {
                    continue;
                }
                for pointer in std::slice::from_raw_parts(modes, mode_count.max(0) as usize) {
                    if let Some(mode) = pointer.as_ref()
                        && mode.w > 0
                        && mode.h > 0
                    {
                        result.push(MonitorMode {
                            monitor: id.0,
                            width: mode.w as u32,
                            height: mode.h as u32,
                            refresh_rate_millihz: (mode.refresh_rate * 1000.).round().max(0.)
                                as u32,
                        });
                    }
                }
                sdl3_sys::stdinc::SDL_free(modes.cast());
            }
            sdl3_sys::stdinc::SDL_free(displays.cast());
        }
        result.sort_unstable();
        result.dedup();
        Ok(result)
    }

    /// Select a monitor and exact supported exclusive refresh rate. Hidden probes
    /// never move, resize, or switch monitor modes on the user's desktop.
    pub fn set_display_mode(
        &self,
        mode: GameWindowMode,
        size: [u32; 2],
        monitor: u32,
        refresh_millihz: u32,
    ) -> Result<()> {
        ensure!(
            !size.contains(&0) && size.iter().all(|v| *v <= i32::MAX as u32),
            "Invalid physical display size"
        );
        ensure!(
            refresh_millihz == 0 || (15_000..=1_000_000).contains(&refresh_millihz),
            "Invalid display refresh rate"
        );
        // SAFETY: all SDL access stays on the retaining window's owning thread.
        unsafe {
            if SDL_GetWindowFlags(self.handle) & SDL_WINDOW_HIDDEN != SDL_WindowFlags(0) {
                return Ok(());
            }
            let display = if monitor == 0 {
                SDL_GetDisplayForWindow(self.handle)
            } else {
                SDL_DisplayID(monitor)
            };
            if monitor != 0 {
                let mut bounds = std::mem::MaybeUninit::uninit();
                ensure!(
                    SDL_GetDisplayBounds(display, bounds.as_mut_ptr()),
                    "SDL monitor: {}",
                    error()
                );
                let bounds = bounds.assume_init();
                ensure!(
                    SDL_SetWindowFullscreen(self.handle, false),
                    "SDL leave fullscreen: {}",
                    error()
                );
                self.sync()?;
                self.set_position([bounds.x, bounds.y])?;
                self.sync()?;
            }
            if mode == GameWindowMode::Fullscreen {
                let mut closest = std::mem::MaybeUninit::uninit();
                ensure!(
                    SDL_GetClosestFullscreenDisplayMode(
                        display,
                        size[0] as i32,
                        size[1] as i32,
                        refresh_millihz as f32 / 1000.,
                        true,
                        closest.as_mut_ptr()
                    ),
                    "SDL exclusive mode: {}",
                    error()
                );
                ensure!(
                    SDL_SetWindowFullscreenMode(self.handle, closest.as_ptr()),
                    "SDL fullscreen mode: {}",
                    error()
                );
                ensure!(
                    SDL_SetWindowFullscreen(self.handle, true),
                    "SDL fullscreen: {}",
                    error()
                );
                self.sync()
            } else {
                let logical_size = if mode == GameWindowMode::Windowed {
                    let density = self.pixel_density().max(1.);
                    size.map(|v| ((v as f32 / density).round() as u32).max(1))
                } else {
                    size
                };
                self.set_game_mode(mode, logical_size)
            }
        }
    }
}
