use anyhow::{Result, ensure};
use sdl3_sys::{error::SDL_GetError, events::*, init::*, video::*, vulkan::*};
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
/// Main-thread-owned resizable SDL3 Vulkan window and video subsystem reference.
pub struct Window {
    pub(crate) handle: *mut SDL_Window,
    _main_thread: PhantomData<*mut ()>,
}
impl Window {
    /// Open a native resizable Vulkan window; creation is confined to the SDL main thread.
    pub fn new(title: &str, size: [u32; 2]) -> Result<Self> {
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
                SDL_WINDOW_VULKAN | SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY,
            );
            if handle.is_null() {
                SDL_QuitSubSystem(SDL_INIT_VIDEO);
                anyhow::bail!("SDL window: {}", error());
            }
            Ok(Self {
                handle,
                _main_thread: PhantomData,
            })
        }
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
        let window = Self::new(title, [rect[2].max(64) as u32, rect[3].max(48) as u32])?;
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
    /// Bring the window to the front and give it keyboard focus.
    pub fn raise(&self) {
        // SAFETY: Live window on its owning thread.
        unsafe {
            SDL_RaiseWindow(self.handle);
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
                    events.push((e.wheel.windowID.0, Event::Wheel([e.wheel.x, e.wheel.y])));
                }
            }
        }
        Ok(events)
    }
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
        // SAFETY: Single owner on the creating thread. Application destroys surfaces first.
        unsafe {
            SDL_DestroyWindow(self.handle);
            SDL_QuitSubSystem(SDL_INIT_VIDEO);
        }
    }
}
