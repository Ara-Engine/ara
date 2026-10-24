//! Editor-only SDL clipboard, IME, monitor scale and Windows accessibility adapters.
#[cfg(windows)]
mod accessibility;
mod coordinates;
pub(crate) mod frame;
#[cfg(windows)]
mod pacing;
use ara_ui::{
    Ui,
    input::{InputEvent, InputState},
};
use ara_ui_text::{Clipboard, EditEvent};
use sdl3_sys::{clipboard::*, events::*, keyboard::*, stdinc::SDL_free, video::*};
use std::{
    ffi::{CStr, CString, c_void},
    sync::mpsc::{self, Receiver, Sender},
};

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

struct Watch {
    window: SDL_WindowID,
    send: Sender<EditEvent>,
}
unsafe extern "C" fn text_watch(user: *mut c_void, event: *mut SDL_Event) -> bool {
    // SAFETY: SDL retains the boxed Watch until RemoveEventWatch; only the event's tagged member is read.
    unsafe {
        let state = &*user.cast::<Watch>();
        let event = &*event;
        let _ = state.window;
        if event.r#type == SDL_EVENT_TEXT_INPUT.0 && !event.text.text.is_null() {
            let text = CStr::from_ptr(event.text.text)
                .to_string_lossy()
                .into_owned();
            let _ = state.send.send(EditEvent::ImeCommit(text));
        }
        if event.r#type == SDL_EVENT_TEXT_EDITING.0 && !event.edit.text.is_null() {
            let text = CStr::from_ptr(event.edit.text)
                .to_string_lossy()
                .into_owned();
            let begin = event.edit.start.max(0) as usize;
            let count = event.edit.length.max(0) as usize;
            let offset = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
            let selection = offset(begin)..offset(begin + count);
            let _ = state.send.send(EditEvent::ImePreedit { text, selection });
        }
    }
    true
}
pub(crate) struct Native {
    window: *mut SDL_Window,
    watch: Box<Watch>,
    edits: Receiver<EditEvent>,
    composing: bool,
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
        let (send, edits) = mpsc::channel();
        // SAFETY: Window is owned by ara-platform on this thread and remains live through Native teardown.
        let window_id = unsafe { SDL_GetWindowID(window) };
        let mut watch = Box::new(Watch {
            window: window_id,
            send,
        });
        // SAFETY: Box keeps userdata stable; callback is removed before userdata is freed.
        anyhow::ensure!(
            // SAFETY: The stable userdata is removed before it can be freed.
            unsafe { SDL_AddEventWatch(Some(text_watch), (&mut *watch as *mut Watch).cast()) },
            "SDL event watch failed"
        );
        #[cfg(windows)]
        let accessibility = match accessibility::Accessibility::new(window) {
            Ok(a) => a,
            Err(e) => {
                // SAFETY: Undo the installed callback before returning and dropping its userdata.
                unsafe {
                    SDL_RemoveEventWatch(Some(text_watch), (&mut *watch as *mut Watch).cast());
                }
                return Err(e);
            }
        };
        Ok(Self {
            window,
            watch,
            edits,
            composing: false,
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
        let mut changed = false;
        let mut clipboard = OsClipboard;
        while let Ok(edit) = self.edits.try_recv() {
            let result = (|| -> ara_ui::Result<bool> {
                match &edit {
                    EditEvent::ImePreedit { text, .. } => {
                        if !self.composing && !text.is_empty() {
                            input.event(
                                ui,
                                InputEvent::Edit(EditEvent::ImeStart),
                                &mut clipboard,
                            )?;
                            self.composing = true;
                        }
                        if text.is_empty() {
                            input.event(
                                ui,
                                InputEvent::Edit(EditEvent::ImeCancel),
                                &mut clipboard,
                            )?;
                            self.composing = false;
                            return Ok(true);
                        }
                    }
                    EditEvent::ImeCommit(_) => self.composing = false,
                    _ => {}
                }
                input.event(ui, InputEvent::Edit(edit), &mut clipboard)?;
                Ok(true)
            })();
            if let Some(applied) = recover_input(result) {
                changed |= applied;
            } else {
                self.composing = false;
                let _ = recover_input(input.event(
                    ui,
                    InputEvent::Edit(EditEvent::ImeCancel),
                    &mut clipboard,
                ));
            }
        }
        #[cfg(windows)]
        {
            changed |= recover_input(self.accessibility.actions(ui, input, &mut clipboard))
                .unwrap_or(false);
        }
        Ok(changed)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: RemoveEventWatch synchronizes callback removal while userdata and window are still live.
        unsafe {
            SDL_RemoveEventWatch(Some(text_watch), (&mut *self.watch as *mut Watch).cast());
            if self.text_active {
                SDL_StopTextInput(self.text_window);
            }
        }
    }
}
pub(crate) struct OsClipboard;
impl Clipboard for OsClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        // SAFETY: SDL returns an owned UTF-8 string, released exactly once with its allocator.
        unsafe {
            let ptr = SDL_GetClipboardText();
            if ptr.is_null() {
                return Err("SDL clipboard read failed".into());
            }
            let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            SDL_free(ptr.cast());
            Ok(text)
        }
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        let result = (|| -> ara_ui_text::Result<()> {
            let text = CString::new(text)?;
            // SAFETY: SDL copies the nul-terminated string before returning.
            if unsafe { SDL_SetClipboardText(text.as_ptr()) } {
                Ok(())
            } else {
                Err("SDL clipboard write failed".into())
            }
        })();
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
