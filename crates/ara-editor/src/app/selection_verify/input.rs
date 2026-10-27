use super::*;
use crate::shell::Binding;

fn key(events: &mut Vec<(u32, Event)>, code: i32, pressed: bool) {
    events.push((
        0,
        Event::Key {
            scancode: code,
            pressed,
            repeat: false,
        },
    ));
}
fn tap(events: &mut Vec<(u32, Event)>, code: i32) {
    key(events, code, true);
    key(events, code, false);
}
fn motion(events: &mut Vec<(u32, Event)>, point: [f32; 2], delta: [f32; 2], scale: f32) {
    events.push((
        0,
        Event::MouseMotion {
            position: point.map(|v| v / scale),
            delta: delta.map(|v| v / scale),
        },
    ));
}
fn button(events: &mut Vec<(u32, Event)>, pressed: bool) {
    events.push((0, Event::MouseButton { button: 1, pressed }));
}
fn click(events: &mut Vec<(u32, Event)>, point: [f32; 2], scale: f32) {
    motion(events, point, [0.; 2], scale);
    button(events, true);
    button(events, false);
}
fn row_point(shell: &Shell, entity: &str) -> Option<[f32; 2]> {
    shell.bindings.iter().find_map(|(id, binding)| {
        if !matches!(binding,Binding::Select(id) if id==entity) {
            return None;
        }
        let node = shell.ui.by_id(*id)?;
        if !shell.ui.shown(node) {
            return None;
        }
        let rect = shell.ui.nodes[node].rect;
        Some([rect.x + rect.w * 0.5, rect.y + rect.h * 0.5])
    })
}
fn project_bounds(camera: &crate::state::Camera, rect: Box2, bounds: [Vec3; 2]) -> Option<Box2> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for index in 0..8 {
        let point = Vec3::new(
            if index & 1 == 0 {
                bounds[0].x
            } else {
                bounds[1].x
            },
            if index & 2 == 0 {
                bounds[0].y
            } else {
                bounds[1].y
            },
            if index & 4 == 0 {
                bounds[0].z
            } else {
                bounds[1].z
            },
        );
        let point = overlay::project(camera, rect, point)?;
        if !point.into_iter().all(f32::is_finite) {
            return None;
        }
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    Some(Box2 {
        x: min[0] - 16.,
        y: min[1] - 16.,
        w: max[0] - min[0] + 32.,
        h: max[1] - min[1] + 32.,
    })
}
impl SelectionCheck {
    fn wait(&mut self) -> ara_ui::Result<()> {
        if self.waited >= PATIENCE {
            return Err("Selection fixture geometry/control did not become reachable within the bounded native journey".into());
        }
        self.waiting = true;
        Ok(())
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "matches the native event injection boundary"
    )]
    pub(super) fn input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        _window: &Window,
        scale: f32,
        scene: &viewport::SceneView,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !scale.is_finite() || scale <= 0. {
            return Err("Selection input scale is invalid".into());
        }
        if self.age == 0 {
            events.push((0, Event::FocusLost));
            return self.setup(act, editor, shell);
        }
        if act.hierarchy() {
            return self.hierarchy_input(act, shell, scale, events);
        }
        if self.age == 7 {
            let Some(viewport) = shell.viewport() else {
                return self.wait();
            };
            // Retained native geometry, not a guessed unit cube, determines both marquee corners.
            if !MESHES.iter().all(|id| {
                scene
                    .selected_bounds(&[(*id).into()], &shell.outlines.channels)
                    .is_some()
            }) {
                return self.wait();
            }
            if act.duplicate() {
                let view = shell.document_view(editor)?;
                let Some(handles) = overlay::handles(&view, &editor.state, viewport) else {
                    return self.wait();
                };
                let axis = glam::Vec2::from(handles.ends[0]) - glam::Vec2::from(handles.origin);
                if axis.length() < 20. {
                    return Err("Selection fixture X gizmo is not legible".into());
                }
                self.from = (glam::Vec2::from(handles.origin) + axis * 0.65).to_array();
                self.to = (glam::Vec2::from(self.from) + axis.normalize() * 80.).to_array();
                if overlay::hit(&handles, self.from) != Some(overlay::Handle::Axis(0)) {
                    return Err(
                        "Native duplicate probe did not resolve its real X gizmo handle".into(),
                    );
                }
            } else {
                let Some(bounds) = scene.selected_bounds(
                    &MESHES[..2]
                        .iter()
                        .map(|id| (*id).into())
                        .collect::<Vec<_>>(),
                    &shell.outlines.channels,
                ) else {
                    return self.wait();
                };
                let Some(rect) = project_bounds(&editor.state.camera, viewport, bounds) else {
                    return self.wait();
                };
                self.from = [rect.x, rect.y];
                self.to = [rect.x + rect.w, rect.y + rect.h];
                if act == Act::MarqueeReverse {
                    std::mem::swap(&mut self.from, &mut self.to);
                }
            }
            if ![self.from, self.to].into_iter().all(|point| {
                viewport.contains(point[0], point[1])
                    && shell.ui.hit_test(point[0], point[1]).is_none()
            }) {
                return Err("Native selection geometry is obstructed by editor UI".into());
            }
            motion(events, self.from, [0.; 2], scale);
        } else if self.age == 8 {
            match act {
                Act::MarqueeAdd => key(events, 225, true),
                Act::MarqueeSubtract => key(events, 224, true),
                act if act.duplicate() => key(events, 226, true),
                _ => {}
            }
            button(events, true);
        } else if self.age == 10 {
            motion(
                events,
                self.to,
                [self.to[0] - self.from[0], self.to[1] - self.from[1]],
                scale,
            );
        } else if self.age == 13 {
            if matches!(act, Act::MarqueeEscape | Act::DuplicateEscape) {
                key(events, 41, true);
            } else {
                button(events, false);
            }
        } else if self.age == 14 {
            if matches!(act, Act::MarqueeEscape | Act::DuplicateEscape) {
                button(events, false);
                key(events, 41, false);
            }
            for modifier in [225, 224, 226] {
                key(events, modifier, false);
            }
        } else if act == Act::Duplicate && self.age == 19 {
            key(events, 224, true);
            tap(events, 29);
            key(events, 224, false);
        } else if act == Act::Duplicate && self.age == 23 {
            key(events, 224, true);
            tap(events, 28);
            key(events, 224, false);
        }
        Ok(())
    }
    fn hierarchy_input(
        &mut self,
        act: Act,
        shell: &Shell,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let destination = match (act, self.age) {
            (Act::KeyboardEscape, 7) => Some((5, false, false)),
            (_, 7) => Some((2, false, false)),
            (Act::ShiftClick, 9) => Some((6, true, false)),
            (Act::CtrlToggle | Act::CtrlShift, 9) => Some((5, false, true)),
            (Act::CtrlToggle, 11) => Some((2, false, true)),
            (Act::CtrlShift, 11) => Some((8, true, true)),
            (Act::DeletedAnchor, 13) => Some((6, true, false)),
            _ => None,
        };
        if let Some((index, shift, control)) = destination {
            let Some(point) = row_point(shell, &row(index)) else {
                return self.wait();
            };
            self.from = point;
            self.to = point;
            if shift {
                key(events, 225, true);
            }
            if control {
                key(events, 224, true);
            }
            click(events, point, scale);
            if control {
                key(events, 224, false);
            }
            if shift {
                key(events, 225, false);
            }
        }
        match (act, self.age) {
            (Act::KeyboardEscape, 9) => {
                key(events, 225, true);
                tap(events, 77);
                key(events, 225, false);
            }
            (Act::KeyboardEscape, 13) => tap(events, 41),
            (Act::DeletedAnchor, 9) => tap(events, 76),
            _ => {}
        }
        Ok(())
    }
}
