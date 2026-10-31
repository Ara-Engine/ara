//! Repaint while the OS runs a modal resize loop.
//!
//! On Windows, dragging a native window border runs a modal loop inside the event
//! pump: `SDL_PollEvent`/`SDL_WaitEvent*` do not return until the mouse is released.
//! SDL reports each live-resize step as `SDL_EVENT_WINDOW_EXPOSED` with `data1 == 1`
//! through event watches, which still run. `with_live_repaint` installs such a
//! watch for the duration of one blocking pump call.
use sdl3_sys::{events::*, init::SDL_IsMainThread};
use std::{cell::Cell, ffi::c_void};

thread_local! {
    static PAINTING: Cell<bool> = const { Cell::new(false) };
}

/// Run `pump` (an event poll or wait) and call `repaint` for every live-resize
/// step it blocks on. `repaint` may resize swapchains and present; it is never
/// re-entered.
pub fn with_live_repaint<R>(repaint: &mut dyn FnMut(), pump: impl FnOnce() -> R) -> R {
    let mut repaint = repaint;
    let data = (&raw mut repaint).cast::<c_void>();
    // SAFETY: `data` points at a local that outlives the watch, which is removed
    // before this function returns; SDL calls the watch synchronously.
    unsafe {
        if !SDL_AddEventWatch(Some(watch), data) {
            return pump();
        }
        let result = pump();
        SDL_RemoveEventWatch(Some(watch), data);
        result
    }
}

unsafe extern "C" fn watch(data: *mut c_void, event: *mut SDL_Event) -> bool {
    // SAFETY: SDL passes a valid event. Other threads may push events through
    // watches too; only main-thread live-resize exposes repaint, where `data` is the
    // `&mut dyn FnMut()` installed by `with_live_repaint` on this same thread.
    unsafe {
        let e = &*event;
        if e.r#type == SDL_EVENT_WINDOW_EXPOSED.0
            && e.window.data1 == 1
            && SDL_IsMainThread()
            && !PAINTING.get()
        {
            PAINTING.set(true);
            let repaint = &mut *data.cast::<&mut dyn FnMut()>();
            repaint();
            PAINTING.set(false);
        }
    }
    true
}
