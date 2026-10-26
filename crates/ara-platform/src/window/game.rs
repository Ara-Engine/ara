//! Game windows use desktop fullscreen without changing the monitor's mode.
use super::*;

/// Standalone game presentation; editor and multiplayer tool windows opt into windowed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GameWindowMode {
    /// Borderless fullscreen at the desktop extent, eligible for direct scanout.
    #[default]
    Borderless,
    /// A resizable native window at the requested extent.
    Windowed,
}

impl Window {
    /// Create a game window without flashing an intermediate decorated window.
    /// Hidden probes always retain their requested windowed extent and never show.
    pub fn game(title: &str, size: [u32; 2], mode: GameWindowMode, hidden: bool) -> Result<Self> {
        let window = Self::hidden(title, size)?;
        if hidden {
            return Ok(window);
        }
        // SAFETY: This main-thread owner retains its SDL window. A null mode
        // requests the desktop mode, so no monitor resolution change is made.
        unsafe {
            if mode == GameWindowMode::Borderless {
                ensure!(
                    SDL_SetWindowFullscreenMode(window.handle, std::ptr::null()),
                    "SDL desktop fullscreen mode: {}",
                    error()
                );
                ensure!(
                    SDL_SetWindowFullscreen(window.handle, true),
                    "SDL desktop fullscreen: {}",
                    error()
                );
            }
            ensure!(SDL_ShowWindow(window.handle), "SDL show: {}", error());
            // Resolve asynchronous fullscreen changes before sizing the loading
            // swapchain, so its handoff never recreates it at the first game frame.
            ensure!(
                SDL_SyncWindow(window.handle),
                "SDL window sync: {}",
                error()
            );
        }
        Ok(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "hidden SDL window; run explicitly on the desktop host"]
    fn hidden_game_modes_never_enter_fullscreen_or_change_the_requested_extent() -> Result<()> {
        for mode in [GameWindowMode::Borderless, GameWindowMode::Windowed] {
            let window = Window::game("Hidden game", [320, 180], mode, true)?;
            assert_eq!(window.size()?, [320, 180]);
            // SAFETY: This main-thread test retains its window through the query.
            let flags = unsafe { SDL_GetWindowFlags(window.handle) };
            assert_eq!((flags & SDL_WINDOW_HIDDEN).0, SDL_WINDOW_HIDDEN.0);
            assert_eq!((flags & SDL_WINDOW_FULLSCREEN).0, 0);
        }
        Ok(())
    }
}
