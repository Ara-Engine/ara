//! Raw SDL injection, drained immediately so borrowed UTF-8 never outlives its owner.
use ara_platform::{Event, Window};
use sdl3_sys::{events::*, scancode::SDL_Scancode, video::SDL_WindowID};
use std::ffi::CString;

fn push(mut event: SDL_Event, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    // SAFETY: every caller initializes the union member selected by its event type.
    // Text callers keep CString storage live until poll_tagged has copied it.
    if !unsafe { SDL_PushEvent(&mut event) } {
        return Err("native input proof: SDL rejected an injected event".into());
    }
    events.extend(Window::poll_tagged()?);
    Ok(())
}

pub(super) fn key(
    window: &Window,
    scan: i32,
    down: bool,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    push(
        SDL_Event {
            key: SDL_KeyboardEvent {
                r#type: if down {
                    SDL_EVENT_KEY_DOWN
                } else {
                    SDL_EVENT_KEY_UP
                },
                windowID: SDL_WindowID(window.id()),
                scancode: SDL_Scancode(scan),
                down,
                ..Default::default()
            },
        },
        events,
    )
}

pub(super) fn tap(
    window: &Window,
    scan: i32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    key(window, scan, true, events)?;
    key(window, scan, false, events)
}

pub(super) fn shortcut(
    window: &Window,
    scan: i32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
    key(window, primary, true, events)?;
    tap(window, scan, events)?;
    key(window, primary, false, events)
}

pub(super) fn text(
    window: &Window,
    text: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let storage = CString::new(text)?;
    let before = events.len();
    push(
        SDL_Event {
            text: SDL_TextInputEvent {
                r#type: SDL_EVENT_TEXT_INPUT,
                windowID: SDL_WindowID(window.id()),
                text: storage.as_ptr(),
                ..Default::default()
            },
        },
        events,
    )?;
    if !events[before..].iter().any(|(id, event)| {
        *id == window.id() && matches!(event, Event::TextInput { text: actual } if actual == text)
    }) {
        return Err("native input proof: SDL text did not pass through Window::poll_tagged".into());
    }
    Ok(())
}

pub(super) fn preedit(
    window: &Window,
    text: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let storage = CString::new(text)?;
    let before = events.len();
    push(
        SDL_Event {
            edit: SDL_TextEditingEvent {
                r#type: SDL_EVENT_TEXT_EDITING,
                windowID: SDL_WindowID(window.id()),
                text: storage.as_ptr(),
                start: 1,
                length: i32::MAX,
                ..Default::default()
            },
        },
        events,
    )?;
    if !events[before..].iter().any(|(id, event)| {
        *id == window.id()
            && matches!(event, Event::TextEditing { text: actual, .. } if actual == text)
    }) {
        return Err(
            "native input proof: SDL preedit did not pass through Window::poll_tagged".into(),
        );
    }
    Ok(())
}

pub(super) fn motion(
    window: &Window,
    point: [f32; 2],
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let [x, y] = point.map(|v| v / scale);
    push(
        SDL_Event {
            motion: SDL_MouseMotionEvent {
                r#type: SDL_EVENT_MOUSE_MOTION,
                windowID: SDL_WindowID(window.id()),
                x,
                y,
                ..Default::default()
            },
        },
        events,
    )
}

pub(super) fn button(
    window: &Window,
    down: bool,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    push(
        SDL_Event {
            button: SDL_MouseButtonEvent {
                r#type: if down {
                    SDL_EVENT_MOUSE_BUTTON_DOWN
                } else {
                    SDL_EVENT_MOUSE_BUTTON_UP
                },
                windowID: SDL_WindowID(window.id()),
                button: 1,
                down,
                clicks: 1,
                ..Default::default()
            },
        },
        events,
    )
}

pub(super) fn focus_lost(window: &Window, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    push(
        SDL_Event {
            window: SDL_WindowEvent {
                r#type: SDL_EVENT_WINDOW_FOCUS_LOST,
                windowID: SDL_WindowID(window.id()),
                ..Default::default()
            },
        },
        events,
    )
}
