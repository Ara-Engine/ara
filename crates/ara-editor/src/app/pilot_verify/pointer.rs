//! Actual SDL events with a persistent synthetic desktop pointer; no OS polling.
use super::*;
use sdl3_sys::{events::*, video::SDL_WindowID};
impl PilotCheck {
    pub(super) fn motion(
        &mut self,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        point: [f32; 2],
        delta: [f32; 2],
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !scale.is_finite() || scale <= 0. {
            return Err("Pilot native scale invalid".into());
        }
        let rect = window.rect()?;
        let desktop = [
            rect[0] as f32 + point[0] / scale,
            rect[1] as f32 + point[1] / scale,
        ];
        self.pointer = Some(desktop);
        shell.desktop_pointer = Some(desktop);
        let mut event = SDL_Event {
            motion: SDL_MouseMotionEvent {
                r#type: SDL_EVENT_MOUSE_MOTION,
                windowID: SDL_WindowID(window.id()),
                x: point[0] / scale,
                y: point[1] / scale,
                xrel: delta[0] / scale,
                yrel: delta[1] / scale,
                ..Default::default()
            },
        };
        // SAFETY: the initialized motion member matches this SDL event tag.
        if !unsafe { SDL_PushEvent(&mut event) } {
            return Err("SDL rejected Pilot native motion".into());
        }
        events.extend(Window::poll_tagged()?);
        if self.trace.len() < 256 {
            self.trace.push(json!({"case":CASES[self.index],"phase":self.phase,"point":point,"delta":delta,"desktop":desktop,"window":window.id(),"scale":scale}));
        }
        Ok(())
    }
    pub(super) fn secondary(
        window: &Window,
        pressed: bool,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let mut event = SDL_Event {
            button: SDL_MouseButtonEvent {
                r#type: if pressed {
                    SDL_EVENT_MOUSE_BUTTON_DOWN
                } else {
                    SDL_EVENT_MOUSE_BUTTON_UP
                },
                windowID: SDL_WindowID(window.id()),
                button: 3,
                down: pressed,
                clicks: 1,
                ..Default::default()
            },
        };
        // SAFETY: the initialized button member matches this SDL event tag.
        if !unsafe { SDL_PushEvent(&mut event) } {
            return Err("SDL rejected Pilot native RMB".into());
        }
        events.extend(Window::poll_tagged()?);
        Ok(())
    }
    pub(super) fn click(
        &mut self,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        id: &str,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let i = shell
            .shown_node(id)
            .ok_or_else(|| format!("Pilot native control unavailable: {id}"))?;
        let node = &shell.ui.nodes[i];
        let rect = node.rect.intersection(node.clip);
        if rect.w < 2. || rect.h < node.rect.h * 0.75 {
            return Err(format!("Pilot control clipped: {id}").into());
        }
        self.motion(
            shell,
            window,
            scale,
            [rect.x + rect.w * 0.4, rect.y + rect.h * 0.5],
            [0.; 2],
            events,
        )?;
        native::button(window, true, events)?;
        native::button(window, false, events)
    }
    pub(super) fn viewport_point(shell: &Shell) -> ara_ui::Result<[f32; 2]> {
        let rect = shell.viewport().ok_or("Pilot Scene viewport absent")?;
        let point = [rect.x + rect.w * 0.55, rect.y + rect.h * 0.65];
        if shell.ui.hit_test(point[0], point[1]).is_some() {
            return Err("Pilot Scene press is occluded by a UI control".into());
        }
        Ok(point)
    }
    pub(super) fn palette(
        &mut self,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        command: &str,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        match self.phase {
            1 => {
                native::shortcut(window, 14, events)?;
                self.next();
            }
            2 => {
                if shell.shown_node("palette-search").is_none() {
                    return Ok(false);
                }
                native::shortcut(window, 4, events)?;
                native::text(window, command, events)?;
                self.next();
            }
            3 => {
                let id = shell
                    .command_bindings()
                    .into_iter()
                    .find(|(id, binding)| {
                        id.starts_with("command-")
                            && binding == command
                            && shell.shown_node(id).is_some()
                    })
                    .map(|(id, _)| id);
                if let Some(id) = id {
                    self.click(shell, window, scale, &id, events)?;
                    self.next();
                }
            }
            _ => return Ok(true),
        }
        Ok(false)
    }
}
