use anyhow::{Context, Result, ensure};
use sdl3_sys::{
    error::SDL_GetError,
    events::*,
    init::*,
    pixels::SDL_PIXELFORMAT_RGBA32,
    surface::{SDL_CreateSurfaceFrom, SDL_DestroySurface},
    video::*,
    vulkan::*,
};
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
};

/// Normalized native events. The simulation host maps input into tick actions.
#[derive(Debug)]
pub enum Event {
    /// Native close request.
    Quit,
    /// Pixel extent changed; zero denotes minimization.
    Resize([u32; 2]),
    /// Keyboard scancode, press/release and repeat state.
    Key {
        /// SDL physical key scancode.
        scancode: i32,
        /// Pressed versus released.
        pressed: bool,
        /// OS key repeat.
        repeat: bool,
    },
    /// Mouse position and relative motion, in window coordinates.
    MouseMotion {
        /// Absolute position.
        position: [f32; 2],
        /// Relative motion.
        delta: [f32; 2],
    },
    /// Mouse button press/release.
    MouseButton {
        /// SDL button number.
        button: u8,
        /// Pressed versus released.
        pressed: bool,
    },
    /// Mouse wheel motion.
    Wheel([f32; 2]),
}
/// The Ara app icon (64 px PNG exported by `ara-brand` from `assets/brand/ara-icon.svg`).
const ARA_ICON_PNG: &[u8] = include_bytes!("../../../assets/brand/png/ara-icon-64.png");

/// Main-thread-owned resizable SDL3 Vulkan window and video subsystem reference.
pub struct Window {
    pub(crate) handle: *mut SDL_Window,
    pub(crate) chrome: Option<Box<crate::chrome::WindowChrome>>,
    pub(crate) cursors: crate::cursor::Cursors,
    _main_thread: PhantomData<*mut ()>,
}
impl Window {
    /// Open a native resizable Vulkan window; creation is confined to the SDL main thread.
    /// The window starts with the Ara app icon; games replace it with [`Window::set_icon`].
    pub fn new(title: &str, size: [u32; 2]) -> Result<Self> {
        Self::create(title, size, false)
    }
    /// Like [`Window::new`], but never shown: automated runs (captures, verification)
    /// render into offscreen targets and read pixels back, without putting a window on
    /// the user's desktop.
    pub fn hidden(title: &str, size: [u32; 2]) -> Result<Self> {
        Self::create(title, size, true)
    }
    fn create(title: &str, size: [u32; 2], hidden: bool) -> Result<Self> {
        let title = CString::new(title)?;
        ensure!(
            !size.contains(&0) && size.iter().all(|n| *n <= i32::MAX as u32),
            "Invalid window extent"
        );
        // SAFETY: SDL owns no Rust pointers; this object retains and destroys the window/subsystem.
        unsafe {
            ensure!(SDL_InitSubSystem(SDL_INIT_VIDEO), "SDL init: {}", error());
            let handle = SDL_CreateWindow(
                title.as_ptr(),
                size[0] as i32,
                size[1] as i32,
                SDL_WINDOW_VULKAN
                    | SDL_WINDOW_RESIZABLE
                    | SDL_WINDOW_HIGH_PIXEL_DENSITY
                    | if hidden {
                        SDL_WINDOW_HIDDEN
                    } else {
                        SDL_WindowFlags(0)
                    },
            );
            if handle.is_null() {
                SDL_QuitSubSystem(SDL_INIT_VIDEO);
                anyhow::bail!("SDL window: {}", error());
            }
            let window = Self {
                handle,
                chrome: None,
                cursors: crate::cursor::Cursors::default(),
                _main_thread: PhantomData,
            };
            // The icon is cosmetic: drivers without window icons (Wayland) still get a window.
            let _ = decode_icon(ARA_ICON_PNG).and_then(|(rgba, size)| window.set_icon(&rgba, size));
            Ok(window)
        }
    }
    /// Update the native title on the SDL main thread (for example startup progress).
    pub fn set_title(&self, title: &str) -> Result<()> {
        let title = CString::new(title)?;
        // SAFETY: this owner retains the window; SDL copies the live UTF-8 string.
        let updated = unsafe { SDL_SetWindowTitle(self.handle, title.as_ptr()) };
        ensure!(updated, "SDL title: {}", error());
        Ok(())
    }

