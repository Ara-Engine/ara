//! Native camera, picking, transform and keyboard input, independent of presentation.
use super::*;
use glam::Vec2;
use std::collections::BTreeSet;
struct GizmoDrag {
    axis: usize,
    start: [f32; 2],
    vector: Vec2,
    length: f32,
}
#[derive(Default)]
pub(super) struct Controller {
    buttons: [bool; 4],
    keys: BTreeSet<i32>,
    gizmo: Option<GizmoDrag>,
}
impl Controller {
    pub fn event(
        &mut self,
        event: Event,
        editor: &mut Editor,
        shell: &mut Shell,
        renderer: &Renderer,
        view: &crate::world::DocumentView,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<(bool, bool, bool)> {
        let mut dirty = false;
        let mut scene_dirty = false;
        let mut quit = false;
        match event {
            Event::Quit => quit = true,
            Event::Resize(_) => dirty = true,
            Event::Key {
                scancode,
                pressed,
                repeat: _,
            } => {
                if pressed {
                    self.keys.insert(scancode);
                } else {
                    self.keys.remove(&scancode);
                }
                if !self.buttons[3] {
                    shell.key(editor, scancode, pressed, clipboard)?;
                    if scancode == 41 && pressed {
                        self.gizmo = None;
                        scene_dirty = true;
                    }
                } else {
                    match scancode {
                        225 | 229 => shell.modifiers[0] = pressed,
                        224 | 228 => shell.modifiers[1] = pressed,
                        226 | 230 => shell.modifiers[2] = pressed,
                        _ => {}
                    }
                }
                dirty = true;
            }
            Event::MouseMotion { position, delta } => {
                shell.pointer = position;
                editor.input_active();
                let inside = shell
                    .viewport()
                    .is_some_and(|r| r.contains(position[0], position[1]))
                    && !editor.state.palette
                    && !shell.context;
                if let Some(drag) = &self.gizmo {
                    if let Some(gesture) = editor.gesture.as_mut() {
                        let movement = Vec2::from(position) - Vec2::from(drag.start);
                        let amount =
                            movement.dot(drag.vector) / drag.vector.length_squared().max(0.01);
                        let amount = match gesture.tool {
                            crate::state::Tool::Translate => amount * drag.length,
                            crate::state::Tool::Rotate => amount * 90.,
                            crate::state::Tool::Scale => amount,
                        };
                        let mut delta = Vec3::ZERO;
                        delta[drag.axis] = amount;
                        gesture.update(delta);
                        scene_dirty = true;
                    }
                } else if inside
                    && (self.buttons[2]
                        || self.buttons[3]
                        || (self.buttons[1] && shell.modifiers[2]))
                {
                    let mut camera = editor.state.camera;
                    if self.buttons[2] {
                        camera.pan(delta);
                    } else {
                        camera.orbit(delta);
                    }
                    set_camera(editor, &camera)?;
                    scene_dirty = true;
                } else {
                    shell.event(InputEvent::PointerMove(position), clipboard)?;
                    shell.hover_tooltip()?;
                }
                dirty = true;
            }
            Event::MouseButton { button, pressed } => {
                if button < 4 {
                    self.buttons[button as usize] = pressed;
                }
                editor.input_active();
                if pressed
                    && shell
                        .viewport()
                        .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                    && !editor.state.palette
                    && !shell.context
                {
                    shell.commit_fields(editor)?;
                    shell.input.blur(&mut shell.ui);
                }
                if button == 3 && pressed {
                    if !shell
                        .viewport()
                        .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                    {
                        shell.context = true;
                        shell.rebuild(editor)?;
                    }
                    dirty = true;
                }
                if button == 1 {
                    let inside = shell
                        .viewport()
                        .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                        && !editor.state.palette
                        && !shell.context;
                    if pressed && !shell.modifiers[2] {
                        if !inside {
                            shell.commit_fields(editor)?;
                            shell.event(InputEvent::PointerDown(shell.pointer), clipboard)?;
                            shell.begin_drag();
                        } else {
                            shell.commit_fields(editor)?;
                            shell.input.blur(&mut shell.ui);
                            let rect = shell.viewport().ok_or("viewport missing")?;
                            if let Some(handles) = overlay::handles(view, &editor.state, rect)
                                .filter(|h| overlay::hit(h, shell.pointer).is_some())
                            {
                                let axis = overlay::hit(&handles, shell.pointer)
                                    .ok_or("gizmo axis missing")?;
                                editor.begin_gesture()?;
                                self.gizmo = Some(GizmoDrag {
                                    axis,
                                    start: shell.pointer,
                                    vector: Vec2::from(handles.ends[axis])
                                        - Vec2::from(handles.origin),
                                    length: handles.length,
                                });
                            } else {
                                let x = ((shell.pointer[0] - rect.x) / rect.w
                                    * renderer.size()[0] as f32)
                                    .max(0.) as u32;
                                let y = ((shell.pointer[1] - rect.y) / rect.h
                                    * renderer.size()[1] as f32)
                                    .max(0.) as u32;
                                if x < renderer.size()[0] && y < renderer.size()[1] {
                                    let id = renderer.entity_at([x, y])?;
                                    let entity = view
                                        .entities()
                                        .keys()
                                        .find(|address| {
                                            Some(crate::world::render_id(address)) == id
                                        })
                                        .filter(|a| !editor.state.locked.contains(*a));
                                    if let Some(path) = shell.pick_field.take() {
                                        if let Some(entity) = entity {
                                            editor.apply(
                                                crate::inspector::edit_commands(
                                                    &editor.state.selection,
                                                    &path,
                                                    &json!(entity),
                                                ),
                                                "Pick entity reference",
                                                None,
                                            )?;
                                        }
                                    } else {
                                        editor.action("editor.select",json!({"entities":entity.into_iter().collect::<Vec<_>>(),"toggle":shell.modifiers[1],"additive":shell.modifiers[0]}))?;
                                    }
                                }
                            }
                        }
                    } else if !pressed {
                        if self.gizmo.take().is_some() {
                            editor.finish_gesture(false)?;
                            scene_dirty = true;
                        } else if !shell.drop_at(editor)? {
                            let slider = shell
                                .input
                                .captured()
                                .and_then(|id| shell.ui.widgets.get(&id))
                                .is_some_and(|w| {
                                    matches!(w, ara_ui::widgets::Widget::Slider { .. })
                                });
                            shell.event(InputEvent::PointerUp(shell.pointer), clipboard)?;
                            shell.dispatch(editor)?;
                            if slider {
                                shell.commit_fields(editor)?;
                            }
                        } else {
                            shell.event(InputEvent::Cancel, clipboard)?;
                        }
                    }
                    dirty = true;
                }
            }
            Event::Wheel(delta) => {
                editor.input_active();
                if shell
                    .viewport()
                    .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                {
                    let mut camera = editor.state.camera;
                    camera.zoom(delta[1]);
                    set_camera(editor, &camera)?;
                    scene_dirty = true;
                } else if !shell.scroll_hierarchy(-delta[1] * 52.)? {
                    shell.event(
                        InputEvent::Wheel {
                            position: shell.pointer,
                            delta: [-delta[0] * 26., -delta[1] * 26.],
                        },
                        clipboard,
                    )?;
                }
                dirty = true;
            }
        }
        Ok((dirty, scene_dirty, quit))
    }
    pub fn fly(&mut self, editor: &mut Editor, shell: &Shell, dt: f32) -> ara_ui::Result<bool> {
        let mut dirty = false;
        let mut scene_dirty = false;
        if self.buttons[3] {
            let direction = Vec3::new(
                (i32::from(self.keys.contains(&7)) - i32::from(self.keys.contains(&4))) as f32,
                (i32::from(self.keys.contains(&8)) - i32::from(self.keys.contains(&20))) as f32,
                (i32::from(self.keys.contains(&22)) - i32::from(self.keys.contains(&26))) as f32,
            );
            if direction.length_squared() > 0. {
                let mut camera = editor.state.camera;
                camera.fly(
                    direction.normalize(),
                    dt * if shell.modifiers[0] { 12. } else { 4. },
                );
                set_camera(editor, &camera)?;
                scene_dirty = true;
                dirty = true;
            }
        }

        Ok(dirty || scene_dirty)
    }
}
