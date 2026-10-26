//! Main-thread cursor ownership. SDL lacks grab cursors, so those use native monochrome hands.
use crate::Window;
use anyhow::{Result, ensure};
use sdl3_sys::mouse::*;
use std::{collections::BTreeMap, ptr::NonNull};

/// Native pointer shape requested by an interactive surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemCursor {
    /// Standard selection arrow.
    #[default]
    Arrow,
    /// Editable text I-beam.
    Text,
    /// Clickable link or button hand.
    Hand,
    /// Horizontal resize.
    ResizeEw,
    /// Vertical resize.
    ResizeNs,
    /// Northwest/southeast resize.
    ResizeNwse,
    /// Northeast/southwest resize.
    ResizeNesw,
    /// Move in any direction.
    Move,
    /// Operation unavailable here.
    NotAllowed,
    /// Open hand over draggable content.
    Grab,
    /// Closed hand while dragging.
    Grabbing,
}

#[derive(Default)]
pub(crate) struct Cursors(BTreeMap<SystemCursor, NonNull<SDL_Cursor>>);
impl Cursors {
    pub(crate) fn clear(&mut self) {
        for cursor in std::mem::take(&mut self.0).into_values() {
            // SAFETY: Each cached cursor is uniquely owned and destroyed before SDL video quits.
            unsafe { SDL_DestroyCursor(cursor.as_ptr()) };
        }
    }
    fn get(&mut self, shape: SystemCursor) -> Result<NonNull<SDL_Cursor>> {
        let cursors = &mut self.0;
        let cursor = if let Some(cursor) = cursors.get(&shape) {
            *cursor
        } else {
            let system = match shape {
                SystemCursor::Arrow => SDL_SYSTEM_CURSOR_DEFAULT,
                SystemCursor::Text => SDL_SYSTEM_CURSOR_TEXT,
                SystemCursor::Hand => SDL_SYSTEM_CURSOR_POINTER,
                SystemCursor::ResizeEw => SDL_SYSTEM_CURSOR_EW_RESIZE,
                SystemCursor::ResizeNs => SDL_SYSTEM_CURSOR_NS_RESIZE,
                SystemCursor::ResizeNwse => SDL_SYSTEM_CURSOR_NWSE_RESIZE,
                SystemCursor::ResizeNesw => SDL_SYSTEM_CURSOR_NESW_RESIZE,
                SystemCursor::Move => SDL_SYSTEM_CURSOR_MOVE,
                SystemCursor::NotAllowed => SDL_SYSTEM_CURSOR_NOT_ALLOWED,
                SystemCursor::Grab | SystemCursor::Grabbing => SDL_SYSTEM_CURSOR_POINTER,
            };
            // SAFETY: The owning Window retains SDL video on this thread; SDL copies bitmap data.
            let raw = unsafe {
                if matches!(shape, SystemCursor::Grab | SystemCursor::Grabbing) {
                    let (data, mask) = hand_bitmap(shape == SystemCursor::Grabbing);
                    SDL_CreateCursor(data.as_ptr(), mask.as_ptr(), 16, 16, 7, 7)
                } else {
                    SDL_CreateSystemCursor(system)
                }
            };
            let cursor = NonNull::new(raw).ok_or_else(|| {
                anyhow::anyhow!("SDL {shape:?} cursor: {}", crate::window::error())
            })?;
            cursors.insert(shape, cursor);
            cursor
        };
        Ok(cursor)
    }
}
impl Window {
    /// Select a lazily cached cursor on the SDL main thread. SDL applies it to the mouse focus.
    pub fn set_cursor(&mut self, shape: SystemCursor) -> Result<()> {
        let cursor = self.cursors.get(shape)?;
        // SAFETY: The cache owns this live cursor until before video shutdown.
        unsafe {
            if SDL_GetCursor() != cursor.as_ptr() {
                ensure!(
                    SDL_SetCursor(cursor.as_ptr()),
                    "SDL cursor selection: {}",
                    crate::window::error()
                );
            }
        }
        Ok(())
    }
}

fn hand_bitmap(closed: bool) -> ([u8; 32], [u8; 32]) {
    let open = [
        "                ",
        "      XX        ",
        "   XX X.X XX    ",
        "   X.XX.XX.X XX ",
        "   X..X..X.XX.X ",
        "   X..X..X..X.X ",
        " XX X.......X.X ",
        " X.XX.........X ",
        " X..X.........X ",
        "  X...........X ",
        "   X..........X ",
        "    X........X  ",
        "    X........X  ",
        "     X......X   ",
        "     X......X   ",
        "     XXXXXXXX   ",
    ];
    let fist = [
        "                ",
        "                ",
        "                ",
        "                ",
        "     XXXXXX     ",
        "   XX..X..XXX   ",
        "   X...X..X..X  ",
        "   X.........X  ",
        "  XX.........X  ",
        "  X..........X  ",
        "   X.........X  ",
        "    X.......X   ",
        "    X.......X   ",
        "     X.....X    ",
        "     X.....X    ",
        "     XXXXXXX    ",
    ];
    let (mut data, mut mask) = ([0; 32], [0; 32]);
    for (y, row) in if closed { fist } else { open }.iter().enumerate() {
        for (x, pixel) in row.bytes().enumerate() {
            let bit = 0x80 >> (x % 8);
            if pixel != b' ' {
                mask[y * 2 + x / 8] |= bit;
            }
            if pixel == b'X' {
                data[y * 2 + x / 8] |= bit;
            }
        }
    }
    (data, mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires SDL Vulkan video; creates only a hidden window and never selects a cursor"]
    fn hidden_window_loads_and_reuses_native_cursors_without_changing_selection() -> Result<()> {
        let mut window = Window::hidden("Cursor resource verification", [64, 64])?;
        // SAFETY: The hidden window retains video on this test's owning thread.
        let selected = unsafe { SDL_GetCursor() };
        for shape in [
            SystemCursor::Arrow,
            SystemCursor::Text,
            SystemCursor::Hand,
            SystemCursor::ResizeEw,
            SystemCursor::ResizeNs,
            SystemCursor::ResizeNwse,
            SystemCursor::ResizeNesw,
            SystemCursor::Move,
            SystemCursor::NotAllowed,
            SystemCursor::Grab,
            SystemCursor::Grabbing,
        ] {
            let first = window.cursors.get(shape)?;
            assert_eq!(first, window.cursors.get(shape)?, "cursor is cached");
        }
        assert_eq!(window.cursors.0.len(), 11);
        // SAFETY: As above; querying selection does not change the desktop cursor.
        assert_eq!(selected, unsafe { SDL_GetCursor() });
        Ok(())
    }
}
