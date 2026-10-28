//! Recorded logical input becomes tagged platform events for the real Controller.
use super::Action;
use crate::shell::Shell;
use ara_platform::Event;

/// A live native window's coordinate mapping, supplied by its platform owner.
pub(in crate::app) struct Surface {
    /// Floating surface identity; absent is the main editor window.
    pub id: Option<String>,
    /// SDL window identity used by the production event router.
    pub sdl: u32,
    /// Window points to logical pixels (pixel density divided by UI DPI).
    pub points_to_logical: f32,
    /// Window rectangle in desktop points, including its virtual offscreen origin.
    pub desktop: [f32; 4],
}

/// All events of one recorded action, with its synthetic desktop pointer.
pub(in crate::app) struct Input {
    /// Route each event before completing the monkey action callback.
    pub events: Vec<(u32, Event)>,
    /// Set Shell::desktop_pointer before routing, so drags never poll the OS mouse.
    pub desktop_pointer: Option<[f32; 2]>,
}

/// Keeps held pointer deltas and the floating keyboard destination across actions.
#[derive(Default)]
pub(in crate::app) struct Adapter {
    keyboard: Option<String>,
    pointer: Option<[f32; 2]>,
}
impl Adapter {
    /// Commands/protocol calls return None and retain their typed action.run path.
    pub fn events(
        &mut self,
        action: &Action,
        shell: &mut Shell,
        main: &Surface,
        floats: &[Surface],
    ) -> ara_ui::Result<Option<Input>> {
        if matches!(
            action,
            Action::Command { .. } | Action::Protocol { .. } | Action::TypedCommand { .. }
        ) {
            return Ok(None);
        }
        let mut events = Vec::new();
        match action {
            Action::Click { point }
            | Action::Press { point }
            | Action::Move { point }
            | Action::Release { point, .. }
            | Action::Wheel { point, .. }
            | Action::PointerButton { point, .. } => {
                self.keyboard = None;
                self.motion(main, *point, &mut events)?;
                match action {
                    Action::Click { .. } => {
                        button(main, true, &mut events);
                        button(main, false, &mut events);
                    }
                    Action::Press { .. } => button(main, true, &mut events),
                    Action::PointerButton {
                        button, pressed, ..
                    } => {
                        if !(1..=3).contains(button) {
                            return Err(
                                "Monkey pointer button must be primary, middle or secondary".into(),
                            );
                        }
                        events.push((
                            main.sdl,
                            Event::MouseButton {
                                button: *button,
                                pressed: *pressed,
                            },
                        ));
                    }
                    Action::Release { cancel, .. } => {
                        if *cancel {
                            tap(main, 41, &mut events);
                        }
                        button(main, false, &mut events);
                    }
                    Action::Wheel { delta, .. } => {
                        if !delta.iter().all(|value| value.is_finite()) {
                            return Err("Non-finite monkey wheel input".into());
                        }
                        // Controller converts wheel detents to signed 26 px widget scrolling.
                        events.push((main.sdl, Event::Wheel(delta.map(|value| -value / 26.))));
                    }
                    _ => {}
                }
            }
            Action::Tab { reverse } => {
                tab(self.keyboard_surface(main, floats)?, *reverse, &mut events)
            }
            Action::Key { scancode } => {
                tap(self.keyboard_surface(main, floats)?, *scancode, &mut events)
            }
            Action::KeyState { scancode, pressed } => key(
                self.keyboard_surface(main, floats)?,
                *scancode,
                *pressed,
                &mut events,
            ),
            Action::FocusLost => {
                let surface = self.keyboard_surface(main, floats).unwrap_or(main);
                events.push((surface.sdl, Event::FocusLost));
                self.keyboard = None;
            }
            Action::Text { id, value } => {
                let surface = if let Some(id) = id {
                    let (surface, point) = self.field(shell, id, main, floats)?;
                    self.keyboard = surface.id.clone();
                    self.motion(surface, point, &mut events)?;
                    button(surface, true, &mut events);
                    button(surface, false, &mut events);
                    surface
                } else {
                    self.keyboard_surface(main, floats)?
                };
                let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
                key(surface, primary, true, &mut events);
                tap(surface, 4, &mut events);
                key(surface, primary, false, &mut events);
                if value.is_empty() {
                    tap(surface, 42, &mut events);
                } else {
                    events.push((
                        surface.sdl,
                        Event::TextInput {
                            text: value.clone(),
                        },
                    ));
                }
                tap(surface, 40, &mut events);
            }
            Action::FloatingInput { surface, reverse } => {
                let surface = floats
                    .iter()
                    .find(|candidate| candidate.id.as_deref() == Some(surface))
                    .ok_or("Floating monkey surface closed before input arrived")?;
                self.keyboard = surface.id.clone();
                let centre = [
                    surface.desktop[0] + surface.desktop[2] * 0.5,
                    surface.desktop[1] + surface.desktop[3] * 0.5,
                ];
                if !centre.iter().all(|value| value.is_finite()) {
                    return Err("Invalid floating monkey surface geometry".into());
                }
                self.pointer = Some(centre);
                tab(surface, *reverse, &mut events);
                tap(surface, 40, &mut events);
            }
            Action::Command { .. } | Action::Protocol { .. } | Action::TypedCommand { .. } => {}
        }
        Ok(Some(Input {
            events,
            desktop_pointer: self.pointer,
        }))
    }
    fn keyboard_surface<'a>(
        &self,
        main: &'a Surface,
        floats: &'a [Surface],
    ) -> ara_ui::Result<&'a Surface> {
        match self.keyboard.as_deref() {
            None => Ok(main),
            Some(id) => floats
                .iter()
                .find(|surface| surface.id.as_deref() == Some(id))
                .ok_or_else(|| "Floating monkey keyboard destination was closed".into()),
        }
    }
    fn motion(
        &mut self,
        surface: &Surface,
        point: [f32; 2],
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let scale = surface.points_to_logical;
        if !scale.is_finite() || scale <= 0. || !point.iter().all(|value| value.is_finite()) {
            return Err("Invalid monkey logical-to-window input mapping".into());
        }
        let position = point.map(|value| value / scale);
        let desktop = [
            surface.desktop[0] + position[0],
            surface.desktop[1] + position[1],
        ];
        let delta = self.pointer.map_or([0.; 2], |previous| {
            [desktop[0] - previous[0], desktop[1] - previous[1]]
        });
        if !position
            .iter()
            .chain(&desktop)
            .chain(&delta)
            .all(|value| value.is_finite())
        {
            return Err("Monkey pointer conversion overflowed".into());
        }
        self.pointer = Some(desktop);
        events.push((surface.sdl, Event::MouseMotion { position, delta }));
        Ok(())
    }
    fn field<'a>(
        &self,
        shell: &mut Shell,
        id: &str,
        main: &'a Surface,
        floats: &'a [Surface],
    ) -> ara_ui::Result<(&'a Surface, [f32; 2])> {
        shell.leave();
        let main_point = field_point(shell, id);
        if let Some(point) = main_point {
            return Ok((main, point));
        }
        for surface in floats {
            if let Some(name) = &surface.id
                && shell.enter(name)
            {
                let point = field_point(shell, id);
                shell.leave();
                if let Some(point) = point {
                    return Ok((surface, point));
                }
            }
        }
        shell.leave();
        Err(format!("Monkey text target `{id}` is no longer mounted and visible").into())
    }
}
fn field_point(shell: &Shell, id: &str) -> Option<[f32; 2]> {
    let index = shell.shown_node(id)?;
    let node = &shell.ui.nodes[index];
    if !matches!(node.element.tag.as_str(), "field" | "input") {
        return None;
    }
    let left = node.rect.x.max(node.clip.x);
    let top = node.rect.y.max(node.clip.y);
    let right = (node.rect.x + node.rect.w).min(node.clip.x + node.clip.w);
    let bottom = (node.rect.y + node.rect.h).min(node.clip.y + node.clip.h);
    (right > left && bottom > top).then_some([(left + right) * 0.5, (top + bottom) * 0.5])
}
fn button(surface: &Surface, pressed: bool, events: &mut Vec<(u32, Event)>) {
    events.push((surface.sdl, Event::MouseButton { button: 1, pressed }));
}
fn key(surface: &Surface, scancode: i32, pressed: bool, events: &mut Vec<(u32, Event)>) {
    events.push((
        surface.sdl,
        Event::Key {
            scancode,
            pressed,
            repeat: false,
        },
    ));
}
fn tap(surface: &Surface, scancode: i32, events: &mut Vec<(u32, Event)>) {
    key(surface, scancode, true, events);
    key(surface, scancode, false, events);
}
fn tab(surface: &Surface, reverse: bool, events: &mut Vec<(u32, Event)>) {
    if reverse {
        key(surface, 225, true, events);
    }
    tap(surface, 43, events);
    if reverse {
        key(surface, 225, false, events);
    }
}
