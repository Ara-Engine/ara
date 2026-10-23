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
    /// Drain native input and lifecycle events without owning gameplay state.
    pub fn poll(&self) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        // SAFETY: SDL initializes the union; only the member matching its type is read.
        unsafe {
            let mut event = std::mem::MaybeUninit::<SDL_Event>::uninit();
            while SDL_PollEvent(event.as_mut_ptr()) {
                let e = event.assume_init_ref();
                let ty = e.r#type;
                if ty == SDL_EVENT_QUIT.0 || ty == SDL_EVENT_WINDOW_CLOSE_REQUESTED.0 {
                    events.push(Event::Quit);
                } else if ty == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED.0
                    || ty == SDL_EVENT_WINDOW_RESTORED.0
                    || ty == SDL_EVENT_WINDOW_MINIMIZED.0
                {
                    events.push(Event::Resize(self.size()?));
                } else if ty == SDL_EVENT_KEY_DOWN.0 || ty == SDL_EVENT_KEY_UP.0 {
                    events.push(Event::Key {
                        scancode: e.key.scancode.0,
                        pressed: ty == SDL_EVENT_KEY_DOWN.0,
                        repeat: e.key.repeat,
                    });
                } else if ty == SDL_EVENT_MOUSE_MOTION.0 {
                    events.push(Event::MouseMotion {
                        position: [e.motion.x, e.motion.y],
                        delta: [e.motion.xrel, e.motion.yrel],
                    });
                } else if ty == SDL_EVENT_MOUSE_BUTTON_DOWN.0 || ty == SDL_EVENT_MOUSE_BUTTON_UP.0 {
                    events.push(Event::MouseButton {
                        button: e.button.button,
                        pressed: ty == SDL_EVENT_MOUSE_BUTTON_DOWN.0,
                    });
                } else if ty == SDL_EVENT_MOUSE_WHEEL.0 {
                    events.push(Event::Wheel([e.wheel.x, e.wheel.y]));
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
