//! DWM frame for floating native captions and the custom main window: colours,
//! Windows 11 rounded corners, border and shadow.
//! The main title bar is client-drawn (D28); both window types share theme tokens.
use sdl3_sys::video::SDL_Window;

/// Apply the theme's frame colors to a native window.
pub(crate) fn style(window: *mut SDL_Window, light: bool) {
    #[cfg(windows)]
    windows_frame(window, light);
    #[cfg(not(windows))]
    let _ = (window, light);
}
#[cfg(windows)]
fn windows_frame(window: *mut SDL_Window, light: bool) {
    use sdl3_sys::{properties::SDL_GetPointerProperty, video::*};
    use windows::Win32::{Foundation::HWND, Graphics::Dwm::*, UI::Controls::MARGINS};
    let tokens = if light {
        ara_ui::theme::EditorTheme::Light
    } else {
        ara_ui::theme::EditorTheme::Dark
    }
    .tokens()
    .unwrap_or_default();
    // COLORREF is 0x00BBGGRR.
    let color = |name: &str, fallback: u32| -> u32 {
        tokens
            .get(name)
            .and_then(|hex| u32::from_str_radix(hex.trim_start_matches('#'), 16).ok())
            .map(|rgb| ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff))
            .unwrap_or(fallback)
    };
    // SAFETY: The SDL window is live on this thread; the HWND property is owned by SDL
    // and DWM attributes only read the provided values for their duration.
    unsafe {
        let hwnd = SDL_GetPointerProperty(
            SDL_GetWindowProperties(window),
            SDL_PROP_WINDOW_WIN32_HWND_POINTER,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            return;
        }
        let hwnd = HWND(hwnd);
        let dark: i32 = i32::from(!light);
        let set = |attribute: DWMWINDOWATTRIBUTE, value: u32| {
            // Older Windows builds lack caption colors; the frame stays native there.
            let _ = DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&raw const value).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        };
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            std::mem::size_of::<i32>() as u32,
        );
        set(DWMWA_CAPTION_COLOR, color("--chrome", 0x000a0a0a));
        // The system border colour, as every native window has.
        set(DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT);
        // Windows 11 rounds and shadows a client-drawn (borderless) window only on
        // request; the one-pixel frame extension is what lets DWM draw the shadow.
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0 as u32);
        let margins = MARGINS {
            cxLeftWidth: 1,
            cxRightWidth: 1,
            cyTopHeight: 1,
            cyBottomHeight: 1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &raw const margins);
        set(DWMWA_TEXT_COLOR, color("--foreground", 0x00f5f5f5));
    }
}
