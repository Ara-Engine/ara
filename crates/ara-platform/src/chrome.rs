//! Custom window captions use SDL's native hit testing, including resize borders.
use crate::{Window, window::error};
use anyhow::{Result, ensure};
use sdl3_sys::{rect::SDL_Point, video::*};
use std::ffi::c_void;

/// Main-window caption geometry in SDL window coordinates (not drawable pixels).
#[derive(Default)]
pub struct WindowChrome {
    /// Height of the custom title bar.
    pub height: f32,
    /// Width of the native resize border; ignored when maximized or fullscreen.
    pub border: f32,
    /// Interactive rectangles [x, y, width, height] excluded from caption dragging.
    pub controls: Vec<[f32; 4]>,
}
impl WindowChrome {
    /// Native hit-test result at a point; corners take priority over caption controls.
    pub fn hit_test(&self, point: [f32; 2], size: [f32; 2], maximized: bool) -> SDL_HitTestResult {
        let [x, y] = point;
        if !maximized {
            let b = self.border;
            let left = x < b;
            let right = x >= size[0] - b;
            let top = y < b;
            let bottom = y >= size[1] - b;
            match (left, right, top, bottom) {
                (true, _, true, _) => return SDL_HITTEST_RESIZE_TOPLEFT,
                (_, true, true, _) => return SDL_HITTEST_RESIZE_TOPRIGHT,
                (true, _, _, true) => return SDL_HITTEST_RESIZE_BOTTOMLEFT,
                (_, true, _, true) => return SDL_HITTEST_RESIZE_BOTTOMRIGHT,
                (true, _, _, _) => return SDL_HITTEST_RESIZE_LEFT,
                (_, true, _, _) => return SDL_HITTEST_RESIZE_RIGHT,
                (_, _, true, _) => return SDL_HITTEST_RESIZE_TOP,
                (_, _, _, true) => return SDL_HITTEST_RESIZE_BOTTOM,
                _ => {}
            }
        }
        if y >= 0. && y < self.height && !self.controls.iter().any(|r| contains(*r, point)) {
            SDL_HITTEST_DRAGGABLE
        } else {
            SDL_HITTEST_NORMAL
        }
    }
}
fn contains(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[1] >= r[1] && p[0] < r[0] + r[2] && p[1] < r[1] + r[3]
}
unsafe extern "C" fn hit_test(
    window: *mut SDL_Window,
    point: *const SDL_Point,
    data: *mut c_void,
) -> SDL_HitTestResult {
    // SAFETY: Window owns the stable Box and destroys the SDL window before dropping it.
    // SDL calls this synchronously on the window thread with valid window/point pointers.
    unsafe {
        let chrome = &*(data.cast::<WindowChrome>());
        let p = &*point;
        let (mut w, mut h) = (0, 0);
        SDL_GetWindowSize(window, &mut w, &mut h);
        let flags = SDL_GetWindowFlags(window);
        chrome.hit_test(
            [p.x as f32, p.y as f32],
            [w as f32, h as f32],
            flags & (SDL_WINDOW_MAXIMIZED | SDL_WINDOW_FULLSCREEN) != 0,
        )
    }
}
impl Window {
    /// Replace the OS caption with a client-drawn title bar. SDL keeps native
    /// caption dragging, double-click maximize, snapping and eight resize regions.
    pub fn enable_custom_chrome(&mut self) -> Result<()> {
        if self.chrome.is_some() {
            return Ok(());
        }
        let mut chrome = Box::<WindowChrome>::default();
        let data = (&raw mut *chrome).cast::<c_void>();
        // SAFETY: This live main-thread window retains the Box until after SDL_DestroyWindow.
        unsafe {
            ensure!(
                SDL_SetWindowHitTest(self.handle, Some(hit_test), data),
                "SDL hit test: {}",
                error()
            );
            if !SDL_SetWindowBordered(self.handle, false) {
                SDL_SetWindowHitTest(self.handle, None, std::ptr::null_mut());
                anyhow::bail!("SDL custom caption: {}", error());
            }
        }
        self.chrome = Some(chrome);
        Ok(())
    }
    /// Mutable geometry reused after UI layout; no per-frame allocation is needed.
    pub fn chrome_mut(&mut self) -> Option<&mut WindowChrome> {
        self.chrome.as_deref_mut()
    }
    /// Whether the native window currently fills the monitor's usable area.
    pub fn maximized(&self) -> bool {
        // SAFETY: This owner retains the SDL window on the creating thread.
        unsafe { SDL_GetWindowFlags(self.handle) & SDL_WINDOW_MAXIMIZED != 0 }
    }
    /// Minimize this native window.
    pub fn minimize(&self) -> Result<()> {
        ensure!(
            // SAFETY: Live native window on its owning thread.
            unsafe { SDL_MinimizeWindow(self.handle) },
            "SDL minimize: {}",
            error()
        );
        Ok(())
    }
    /// Toggle native maximize/restore, preserving the window's restore rectangle.
    pub fn toggle_maximize(&self) -> Result<()> {
        // SAFETY: Live native window on its owning thread; SDL manages the restore rectangle.
        let ok = unsafe {
            if self.maximized() {
                SDL_RestoreWindow(self.handle)
            } else {
                SDL_MaximizeWindow(self.handle)
            }
        };
        ensure!(ok, "SDL maximize/restore: {}", error());
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caption_controls_and_all_resize_regions() {
        let chrome = WindowChrome {
            height: 36.,
            border: 5.,
            controls: vec![[10., 6., 300., 30.], [680., 6., 120., 30.]],
        };
        assert_eq!(
            chrome.hit_test([400., 20.], [800., 600.], false),
            SDL_HITTEST_DRAGGABLE
        );
        assert_eq!(
            chrome.hit_test([50., 20.], [800., 600.], false),
            SDL_HITTEST_NORMAL
        );
        assert_eq!(
            chrome.hit_test([700., 20.], [800., 600.], false),
            SDL_HITTEST_NORMAL
        );
        assert_eq!(
            chrome.hit_test([400., 200.], [800., 600.], false),
            SDL_HITTEST_NORMAL
        );
        for (p, expected) in [
            ([1., 1.], SDL_HITTEST_RESIZE_TOPLEFT),
            ([400., 1.], SDL_HITTEST_RESIZE_TOP),
            ([799., 1.], SDL_HITTEST_RESIZE_TOPRIGHT),
            ([799., 300.], SDL_HITTEST_RESIZE_RIGHT),
            ([799., 599.], SDL_HITTEST_RESIZE_BOTTOMRIGHT),
            ([400., 599.], SDL_HITTEST_RESIZE_BOTTOM),
            ([1., 599.], SDL_HITTEST_RESIZE_BOTTOMLEFT),
            ([1., 300.], SDL_HITTEST_RESIZE_LEFT),
        ] {
            assert_eq!(chrome.hit_test(p, [800., 600.], false), expected);
        }
        assert_eq!(
            chrome.hit_test([400., 1.], [800., 600.], true),
            SDL_HITTEST_DRAGGABLE
        );
        assert_eq!(
            chrome.hit_test([1., 300.], [800., 600.], true),
            SDL_HITTEST_NORMAL
        );
    }
}
