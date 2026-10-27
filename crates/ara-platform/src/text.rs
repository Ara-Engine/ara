//! Main-thread SDL text input and the actual OS clipboard. Coordinates supplied
//! for an IME area are already in window points; mapping remains host-owned.
use crate::Window;
use anyhow::{Result, ensure};
use std::ffi::{CStr, CString};

/// Read owned UTF-8 text from the native clipboard on the SDL main thread.
pub fn read_clipboard() -> Result<String> {
    // SAFETY: SDL returns an owned NUL-terminated allocation, copied before free.
    unsafe {
        let text = sdl3_sys::clipboard::SDL_GetClipboardText();
        ensure!(!text.is_null(), "SDL clipboard read failed");
        let result = CStr::from_ptr(text).to_string_lossy().into_owned();
        sdl3_sys::stdinc::SDL_free(text.cast());
        Ok(result)
    }
}

/// Write UTF-8 text to the native clipboard on the SDL main thread.
pub fn write_clipboard(text: &str) -> Result<()> {
    let text = CString::new(text)?;
    // SAFETY: SDL copies this live NUL-terminated string before returning.
    ensure!(
        unsafe { sdl3_sys::clipboard::SDL_SetClipboardText(text.as_ptr()) },
        "SDL clipboard write failed"
    );
    Ok(())
}

impl Window {
    /// Enable native text/IME only while a retained text field owns keyboard focus.
    /// Repeating the current state does not restart an in-progress composition.
    pub fn set_text_input(&self, enabled: bool) -> Result<()> {
        // SAFETY: this Window owns its live SDL handle on the main thread.
        unsafe {
            if sdl3_sys::keyboard::SDL_TextInputActive(self.handle) == enabled {
                return Ok(());
            }
            let changed = if enabled {
                sdl3_sys::keyboard::SDL_StartTextInput(self.handle)
            } else {
                sdl3_sys::keyboard::SDL_StopTextInput(self.handle)
            };
            ensure!(changed, "SDL text input state failed");
        }
        Ok(())
    }

    /// Position the IME candidate window beside a field, in native window points.
    pub fn set_text_input_area(&self, area: [i32; 4], cursor: i32) -> Result<()> {
        ensure!(
            area[2] >= 0 && area[3] >= 0,
            "IME area has a negative extent"
        );
        let rect = sdl3_sys::rect::SDL_Rect {
            x: area[0],
            y: area[1],
            w: area[2],
            h: area[3],
        };
        // SAFETY: the window is live, and SDL copies the stack rectangle.
        ensure!(
            unsafe { sdl3_sys::keyboard::SDL_SetTextInputArea(self.handle, &rect, cursor.max(0)) },
            "SDL IME area failed"
        );
        Ok(())
    }
}
