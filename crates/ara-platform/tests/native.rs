//! Explicit native integration gate for shared SDL ownership and input.
use anyhow::{Result, ensure};
use ara_audio::device::{DeviceBackend, DeviceEvent, SdlBackend};
use ara_platform::{Event, Window};
use sdl3_sys::{
    events::*, gamepad::SDL_GetGamepads, init::*, joystick::*, scancode::SDL_SCANCODE_A,
    stdinc::SDL_free,
};

#[test]
#[ignore = "requires a Vulkan desktop and an output audio device"]
fn shared_sdl_input_audio_and_subsystem_lifetimes() -> Result<()> {
    let window = Window::new("Ara SDL integration", [320, 240])?;
    let mut audio = SdlBackend::open(48000)?;
    let second_audio = SdlBackend::open(48000)?;
    ensure!(!audio.driver()?.is_empty() && !audio.name()?.is_empty());
    drop(second_audio);
    ensure!(audio.poll()? != DeviceEvent::Lost);
    ensure!(audio.recover()? == DeviceEvent::Recovered);
    // SAFETY: the window and audio owners retain their subsystem references.
    unsafe {
        ensure!(SDL_WasInit(SDL_INIT_VIDEO | SDL_INIT_AUDIO) == SDL_INIT_VIDEO | SDL_INIT_AUDIO);
        ensure!(SDL_InitSubSystem(SDL_INIT_GAMEPAD));
    }
    let gamepads = Gamepads;
    let mut descriptor = SDL_VirtualJoystickDesc::new();
    descriptor.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
    descriptor.naxes = 6;
    descriptor.nbuttons = 16;
    descriptor.name = c"Ara virtual gamepad".as_ptr();
    // SAFETY: descriptor has no borrowed callback state; SDL copies its name.
    let id = unsafe { SDL_AttachVirtualJoystick(&descriptor) };
    ensure!(id.0 != 0, "virtual joystick attachment failed");
    let joystick = VirtualJoystick(id);
    // SAFETY: SDL returns an allocated zero-terminated enumeration, freed below.
    unsafe {
        let mut count = 0;
        let ids = SDL_GetGamepads(&mut count);
        ensure!(!ids.is_null(), "gamepad enumeration failed");
        let found = std::slice::from_raw_parts(ids, count as usize).contains(&id);
        SDL_free(ids.cast());
        ensure!(found, "virtual gamepad missing from enumeration");
        let mut key = SDL_Event {
            key: SDL_KeyboardEvent {
                r#type: SDL_EVENT_KEY_DOWN,
                scancode: SDL_SCANCODE_A,
                down: true,
                ..Default::default()
            },
        };
        let mut mouse = SDL_Event {
            motion: SDL_MouseMotionEvent {
                r#type: SDL_EVENT_MOUSE_MOTION,
                x: 10.0,
                y: 20.0,
                xrel: 3.0,
                yrel: 4.0,
                ..Default::default()
            },
        };
        ensure!(SDL_PushEvent(&mut key) && SDL_PushEvent(&mut mouse));
    }
    let events = window.poll()?;
    ensure!(events.iter().any(|e| matches!(
        e,
        Event::Key {
            scancode: 4,
            pressed: true,
            ..
        }
    )));
    ensure!(events.iter().any(|e| matches!(
        e,
        Event::MouseMotion {
            delta: [3.0, 4.0],
            ..
        }
    )));
    drop(joystick);
    drop(gamepads);
    drop(window);
    // SAFETY: audio remains live after video is released; neither owner calls SDL_Quit.
    unsafe {
        ensure!(SDL_WasInit(SDL_INIT_AUDIO) == SDL_INIT_AUDIO);
    }
    audio.submit(&[0.0; 256])?;
    drop(audio);
    // SAFETY: each owner has released exactly its own subsystem reference.
    unsafe {
        ensure!(SDL_WasInit(SDL_INIT_AUDIO | SDL_INIT_VIDEO | SDL_INIT_GAMEPAD).0 == 0);
    }
    Ok(())
}

struct Gamepads;
impl Drop for Gamepads {
    fn drop(&mut self) {
        // SAFETY: balances the test's one successful gamepad subsystem initialization.
        unsafe {
            SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
        }
    }
}
struct VirtualJoystick(SDL_JoystickID);
impl Drop for VirtualJoystick {
    fn drop(&mut self) {
        // SAFETY: the test owns this virtual joystick and keeps its subsystem alive.
        unsafe {
            SDL_DetachVirtualJoystick(self.0);
        }
    }
}
