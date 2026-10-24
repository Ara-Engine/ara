//! SDL text input (IME), clipboard and dropped folders for the Hub window.
//!
//! Copied in minimal form from `ara-editor/src/native/mod.rs` (text watch,
//! clipboard) rather than shared, so the editor stays untouched while its panels
//! are rebuilt. Extract both into a shared native-UI crate later.
use ara_ui::{
    Ui,
    input::{InputEvent, InputState},
};
use ara_ui_text::{Clipboard, EditEvent};
use sdl3_sys::{clipboard::*, events::*, keyboard::*, stdinc::SDL_free, video::*};
use std::{
    ffi::{CStr, CString, c_void},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

/// What the event watch forwards from SDL's queue.
enum Watched {
    Edit(EditEvent),
    Drop(PathBuf),
}
struct Watch {
    send: Sender<Watched>,
}
unsafe extern "C" fn watch(user: *mut c_void, event: *mut SDL_Event) -> bool {
    // SAFETY: SDL retains the boxed Watch until RemoveEventWatch; only the tagged member
    // matching the event type is read, and strings are copied before returning.
    unsafe {
        let state = &*user.cast::<Watch>();
        let event = &*event;
        let ty = event.r#type;
        if ty == SDL_EVENT_TEXT_INPUT.0 && !event.text.text.is_null() {
            let text = CStr::from_ptr(event.text.text)
                .to_string_lossy()
                .into_owned();
            let _ = state.send.send(Watched::Edit(EditEvent::ImeCommit(text)));
        } else if ty == SDL_EVENT_TEXT_EDITING.0 && !event.edit.text.is_null() {
            let text = CStr::from_ptr(event.edit.text)
                .to_string_lossy()
                .into_owned();
            let begin = event.edit.start.max(0) as usize;
            let count = event.edit.length.max(0) as usize;
            let offset = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
            let selection = offset(begin)..offset(begin + count);
            let _ = state
                .send
                .send(Watched::Edit(EditEvent::ImePreedit { text, selection }));
        } else if ty == SDL_EVENT_DROP_FILE.0 && !event.drop.data.is_null() {
            let path = CStr::from_ptr(event.drop.data)
                .to_string_lossy()
                .into_owned();
            let _ = state.send.send(Watched::Drop(PathBuf::from(path)));
        }
    }
    true
}

/// Text input, IME and drop routing for one SDL window.
pub struct NativeInput {
    window: *mut SDL_Window,
    watch: Box<Watch>,
    events: Receiver<Watched>,
    drops: Vec<PathBuf>,
    composing: bool,
    text_active: bool,
}
impl NativeInput {
    /// Install the event watch for `window` (a live SDL window on this thread).
    pub fn attach(window: *mut SDL_Window) -> anyhow::Result<Self> {
        let (send, events) = mpsc::channel();
        let mut watch = Box::new(Watch { send });
        anyhow::ensure!(
            // SAFETY: The boxed userdata stays at a stable address until Drop removes the watch.
            unsafe { SDL_AddEventWatch(Some(self::watch), (&mut *watch as *mut Watch).cast()) },
            "SDL event watch failed"
        );
        Ok(Self {
            window,
            watch,
            events,
            drops: Vec::new(),
            composing: false,
            text_active: false,
        })
    }
    /// Logical-to-physical UI scale of the window's display.
    pub fn dpi(&self) -> f32 {
        // SAFETY: Live main-thread SDL window.
        unsafe { SDL_GetWindowDisplayScale(self.window).max(0.25) }
    }
    /// SDL window points per logical UI pixel, for pointer conversion.
    pub fn points_per_logical(&self, dpi: f32) -> f32 {
        // SAFETY: Live main-thread SDL window.
        unsafe { SDL_GetWindowPixelDensity(self.window).max(0.25) / dpi }
    }
    /// Folders dropped onto the window since the last call.
    pub fn take_drops(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.drops)
    }
    /// Start or stop OS text input to match the focused control.
    pub fn sync_text_input(&mut self, ui: &Ui, input: &InputState, dpi: f32) -> anyhow::Result<()> {
        let field = input
            .focused()
            .and_then(|id| ui.by_id(id))
            .filter(|i| matches!(ui.nodes[*i].element.tag.as_str(), "field" | "input"));
        // SAFETY: Live main-thread window; IME geometry uses window units.
        unsafe {
            if field.is_some() != self.text_active {
                let ok = if field.is_some() {
                    SDL_StartTextInput(self.window)
                } else {
                    SDL_StopTextInput(self.window)
                };
                anyhow::ensure!(ok, "SDL text input state failed");
                self.text_active = field.is_some();
            }
            if let Some(i) = field {
                let r = ui.nodes[i].rect;
                let scale = dpi / SDL_GetWindowPixelDensity(self.window).max(0.25);
                let area = sdl3_sys::rect::SDL_Rect {
                    x: (r.x * scale) as i32,
                    y: (r.y * scale) as i32,
                    w: (r.w * scale) as i32,
                    h: (r.h * scale) as i32,
                };
                SDL_SetTextInputArea(self.window, &area, 0);
            }
        }
        Ok(())
    }
    /// Deliver queued text/IME edits to the focused field; collect drops.
    pub fn pump(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<bool> {
        let mut changed = false;
        let mut clipboard = OsClipboard;
        while let Ok(watched) = self.events.try_recv() {
            let edit = match watched {
                Watched::Drop(path) => {
                    self.drops.push(path);
                    changed = true;
                    continue;
                }
                Watched::Edit(edit) => edit,
            };
            match &edit {
                EditEvent::ImePreedit { text, .. } => {
                    if !self.composing && !text.is_empty() {
                        input.event(ui, InputEvent::Edit(EditEvent::ImeStart), &mut clipboard)?;
                        self.composing = true;
                    }
                    if text.is_empty() {
                        input.event(ui, InputEvent::Edit(EditEvent::ImeCancel), &mut clipboard)?;
                        self.composing = false;
                        continue;
                    }
                }
                EditEvent::ImeCommit(_) => self.composing = false,
                _ => {}
            }
            input.event(ui, InputEvent::Edit(edit), &mut clipboard)?;
            changed = true;
        }
        Ok(changed)
    }
}
impl Drop for NativeInput {
    fn drop(&mut self) {
        // SAFETY: RemoveEventWatch synchronizes callback removal while the userdata and
        // window are still live.
        unsafe {
            SDL_RemoveEventWatch(Some(watch), (&mut *self.watch as *mut Watch).cast());
            if self.text_active {
                SDL_StopTextInput(self.window);
            }
        }
    }
}

/// The OS clipboard through SDL.
pub struct OsClipboard;
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
        let text = CString::new(text)?;
        // SAFETY: SDL copies the nul-terminated string before returning.
        if unsafe { SDL_SetClipboardText(text.as_ptr()) } {
            Ok(())
        } else {
            Err("SDL clipboard write failed".into())
        }
    }
}