    /// Set the title-bar and taskbar icon from straight-alpha RGBA8 pixels (`size` = width, height).
    pub fn set_icon(&self, rgba: &[u8], size: [u32; 2]) -> Result<()> {
        ensure!(
            !size.contains(&0)
                && size.iter().all(|n| *n <= 4096)
                && rgba.len() == size[0] as usize * size[1] as usize * 4,
            "Invalid icon: {}x{} with {} bytes",
            size[0],
            size[1],
            rgba.len()
        );
        // SAFETY: The surface borrows `rgba` read-only for this call only; SDL_SetWindowIcon
        // copies it into its own ARGB8888 surface before we destroy ours.
        unsafe {
            let surface = SDL_CreateSurfaceFrom(
                size[0] as i32,
                size[1] as i32,
                SDL_PIXELFORMAT_RGBA32,
                rgba.as_ptr().cast_mut().cast(),
                size[0] as i32 * 4,
            );
            ensure!(!surface.is_null(), "SDL icon surface: {}", error());
            let set = SDL_SetWindowIcon(self.handle, surface);
            SDL_DestroySurface(surface);
            ensure!(set, "SDL icon: {}", error());
        }
        Ok(())
    }
    /// Instance extensions required by the active SDL Vulkan driver.
    pub fn extensions(&self) -> Result<Vec<CString>> {
        let mut count = 0;
        // SAFETY: SDL video is live; its static extension strings outlive Vulkan instance creation.
        unsafe {
            let names = SDL_Vulkan_GetInstanceExtensions(&mut count);
            ensure!(!names.is_null(), "SDL extensions: {}", error());
            Ok(std::slice::from_raw_parts(names, count as usize)
                .iter()
                .map(|p| CStr::from_ptr(*p).to_owned())
                .collect())
        }
    }
    /// Current drawable size in physical pixels.
    pub fn size(&self) -> Result<[u32; 2]> {
        let mut w = 0;
        let mut h = 0;
        // SAFETY: Window is owned on the creating thread; SDL writes both sizes.
        unsafe {
            ensure!(
                SDL_GetWindowSizeInPixels(self.handle, &mut w, &mut h),
                "SDL size: {}",
                error()
            );
        }
        Ok([w.max(0) as u32, h.max(0) as u32])
    }
    /// Change logical window size, used by tools and the resize verification route.
    pub fn set_size(&self, size: [u32; 2]) -> Result<()> {
        ensure!(
            !size.contains(&0) && size.iter().all(|n| *n <= i32::MAX as u32),
            "Invalid window size"
        );
        // SAFETY: Live window on its owning thread.
        unsafe {
            ensure!(
                SDL_SetWindowSize(self.handle, size[0] as i32, size[1] as i32),
                "SDL resize: {}",
                error()
            );
        }
        Ok(())
    }
    /// Open an owned tool window at a desktop position: it stays above `parent`
    /// and minimizes with it (D23 floating panels). `rect` is x, y, width, height
    /// in desktop logical pixels.
    pub fn floating(title: &str, rect: [i32; 4], parent: &Window) -> Result<Self> {
        Self::floating_with(title, rect, parent, false)
    }
    /// A floating window that is never shown (see [`Window::hidden`]).
    pub fn floating_hidden(title: &str, rect: [i32; 4], parent: &Window) -> Result<Self> {
        Self::floating_with(title, rect, parent, true)
    }
    fn floating_with(title: &str, rect: [i32; 4], parent: &Window, hidden: bool) -> Result<Self> {
        let window = Self::create(
            title,
            [rect[2].max(64) as u32, rect[3].max(48) as u32],
            hidden,
        )?;
        // SAFETY: Both windows are live on this (main) thread.
        unsafe {
            ensure!(
                SDL_SetWindowParent(window.handle, parent.handle),
                "SDL parent: {}",
                error()
            );
            ensure!(
                SDL_SetWindowPosition(window.handle, rect[0], rect[1]),
                "SDL position: {}",
                error()
            );
        }
        Ok(window)
    }
    /// SDL window id; tagged events name the window they belong to.
    pub fn id(&self) -> u32 {
        // SAFETY: Live window on its owning thread.
        unsafe { SDL_GetWindowID(self.handle).0 }
    }
    /// Raw SDL handle for host adapters (IME, accessibility, OS frame styling).
    /// The pointer is valid while this `Window` lives, on its owning thread.
    pub fn raw(&self) -> *mut SDL_Window {
        self.handle
    }
    /// Outer desktop rectangle (x, y, width, height) in logical pixels.
    pub fn rect(&self) -> Result<[i32; 4]> {
        let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
        // SAFETY: Live window on its owning thread; SDL writes all four values.
        unsafe {
            ensure!(
                SDL_GetWindowPosition(self.handle, &mut x, &mut y),
                "SDL position: {}",
                error()
            );
            ensure!(
                SDL_GetWindowSize(self.handle, &mut w, &mut h),
                "SDL size: {}",
                error()
            );
        }
        Ok([x, y, w, h])
    }
    /// Move the window to a desktop position in logical pixels.
    pub fn set_position(&self, position: [i32; 2]) -> Result<()> {
        // SAFETY: Live window on its owning thread.
        unsafe {
            ensure!(
                SDL_SetWindowPosition(self.handle, position[0], position[1]),
                "SDL position: {}",
                error()
            );
        }
        Ok(())
    }
    /// Bring the window to the front and give it keyboard focus (hidden windows stay
    /// hidden).
    pub fn raise(&self) {
        // SAFETY: Live window on its owning thread.
        unsafe {
            if SDL_GetWindowFlags(self.handle) & SDL_WINDOW_HIDDEN == SDL_WindowFlags(0) {
                SDL_RaiseWindow(self.handle);
            }
        }
    }
    /// Toggle desktop full screen.
    pub fn toggle_fullscreen(&self) -> Result<()> {
        // SAFETY: Live window on its owning thread.
        unsafe {
            let full = SDL_GetWindowFlags(self.handle) & SDL_WINDOW_FULLSCREEN != 0;
            ensure!(
                SDL_SetWindowFullscreen(self.handle, !full),
                "SDL fullscreen: {}",
                error()
            );
        }
        Ok(())
    }
    /// Display scale of the monitor showing this window.
    pub fn display_scale(&self) -> f32 {
        // SAFETY: Live window on its owning thread.
        unsafe { SDL_GetWindowDisplayScale(self.handle).max(0.25) }
    }
    /// Usable bounds (x, y, width, height) of every connected display.
    pub fn displays() -> Vec<[i32; 4]> {
        let mut out = Vec::new();
        let mut count = 0;
        // SAFETY: SDL video is initialized by a live Window; the id array is freed once.
        unsafe {
            let ids = SDL_GetDisplays(&mut count);
            if ids.is_null() {
                return out;
            }
            for id in std::slice::from_raw_parts(ids, count.max(0) as usize) {
                let mut rect = sdl3_sys::rect::SDL_Rect {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                };
                if SDL_GetDisplayUsableBounds(*id, &mut rect) {
                    out.push([rect.x, rect.y, rect.w, rect.h]);
                }
            }
            sdl3_sys::stdinc::SDL_free(ids.cast());
        }
        out
    }
    /// Pointer position on the desktop, independent of any window.
    pub fn global_mouse() -> [f32; 2] {
        let (mut x, mut y) = (0., 0.);
        // SAFETY: SDL writes both coordinates; mouse state is global.
        unsafe {
            sdl3_sys::mouse::SDL_GetGlobalMouseState(&mut x, &mut y);
        }
        [x, y]
    }
    /// Drain native input and lifecycle events without owning gameplay state.
    pub fn poll(&self) -> Result<Vec<Event>> {
        Ok(Self::poll_tagged()?.into_iter().map(|(_, e)| e).collect())
    }
    /// Drain events for every window, tagged with the SDL window id (0 for
    /// application events). A close request is reported as `Quit` for its window.
    pub fn poll_tagged() -> Result<Vec<(u32, Event)>> {
        let mut events = Vec::new();
        // SAFETY: SDL initializes the union; only the member matching its type is read.
        unsafe {
            let mut event = std::mem::MaybeUninit::<SDL_Event>::uninit();
            while SDL_PollEvent(event.as_mut_ptr()) {
                let e = event.assume_init_ref();
                let ty = e.r#type;
                if ty == SDL_EVENT_QUIT.0 {
                    events.push((0, Event::Quit));
                } else if ty == SDL_EVENT_WINDOW_CLOSE_REQUESTED.0 {
                    events.push((e.window.windowID.0, Event::Quit));
                } else if ty == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED.0
                    || ty == SDL_EVENT_WINDOW_RESTORED.0
                    || ty == SDL_EVENT_WINDOW_MINIMIZED.0
                {
                    let (mut w, mut h) = (0, 0);
                    let window = SDL_GetWindowFromID(e.window.windowID);
                    if !window.is_null() {
                        SDL_GetWindowSizeInPixels(window, &mut w, &mut h);
                    }
                    events.push((
                        e.window.windowID.0,
                        Event::Resize([w.max(0) as u32, h.max(0) as u32]),
                    ));
                } else if ty == SDL_EVENT_KEY_DOWN.0 || ty == SDL_EVENT_KEY_UP.0 {
                    events.push((
                        e.key.windowID.0,
                        Event::Key {
                            scancode: e.key.scancode.0,
                            pressed: ty == SDL_EVENT_KEY_DOWN.0,
                            repeat: e.key.repeat,
                        },
                    ));
                } else if ty == SDL_EVENT_MOUSE_MOTION.0 {
                    events.push((
                        e.motion.windowID.0,
                        Event::MouseMotion {
                            position: [e.motion.x, e.motion.y],
                            delta: [e.motion.xrel, e.motion.yrel],
                        },
                    ));
                } else if ty == SDL_EVENT_MOUSE_BUTTON_DOWN.0 || ty == SDL_EVENT_MOUSE_BUTTON_UP.0 {
                    events.push((
                        e.button.windowID.0,
                        Event::MouseButton {
                            button: e.button.button,
                            pressed: ty == SDL_EVENT_MOUSE_BUTTON_DOWN.0,
                        },
                    ));
                } else if ty == SDL_EVENT_MOUSE_WHEEL.0 {
                    let direction = if e.wheel.direction == sdl3_sys::mouse::SDL_MOUSEWHEEL_FLIPPED
                    {
                        -1.
                    } else {
                        1.
                    };
                    events.push((
                        e.wheel.windowID.0,
                        Event::Wheel([e.wheel.x * direction, e.wheel.y * direction]),
                    ));
                }
            }
        }
        Ok(events)
    }
}
/// Decode an 8-bit RGBA PNG into pixels and its size.
fn decode_icon(png_bytes: &[u8]) -> Result<(Vec<u8>, [u32; 2])> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes)).read_info()?;
    let mut rgba = vec![0; reader.output_buffer_size().context("icon too large")?];
    let info = reader.next_frame(&mut rgba)?;
    ensure!(
        info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight,
        "icon must be 8-bit RGBA"
    );
    rgba.truncate(info.buffer_size());
    Ok((rgba, [info.width, info.height]))
}
pub(crate) fn error() -> String {
    // SAFETY: SDL_GetError returns a valid thread-local nul-terminated string.
    unsafe {
        CStr::from_ptr(SDL_GetError())
            .to_string_lossy()
            .into_owned()
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        self.cursors.clear();
        // SAFETY: Single owner on the creating thread. Application destroys surfaces first.
        unsafe {
            SDL_DestroyWindow(self.handle);
            SDL_QuitSubSystem(SDL_INIT_VIDEO);
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn ara_icon_decodes_to_64px_rgba() {
        let (rgba, size) = super::decode_icon(super::ARA_ICON_PNG).expect("embedded icon decodes");
        assert_eq!(size, [64, 64]);
        assert_eq!(rgba.len(), 64 * 64 * 4);
    }
}
