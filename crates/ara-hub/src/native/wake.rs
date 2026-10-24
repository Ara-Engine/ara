//! Wake the SDL event loop from workers; idle Hubs need no frame-rate polling.
pub(super) fn wake() {
    let mut event = sdl3_sys::events::SDL_Event::default();
    event.r#type = sdl3_sys::events::SDL_EVENT_USER.0;
    // SAFETY: SDL_PushEvent is thread-safe and copies the event. This notification
    // carries no borrowed pointers, and failure during shutdown is harmless.
    unsafe {
        sdl3_sys::events::SDL_PushEvent(&mut event);
    }
}
