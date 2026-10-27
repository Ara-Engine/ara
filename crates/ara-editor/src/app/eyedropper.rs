//! Feeds the colour picker's eyedropper each frame: the pixels of the last rendered
//! frame around the pointer, or, on Windows, the screen pixel under the cursor when it
//! is over another window (GDI), where a click picks it and a right click cancels.
use super::*;

/// The global mouse buttons last frame, so a click outside the window is an edge.
#[derive(Default)]
pub(super) struct Buttons {
    #[cfg(windows)]
    left: bool,
    #[cfg(windows)]
    right: bool,
}

/// Sample for an active eyedropper; true when the preview changed (redraw). `screen`
/// allows sampling the desktop outside the window; offscreen runs (verification,
/// captures) never read the user's screen or mouse.
pub(super) fn sample(
    editor: &mut Editor,
    shell: &mut Shell,
    gpu: &mut NativeUi,
    window: &Window,
    dpi: f32,
    screen: bool,
    buttons: &mut Buttons,
) -> ara_ui::Result<bool> {
    if !shell.eyedropper_active() {
        *buttons = Buttons::default();
        return Ok(false);
    }
    #[cfg(windows)]
    if screen && let Some(screen) = screen::outside(window) {
        let (left, right) = (screen.left && !buttons.left, screen.right && !buttons.right);
        *buttons = Buttons {
            left: screen.left,
            right: screen.right,
        };
        let changed = shell.eyedropper_screen(screen.rgb)?;
        if right {
            shell.cancel_eyedropper();
            shell.rebuild(editor)?;
            return Ok(true);
        }
        if left && !shell.eyedropper_held() {
            shell.pick_eyedropper(editor)?;
            return Ok(true);
        }
        return Ok(changed);
    }
    #[cfg(not(windows))]
    let _ = (editor, window, screen);
    *buttons = Buttons::default();
    let Some((point, side)) = shell.eyedropper_point() else {
        return Ok(false);
    };
    let pixel = [(point[0] * dpi) as u32, (point[1] * dpi) as u32];
    let origin = [
        pixel[0].saturating_sub(side / 2),
        pixel[1].saturating_sub(side / 2),
    ];
    // A frame that cannot be read back (none rendered yet) just previews nothing.
    let Ok(block) = gpu.pixels_in(origin, [side, side]) else {
        return Ok(false);
    };
    shell.eyedropper_sample(&block, [pixel[0] - origin[0], pixel[1] - origin[1]], point)
}

#[cfg(windows)]
mod screen {
    use super::*;
    use ::windows::Win32::Foundation::{HWND, POINT};
    use ::windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use ::windows::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetCursorPos, WindowFromPoint,
    };

    /// The screen under the cursor while it is over another window.
    pub(super) struct Screen {
        /// The pixel's display sRGB.
        pub rgb: [u8; 3],
        /// The global left and right buttons are down.
        pub left: bool,
        pub right: bool,
    }

    /// The pixel under the cursor and the global buttons, unless the cursor is over the
    /// editor's main window (whose own frame is sampled instead).
    pub(super) fn outside(window: &Window) -> Option<Screen> {
        // SAFETY: SDL owns the main window's HWND for the editor's lifetime.
        let main = unsafe {
            HWND(sdl3_sys::properties::SDL_GetPointerProperty(
                sdl3_sys::video::SDL_GetWindowProperties(window.raw()),
                sdl3_sys::video::SDL_PROP_WINDOW_WIN32_HWND_POINTER,
                std::ptr::null_mut(),
            ))
        };
        let mut cursor = POINT::default();
        // SAFETY: Plain Win32 queries on this thread with a valid out pointer; the
        // screen DC is released before returning.
        unsafe {
            GetCursorPos(&mut cursor).ok()?;
            let under = WindowFromPoint(cursor);
            if !main.0.is_null() && GetAncestor(under, GA_ROOT) == main {
                return None;
            }
            let dc = GetDC(None);
            let color = GetPixel(dc, cursor.x, cursor.y);
            ReleaseDC(None, dc);
            // COLORREF is 0x00BBGGRR; CLR_INVALID (all ones) is a pixel off every screen.
            if color.0 == u32::MAX {
                return None;
            }
            let [r, g, b, _] = color.0.to_le_bytes();
            Some(Screen {
                rgb: [r, g, b],
                left: GetAsyncKeyState(i32::from(VK_LBUTTON.0)) < 0,
                right: GetAsyncKeyState(i32::from(VK_RBUTTON.0)) < 0,
            })
        }
    }
}
