//! Native batch boundaries deliberately span several production event-loop frames.
use super::*;
use sdl3_sys::{events::*, scancode::SDL_Scancode, video::SDL_WindowID};
use std::ffi::CString;

/// Poll while the owned filename is alive; ara-platform must copy it into FileDrop.
fn native(id: u32, event: Event, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    let expected = format!("{event:?}");
    let filename = match &event {
        Event::FileDrop { path, .. } => Some(CString::new(path.to_string_lossy().as_bytes())?),
        _ => None,
    };
    let mut raw = match &event {
        Event::FileDropBegin => SDL_Event {
            drop: SDL_DropEvent {
                r#type: SDL_EVENT_DROP_BEGIN,
                windowID: SDL_WindowID(id),
                ..Default::default()
            },
        },
        Event::FileDropPosition { position } => SDL_Event {
            drop: SDL_DropEvent {
                r#type: SDL_EVENT_DROP_POSITION,
                windowID: SDL_WindowID(id),
                x: position[0],
                y: position[1],
                ..Default::default()
            },
        },
        Event::FileDrop { position, .. } => SDL_Event {
            drop: SDL_DropEvent {
                r#type: SDL_EVENT_DROP_FILE,
                windowID: SDL_WindowID(id),
                x: position[0],
                y: position[1],
                data: filename
                    .as_ref()
                    .ok_or("Native filename disappeared")?
                    .as_ptr(),
                ..Default::default()
            },
        },
        // Completion has no independent coordinate. Poison it to prove the
        // production transport retains the last valid tagged window position.
        Event::FileDropComplete => SDL_Event {
            drop: SDL_DropEvent {
                r#type: SDL_EVENT_DROP_COMPLETE,
                windowID: SDL_WindowID(id),
                x: f32::NAN,
                y: f32::INFINITY,
                ..Default::default()
            },
        },
        Event::FocusLost | Event::Quit => SDL_Event {
            window: SDL_WindowEvent {
                r#type: if matches!(&event, Event::FocusLost) {
                    SDL_EVENT_WINDOW_FOCUS_LOST
                } else {
                    SDL_EVENT_WINDOW_CLOSE_REQUESTED
                },
                windowID: SDL_WindowID(id),
                ..Default::default()
            },
        },
        Event::Key {
            scancode, pressed, ..
        } => SDL_Event {
            key: SDL_KeyboardEvent {
                r#type: if *pressed {
                    SDL_EVENT_KEY_DOWN
                } else {
                    SDL_EVENT_KEY_UP
                },
                windowID: SDL_WindowID(id),
                scancode: SDL_Scancode(*scancode),
                down: *pressed,
                ..Default::default()
            },
        },
        _ => {
            return Err("The import proof only injects file batches and cancellation input".into());
        }
    };
    // SAFETY: the union member matches its type; filename remains owned through polling.
    if !unsafe { SDL_PushEvent(&mut raw) } {
        return Err("SDL rejected the native import proof event".into());
    }
    let decoded = Window::poll_tagged()?;
    let delivered = decoded
        .iter()
        .any(|(window, event)| *window == id && format!("{event:?}") == expected);
    events.extend(decoded);
    if !delivered {
        return Err("SDL did not preserve the native file event or its destination tag".into());
    }
    Ok(())
}
impl ImportCheck {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn drop_point(
        &self,
        act: Act,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
    ) -> ara_ui::Result<Option<(u32, [f32; 2])>> {
        let storage = &self.target.as_ref().ok_or("Import target missing")?.storage;
        shell.leave();
        if act == Act::ClosedTarget {
            let Some(float) = floats.windows.iter().find(|float| {
                editor.state.floating.iter().any(|state| {
                    state.id == float.id
                        && super::super::panels_of(&state.root)
                            .iter()
                            .any(|panel| panel == "assets")
                })
            }) else {
                return Ok(None);
            };
            if !shell.enter(&float.id) {
                return Ok(None);
            }
            let scale = crate::native::points_to_logical(
                float.window.raw(),
                float.window.display_scale() * windows::ui_scale(editor),
            );
            let point = folder_point(shell, storage)
                .map(|point| (float.sdl, point.map(|value| value / scale)));
            shell.leave();
            Ok(point)
        } else {
            let scale = crate::native::points_to_logical(main.raw(), dpi);
            Ok(folder_point(shell, storage)
                .map(|point| (main.id(), point.map(|value| value / scale))))
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn stream_input(
        &mut self,
        act: Act,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.age == 1 {
            let Some((sdl, point)) = self.drop_point(act, editor, shell, main, dpi, floats)? else {
                return self.wait();
            };
            self.stream_target = Some((sdl, point));
            native(sdl, Event::FileDropBegin, events)?;
            native(sdl, Event::FileDropPosition { position: point }, events)?;
            return Ok(());
        }
        let (sdl, point) = self
            .stream_target
            .ok_or("Native import stream lost its surface")?;
        if self.age <= 4 && !(act.discarded() && self.age == 4) {
            let report = shell.asset_import_report();
            if job_ids(&report) != self.old_jobs
                || report["queued"] != 0
                || report["ready"] != 0
                || editor.host.store().revision() != self.before_revision
                || fixture::selection(editor, shell) != self.selection
            {
                return Err("A native file batch started authoring before COMPLETE".into());
            }
            if shell.file_drag_report()["feedback"]["valid"] != true {
                return Err(
                    "The native folder preview did not expose its accepted cached destination"
                        .into(),
                );
            }
            self.stream_frames += 1;
            self.trace.push(json!({"stage":"before-complete","age":self.age,"fileDrag":shell.file_drag_report(),"imports":report}));
        }
        match self.age {
            2 => native(
                sdl,
                Event::FileDrop {
                    path: self.inputs.tone.clone(),
                    position: point,
                },
                events,
            )?,
            3 if act == Act::DragEscape => {
                for pressed in [true, false] {
                    native(
                        sdl,
                        Event::Key {
                            scancode: 41,
                            pressed,
                            repeat: false,
                        },
                        events,
                    )?;
                }
            }
            3 if act == Act::DragFocusLost => native(sdl, Event::FocusLost, events)?,
            3 if act == Act::ClosedTarget => native(sdl, Event::Quit, events)?,
            3 => {
                native(
                    sdl,
                    Event::FileDrop {
                        path: self.inputs.other.clone(),
                        position: point,
                    },
                    events,
                )?;
                native(
                    sdl,
                    Event::FileDrop {
                        path: self.inputs.tone.clone(),
                        position: point,
                    },
                    events,
                )?;
            }
            4 => {
                if act.discarded() {
                    native(
                        sdl,
                        Event::FileDrop {
                            path: self.inputs.other.clone(),
                            position: point,
                        },
                        events,
                    )?;
                }
                native(sdl, Event::FileDropComplete, events)?;
            }
            _ if act.discarded() => {
                let report = shell.asset_import_report();
                if job_ids(&report) != self.old_jobs
                    || editor.host.store().revision() != self.before_revision
                    || !shell.file_drag_report().is_null()
                {
                    return Err("A cancelled native batch imported late files".into());
                }
                self.terminal = Some(Value::Null);
                self.committed = Some(self.before.clone());
                self.age = 9;
            }
            _ => {}
        }
        Ok(())
    }
}
