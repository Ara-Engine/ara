//! Game windows: borderless desktop fullscreen by default (no monitor mode change),
//! windowed, or exclusive fullscreen; chosen at creation and switched at runtime.
use super::*;
use std::mem::MaybeUninit;

/// Standalone game presentation; editor and multiplayer tool windows opt into windowed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GameWindowMode {
    /// Borderless fullscreen at the desktop extent, eligible for direct scanout.
    #[default]
    Borderless,
    /// A resizable native window at the requested extent.
    Windowed,
    /// Exclusive fullscreen at the display mode closest to the requested extent, at the
    /// desktop refresh rate: the monitor switches modes.
    Fullscreen,
}

impl Window {
    /// Create a game window without flashing an intermediate decorated window.
    /// Hidden probes always retain their requested windowed extent and never show.
    pub fn game(title: &str, size: [u32; 2], mode: GameWindowMode, hidden: bool) -> Result<Self> {
        // A game takes the click that activates its window (macOS otherwise drops it,
        // so the first menu click after a Hub or Finder launch did nothing).
        // SAFETY: Static NUL-terminated hint name and value, set before the window.
        unsafe {
            sdl3_sys::hints::SDL_SetHint(
                sdl3_sys::hints::SDL_HINT_MOUSE_FOCUS_CLICKTHROUGH,
                c"1".as_ptr(),
            );
        }
        let window = Self::hidden(title, size)?;
        if hidden {
            return Ok(window);
        }
        window.enter(mode, size)?;
        // SAFETY: This main-thread owner retains its SDL window.
        unsafe {
            ensure!(SDL_ShowWindow(window.handle), "SDL show: {}", error());
            // Ask for keyboard focus too; a launched game should not need a first click.
            SDL_RaiseWindow(window.handle);
        }
        // Resolve asynchronous fullscreen changes before sizing the loading swapchain,
        // so its handoff never recreates it at the first game frame.
        window.sync()?;
        Ok(window)
    }

    /// Switch a game window's mode at runtime (a game's Settings menu). `size` is the
    /// windowed extent, or the extent exclusive fullscreen picks the closest display mode
    /// to; borderless ignores it. Presenters follow the new drawable size on their next
    /// frame. Hidden probe windows keep their requested extent and never enter fullscreen.
    pub fn set_game_mode(&self, mode: GameWindowMode, size: [u32; 2]) -> Result<()> {
        // SAFETY: Live window on its owning thread.
        let flags = unsafe { SDL_GetWindowFlags(self.handle) };
        if flags & SDL_WINDOW_HIDDEN != SDL_WindowFlags(0) {
            return Ok(());
        }
        self.enter(mode, size)?;
        self.sync()
    }

    /// The window's current mode (a requested change still in flight reads as made).
    pub fn game_mode(&self) -> GameWindowMode {
        // SAFETY: Live window on its owning thread; the mode pointer is only null-checked.
        unsafe {
            if SDL_GetWindowFlags(self.handle) & SDL_WINDOW_FULLSCREEN == SDL_WindowFlags(0) {
                GameWindowMode::Windowed
            } else if SDL_GetWindowFullscreenMode(self.handle).is_null() {
                GameWindowMode::Borderless
            } else {
                GameWindowMode::Fullscreen
            }
        }
    }

    /// Request `mode`; callers synchronize (`sync`) once the window is shown.
    fn enter(&self, mode: GameWindowMode, size: [u32; 2]) -> Result<()> {
        ensure!(
            !size.contains(&0) && size.iter().all(|n| *n <= i32::MAX as u32),
            "Invalid window size"
        );
        // SAFETY: This main-thread owner retains its SDL window. A null fullscreen mode
        // requests the desktop mode (no monitor change); the closest exclusive mode is
        // fully written by SDL before it is read, and SDL copies it.
        unsafe {
            match mode {
                GameWindowMode::Windowed => {
                    if SDL_GetWindowFlags(self.handle) & SDL_WINDOW_FULLSCREEN != SDL_WindowFlags(0)
                    {
                        ensure!(
                            SDL_SetWindowFullscreen(self.handle, false),
                            "SDL leave fullscreen: {}",
                            error()
                        );
                        // Leaving fullscreen restores the last windowed extent first.
                        self.sync()?;
                    }
                    self.set_size(size)?;
                }
                GameWindowMode::Borderless => {
                    ensure!(
                        SDL_SetWindowFullscreenMode(self.handle, std::ptr::null()),
                        "SDL desktop fullscreen mode: {}",
                        error()
                    );
                    ensure!(
                        SDL_SetWindowFullscreen(self.handle, true),
                        "SDL desktop fullscreen: {}",
                        error()
                    );
                }
                GameWindowMode::Fullscreen => {
                    let display = SDL_GetDisplayForWindow(self.handle);
                    let mut closest = MaybeUninit::<SDL_DisplayMode>::uninit();
                    ensure!(
                        SDL_GetClosestFullscreenDisplayMode(
                            display,
                            size[0] as i32,
                            size[1] as i32,
                            0.,
                            true,
                            closest.as_mut_ptr(),
                        ),
                        "SDL closest display mode to {}x{}: {}",
                        size[0],
                        size[1],
                        error()
                    );
                    ensure!(
                        SDL_SetWindowFullscreenMode(self.handle, closest.as_ptr()),
                        "SDL exclusive fullscreen mode: {}",
                        error()
                    );
                    ensure!(
                        SDL_SetWindowFullscreen(self.handle, true),
                        "SDL exclusive fullscreen: {}",
                        error()
                    );
                }
            }
        }
        Ok(())
    }

    /// Wait for the window system to apply pending mode, size and position changes.
    fn sync(&self) -> Result<()> {
        // SAFETY: Live window on its owning thread.
        unsafe {
            ensure!(SDL_SyncWindow(self.handle), "SDL window sync: {}", error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "hidden SDL window; run explicitly on the desktop host"]
    fn hidden_game_modes_never_enter_fullscreen_or_change_the_requested_extent() -> Result<()> {
        let modes = [
            GameWindowMode::Borderless,
            GameWindowMode::Windowed,
            GameWindowMode::Fullscreen,
        ];
        for mode in modes {
            let window = Window::game("Hidden game", [320, 180], mode, true)?;
            for switch in modes {
                window.set_game_mode(switch, [640, 360])?;
            }
            assert_eq!(window.size()?, [320, 180]);
            assert_eq!(window.game_mode(), GameWindowMode::Windowed);
            // SAFETY: This main-thread test retains its window through the query.
            let flags = unsafe { SDL_GetWindowFlags(window.handle) };
            assert_eq!((flags & SDL_WINDOW_HIDDEN).0, SDL_WINDOW_HIDDEN.0);
            assert_eq!((flags & SDL_WINDOW_FULLSCREEN).0, 0);
        }
        Ok(())
    }
}
