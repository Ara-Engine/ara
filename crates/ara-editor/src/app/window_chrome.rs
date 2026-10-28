//! Windows snap-layout support for a client-drawn maximize button.
use ara_platform::Window;
use ara_ui::paint::Box2;
#[cfg(windows)]
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::ScreenToClient,
    UI::{Shell::*, WindowsAndMessaging::*},
};
#[cfg(windows)]
const SUBCLASS_ID: usize = 0x41524143;
#[cfg(windows)]
#[derive(Default)]
struct State {
    button: Box2,
    pressed: bool,
    window: *mut sdl3_sys::video::SDL_Window,
}
/// Removes its Windows subclass before the SDL window is destroyed.
pub(super) struct SnapCaption {
    #[cfg(windows)]
    hwnd: HWND,
    #[cfg(windows)]
    state: Box<State>,
}
impl SnapCaption {
    pub fn new(window: &Window) -> ara_ui::Result<Self> {
        #[cfg(windows)]
        {
            // SAFETY: SDL owns this HWND; the window outlives this subclass guard.
            let hwnd = unsafe {
                HWND(sdl3_sys::properties::SDL_GetPointerProperty(
                    sdl3_sys::video::SDL_GetWindowProperties(window.raw()),
                    sdl3_sys::video::SDL_PROP_WINDOW_WIN32_HWND_POINTER,
                    std::ptr::null_mut(),
                ))
            };
            if hwnd.0.is_null() {
                return Err("SDL window has no HWND for snap layouts".into());
            }
            let mut state = Box::new(State {
                window: window.raw(),
                ..Default::default()
            });
            // SAFETY: Stable Box and matching callback signature; Drop removes the hook.
            if !unsafe {
                SetWindowSubclass(
                    hwnd,
                    Some(subclass),
                    SUBCLASS_ID,
                    (&raw mut *state) as usize,
                )
            }
            .as_bool()
            {
                return Err("Could not attach the native snap-layout caption".into());
            }
            Ok(Self { hwnd, state })
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            Ok(Self {})
        }
    }
    /// Read the actual HWND hit-test contract without showing the window.
    pub fn verify(
        &self,
        window: &Window,
        shell: &crate::shell::Shell,
        dpi: f32,
    ) -> ara_ui::Result<serde_json::Value> {
        #[cfg(windows)]
        {
            use windows::Win32::Graphics::Gdi::ClientToScreen;
            let scale = 1. / crate::native::points_to_logical(window.raw(), dpi);
            let point = |id: &str| -> ara_ui::Result<[f32; 2]> {
                let r = shell.ui.nodes[shell
                    .shown_node(id)
                    .ok_or("Chrome verification control missing")?]
                .rect;
                Ok([(r.x + r.w * 0.5) * scale, (r.y + r.h * 0.5) * scale])
            };
            let hit = |p: [f32; 2]| -> ara_ui::Result<isize> {
                let mut native = POINT {
                    x: p[0].round() as i32,
                    y: p[1].round() as i32,
                };
                // SAFETY: HWND is live and these messages only query geometry, with no desktop input.
                unsafe {
                    if !ClientToScreen(self.hwnd, &mut native).as_bool() {
                        return Err("ClientToScreen failed".into());
                    }
                    let l = LPARAM(
                        ((native.x as u16 as u32) | ((native.y as u16 as u32) << 16)) as isize,
                    );
                    Ok(SendMessageW(self.hwnd, WM_NCHITTEST, Some(WPARAM(0)), Some(l)).0)
                }
            };
            let max = hit(point("window-maximize")?)?;
            let menu_id = shell
                .menu_header_node("File")
                .ok_or("Chrome verification menu missing")?;
            let menu = hit(point(menu_id)?)?;
            let left =
                shell.ui.nodes[shell.shown_node("titlebar-left").ok_or("Caption left")?].rect;
            let caption = hit([(left.x + left.w - 8.) * scale, 18. * scale])?;
            let resize = hit([1., 1.])?;
            if max != HTMAXBUTTON as isize
                || menu != HTCLIENT as isize
                || caption != HTCAPTION as isize
                || resize != HTTOPLEFT as isize
            {
                return Err(format!("Native caption hit test: maximize={max}, menu={menu}, caption={caption}, resize={resize}").into());
            }
            Ok(
                serde_json::json!({"maximize":max,"menu":menu,"caption":caption,"resizeCorner":resize}),
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (window, shell, dpi);
            Ok(serde_json::json!({"sdl":true}))
        }
    }
    /// Maximize button bounds in native client pixels (DPI and UI scale included).
    pub fn update(&mut self, button: Box2) {
        #[cfg(windows)]
        {
            self.state.button = button;
        }
        #[cfg(not(windows))]
        {
            let _ = button;
        }
    }
}
#[cfg(windows)]
impl Drop for SnapCaption {
    fn drop(&mut self) {
        // SAFETY: Window outlives this guard, and the Box remains alive through removal.
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(subclass), SUBCLASS_ID);
        }
    }
}
#[cfg(windows)]
fn contains(r: Box2, p: POINT) -> bool {
    p.x as f32 >= r.x && p.y as f32 >= r.y && (p.x as f32) < r.x + r.w && (p.y as f32) < r.y + r.h
}
#[cfg(windows)]
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: The main-thread guard keeps the Box alive until subclass removal.
    // Only copy state out across calls that can synchronously dispatch another message.
    unsafe {
        if matches!(
            message,
            WM_NCHITTEST | WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCMOUSEMOVE
        ) {
            let point = POINT {
                x: (l.0 as u16 as i16) as i32,
                y: ((l.0 >> 16) as u16 as i16) as i32,
            };
            let mut local = point;
            let _ = ScreenToClient(hwnd, &mut local);
            let inside = contains((*(data as *const State)).button, local);
            if message == WM_NCHITTEST && inside {
                return LRESULT(HTMAXBUTTON as isize);
            }
            if message == WM_NCLBUTTONDOWN && w.0 == HTMAXBUTTON as usize {
                (*(data as *mut State)).pressed = true;
                return LRESULT(0);
            }
            if message == WM_NCLBUTTONUP && w.0 == HTMAXBUTTON as usize {
                let pressed = std::mem::take(&mut (*(data as *mut State)).pressed);
                let window = (*(data as *const State)).window;
                if pressed && inside {
                    use sdl3_sys::video::*;
                    if SDL_GetWindowFlags(window) & SDL_WINDOW_MAXIMIZED != 0 {
                        SDL_RestoreWindow(window);
                    } else {
                        SDL_MaximizeWindow(window);
                    }
                }
                return LRESULT(0);
            }
            if message == WM_NCMOUSEMOVE
                && (w.0 == HTMAXBUTTON as usize || w.0 == HTCAPTION as usize)
            {
                // Let Windows observe the HTMAXBUTTON hover (the native snap chooser),
                // and clear the retained button hover when moving back into the caption.
                use sdl3_sys::{events::*, video::SDL_GetWindowID};
                let mut event: SDL_Event = std::mem::zeroed();
                event.motion.r#type = SDL_EVENT_MOUSE_MOTION;
                event.motion.windowID = SDL_GetWindowID((*(data as *const State)).window);
                event.motion.x = local.x as f32;
                event.motion.y = local.y as f32;
                SDL_PushEvent(&mut event);
            }
        }
        DefSubclassProc(hwnd, message, w, l)
    }
}
