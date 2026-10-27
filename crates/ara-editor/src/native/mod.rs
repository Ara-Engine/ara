//! Editor-only SDL clipboard, IME, monitor scale and Windows accessibility adapters.
#[cfg(windows)]
mod accessibility;
mod coordinates;
pub(crate) mod frame;
#[cfg(windows)]
mod pacing;
use ara_ui::{Ui, input::InputState};
use ara_ui_text::Clipboard;
#[cfg(not(windows))]
use sdl3_sys::events::SDL_WaitEventTimeout;
use sdl3_sys::{keyboard::*, stdinc::SDL_free, video::*};
use std::ffi::CStr;

/// Native event failures belong in diagnostics, never the window's fatal path.
fn recover_input<T>(result: ara_ui::Result<T>) -> Option<T> {
    result
        .map_err(|error| {
            let message = error.to_string();
            let code = message
                .split(':')
                .next()
                .and_then(ara_diag::Code::parse)
                .unwrap_or(ara_diag::codes::IO);
            eprintln!("{code}: Dropped editor input event: {message}");
            ara_diag::Diagnostic::warning(code, format!("Dropped editor input event: {message}"))
                .emit();
        })
        .ok()
}

pub(crate) struct Native {
    window: *mut SDL_Window,
    text_active: bool,
    /// Window currently receiving OS text input (main or a floating window).
    text_window: *mut SDL_Window,
    #[cfg(windows)]
    accessibility: accessibility::Accessibility,
    #[cfg(windows)]
    timer: pacing::FrameTimer,
}
impl Native {
    /// Fix a verification window's physical client extent independently of desktop scaling.
    pub fn verification_extent(&self, pixels: [u32; 2]) -> anyhow::Result<()> {
        use sdl3_sys::video::{
            SDL_GetWindowSizeInPixels, SDL_RestoreWindow, SDL_SetWindowBordered,
            SDL_SetWindowPosition, SDL_SetWindowResizable, SDL_SetWindowSize, SDL_SyncWindow,
        };
        // SAFETY: This is the main thread's live SDL window; all output pointers are valid.
        unsafe {
            anyhow::ensure!(
                SDL_RestoreWindow(self.window),
                "restore verification window"
            );
            anyhow::ensure!(
                SDL_SetWindowResizable(self.window, false),
                "lock verification extent"
            );
            anyhow::ensure!(
                SDL_SetWindowBordered(self.window, false),
                "remove verification chrome"
            );
            anyhow::ensure!(
                SDL_SetWindowPosition(self.window, 0, 0),
                "position verification window"
            );
            let density = SDL_GetWindowPixelDensity(self.window).max(0.25);
            anyhow::ensure!(
                SDL_SetWindowSize(
                    self.window,
                    (pixels[0] as f32 / density).round() as i32,
                    (pixels[1] as f32 / density).round() as i32
                ),
                "set verification extent"
            );
            anyhow::ensure!(
                SDL_SyncWindow(self.window),
                "synchronize verification window"
            );
            let (mut width, mut height) = (0, 0);
            anyhow::ensure!(
                SDL_GetWindowSizeInPixels(self.window, &mut width, &mut height),
                "read verification extent"
            );
            anyhow::ensure!(
                [width as u32, height as u32] == pixels,
                "verification requested {pixels:?}, native client is {width}x{height}"
            );
        }
        Ok(())
    }
    pub fn wait(&self, remaining: std::time::Duration) -> anyhow::Result<()> {
        #[cfg(windows)]
        self.timer.wait(remaining)?;
        #[cfg(not(windows))]
        // SAFETY: Main-thread SDL pump; a null event pointer leaves queued input for Window::poll.
        unsafe {
            SDL_WaitEventTimeout(
                std::ptr::null_mut(),
                remaining.as_millis().min(i32::MAX as u128) as i32,
            );
        }
        Ok(())
    }
    pub fn attach(title: &str) -> anyhow::Result<Self> {
        #[cfg(windows)]
        let timer = pacing::FrameTimer::new()?;
        let mut count = 0;
        // SAFETY: SDL video is initialized by ara-platform; its window array is freed with SDL_free.
        let window = unsafe {
            let windows = SDL_GetWindows(&mut count);
            anyhow::ensure!(!windows.is_null(), "SDL window inventory failed");
            let found = std::slice::from_raw_parts(windows, count.max(0) as usize)
                .iter()
                .copied()
                .find(|w| {
                    !w.is_null()
                        && CStr::from_ptr(SDL_GetWindowTitle(*w)).to_string_lossy() == title
                });
            SDL_free(windows.cast());
            found.ok_or_else(|| anyhow::anyhow!("editor SDL window was not found"))?
        };
        #[cfg(windows)]
        let accessibility = accessibility::Accessibility::new(window)?;
        Ok(Self {
            window,
            text_active: false,
            text_window: window,
            #[cfg(windows)]
            accessibility,
            #[cfg(windows)]
            timer,
        })
    }
    pub fn dpi(&self) -> f32 {
        // SAFETY: Live main-thread SDL window. SDL updates scale as it crosses monitor boundaries.
        unsafe { SDL_GetWindowDisplayScale(self.window).max(0.25) }
    }
    /// Refresh native IME and accessibility geometry at the current display scale.
    pub fn update(&mut self, ui: &Ui, input: &InputState, dpi: f32) -> anyhow::Result<()> {
        self.text_input(self.window, ui, input, dpi)?;
        #[cfg(windows)]
        self.accessibility.update(ui, input.focused(), dpi);
        Ok(())
    }
    /// Start or stop OS text input (IME) for the window whose surface has focus.
    pub fn text_input(
        &mut self,
        window: *mut SDL_Window,
        ui: &Ui,
        input: &InputState,
        dpi: f32,
    ) -> anyhow::Result<()> {
        let field = input
            .focused()
            .and_then(|id| ui.by_id(id))
            .filter(|i| matches!(ui.nodes[*i].element.tag.as_str(), "field" | "input"));
        // SAFETY: Both windows are live on this main thread; IME geometry uses logical window units.
        unsafe {
            if window != self.text_window && self.text_active {
                SDL_StopTextInput(self.text_window);
                self.text_active = false;
            }
            self.text_window = window;
            if field.is_some() != self.text_active {
                let ok = if field.is_some() {
                    SDL_StartTextInput(window)
                } else {
                    SDL_StopTextInput(window)
                };
                if !ok {
                    let _ = recover_input::<()>(Err("SDL text input state failed".into()));
                    return Ok(());
                }
                self.text_active = field.is_some();
            }
            if let Some(i) = field {
                let r = ui.nodes[i].rect;
                let scale = dpi / SDL_GetWindowPixelDensity(window).max(0.25);
                let area = sdl3_sys::rect::SDL_Rect {
                    x: (r.x * scale) as i32,
                    y: (r.y * scale) as i32,
                    w: (r.w * scale) as i32,
                    h: (r.h * scale) as i32,
                };
                SDL_SetTextInputArea(window, &area, 0);
            }
        }
        Ok(())
    }
    pub fn input(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<bool> {
        #[cfg(windows)]
        {
            Ok(
                recover_input(self.accessibility.actions(ui, input, &mut OsClipboard))
                    .unwrap_or(false),
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (ui, input);
            Ok(false)
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: The platform still owns this main-thread window during teardown.
        unsafe {
            if self.text_active {
                SDL_StopTextInput(self.text_window);
            }
        }
    }
}
pub(crate) struct OsClipboard;
impl Clipboard for OsClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        ara_platform::text::read_clipboard().map_err(|error| error.to_string().into())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        let result: ara_ui_text::Result<()> =
            ara_platform::text::write_clipboard(text).map_err(|error| error.to_string().into());
        // Command-driven copies may intentionally ignore the return value.
        if let Err(error) = &result {
            eprintln!(
                "{}: Editor clipboard event failed: {error}",
                ara_diag::codes::IO
            );
            ara_diag::Diagnostic::warning(
                ara_diag::codes::IO,
                format!("Editor clipboard event failed: {error}"),
            )
            .emit();
        }
        result
    }
}
/// Convert an event from any editor window into that window's logical pixels.
pub(crate) fn logical_event_for(
    window: *mut SDL_Window,
    event: ara_platform::Event,
    dpi: f32,
) -> ara_platform::Event {
    // SAFETY: The caller passes a live main-thread SDL window.
    let density = unsafe { SDL_GetWindowPixelDensity(window).max(0.25) };
    coordinates::logical_event(event, density, dpi)
}
/// Window points to editor logical pixels for a window (desktop drag tracking).
pub(crate) fn points_to_logical(window: *mut SDL_Window, dpi: f32) -> f32 {
    // SAFETY: The caller passes a live main-thread SDL window.
    unsafe { SDL_GetWindowPixelDensity(window).max(0.25) / dpi }
}
/// SDL id of the window with keyboard focus, or 0.
pub(crate) fn keyboard_focus() -> u32 {
    // SAFETY: Main-thread keyboard state query; a null result means no focus.
    unsafe {
        let window = SDL_GetKeyboardFocus();
        if window.is_null() {
            0
        } else {
            SDL_GetWindowID(window).0
        }
    }
}
