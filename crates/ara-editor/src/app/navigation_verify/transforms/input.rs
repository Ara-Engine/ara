//! Events target retained controls and the exact visible production handle geometry.
use super::*;

fn key(window: &Window, code: i32, pressed: bool, events: &mut Vec<(u32, Event)>) {
    events.push((
        window.id(),
        Event::Key {
            scancode: code,
            pressed,
            repeat: false,
        },
    ));
}
fn motion(
    window: &Window,
    point: [f32; 2],
    delta: [f32; 2],
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) {
    events.push((
        window.id(),
        Event::MouseMotion {
            position: point.map(|v| v / scale),
            delta: delta.map(|v| v / scale),
        },
    ));
}
fn button(window: &Window, pressed: bool, events: &mut Vec<(u32, Event)>) {
    events.push((window.id(), Event::MouseButton { button: 1, pressed }));
}
fn click(
    shell: &Shell,
    id: &str,
    window: &Window,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let index = shell
        .shown_node(id)
        .ok_or_else(|| format!("Native J1 control {id} is not mounted"))?;
    let node = &shell.ui.nodes[index];
    let rect = node.rect.intersection(node.clip);
    if rect.w < 4. || rect.h < 4. {
        return Err(format!("Native J1 control {id} is clipped").into());
    }
    motion(window, centre(rect), [0.; 2], scale, events);
    button(window, true, events);
    button(window, false, events);
    Ok(())
}
impl Proof {
    #[allow(
        clippy::too_many_arguments,
        reason = "native owners and input coordinates are borrowed independently"
    )]
    pub(in crate::app::navigation_verify) fn inject(
        &mut self,
        age: u32,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        scene_ready: bool,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        if !scale.is_finite() || scale <= 0. {
            return Err("Native J1 pointer scale is invalid".into());
        }
        if [1, 5, 10, 13, 16, 19].contains(&age) && !scene_ready {
            return Ok(false);
        }
        let began = events.len();
        match age {
            2 => click(
                shell,
                match self.action {
                    Action::Local => "tool-space",
                    Action::Centre | Action::Pivot => "tool-Rotate",
                    Action::Precision | Action::Cancel => "snap-toggle",
                    Action::World => "tool-Translate",
                },
                window,
                scale,
                events,
            )?,
            3 if self.action == Action::Centre => {
                click(shell, "tool-pivot", window, scale, events)?
            }
            4 if self.action.rotate() => click(shell, "snap-toggle", window, scale, events)?,
            5 => {
                if editor.state.local != (self.action == Action::Local)
                    || editor.state.centre != (self.action == Action::Centre)
                    || editor.state.snapping != self.action.snap()
                    || editor.state.tool
                        != if self.action.rotate() {
                            crate::state::Tool::Rotate
                        } else {
                            crate::state::Tool::Translate
                        }
                {
                    return Err("Native J1 toolbar did not reach the requested local/world, pivot/centre, snapping and tool state".into());
                }
                let viewport = shell.viewport().ok_or("Native J1 Scene viewport missing")?;
                let view = shell.document_view(editor)?;
                let handles = overlay::handles(&view, &editor.state, viewport)
                    .ok_or("Native J1 handles missing")?;
                let valid = |point: [f32; 2]| {
                    viewport.contains(point[0], point[1])
                        && shell.ui.hit_test(point[0], point[1]).is_none()
                };
                if self.action.rotate() {
                    let candidate = (0..48)
                        .find_map(|n| {
                            let angle = (n as f32 + 0.5) * std::f32::consts::TAU / 48.;
                            let point = |angle: f32| {
                                overlay::project(
                                    &editor.state.camera,
                                    viewport,
                                    handles.pivot
                                        + Vec3::new(angle.cos(), angle.sin(), 0.) * handles.length,
                                )
                            };
                            let from = point(angle)?;
                            let to = point(angle + 37f32.to_radians())?;
                            (valid(from)
                                && valid(to)
                                && overlay::hit(&handles, from) == Some(overlay::Handle::Axis(2)))
                            .then_some((from, to, angle))
                        })
                        .ok_or("Native J1 cannot reach the real Z rotation ring")?;
                    self.from = candidate.0;
                    self.to = candidate.1;
                    self.geometry = json!({"axis":"world-Z","pivot":handles.pivot.to_array(),"ringRadius":handles.length,"rawDegrees":37,"snappedDegrees":30,"startRadians":candidate.2});
                } else {
                    let vector = Vec2::from(handles.ends[0]) - Vec2::from(handles.origin);
                    if vector.length() < 20. || handles.length <= 0. {
                        return Err("Native J1 X axis is not legible".into());
                    }
                    self.from = (Vec2::from(handles.origin) + vector * 0.65).to_array();
                    let amount = if self.action.snap() { 0.83 } else { 1.1 };
                    self.to =
                        (Vec2::from(self.from) + vector * (amount / handles.length)).to_array();
                    if !valid(self.from)
                        || !valid(self.to)
                        || overlay::hit(&handles, self.from) != Some(overlay::Handle::Axis(0))
                    {
                        return Err("Native J1 cannot reach its real X translation handle".into());
                    }
                    self.geometry = json!({"axis":handles.axes[0].to_array(),"pivot":handles.pivot.to_array(),"rawMetres":amount,"snappedMetres":if self.action.snap(){0.75}else{1.1},"handleLength":handles.length});
                }
                self.geometry["from"] = json!(self.from);
                self.geometry["to"] = json!(self.to);
                self.geometry["viewport"] = json!(viewport);
            }
            6 => motion(window, self.from, [0.; 2], scale, events),
            7 => button(window, true, events),
            8 => {
                let half = (Vec2::from(self.from) + Vec2::from(self.to)) * 0.5;
                motion(
                    window,
                    half.to_array(),
                    (half - Vec2::from(self.from)).to_array(),
                    scale,
                    events,
                );
            }
            9 => motion(
                window,
                self.to,
                (Vec2::from(self.to) - Vec2::from(self.from)).to_array(),
                scale,
                events,
            ),
            11 if self.action == Action::Cancel => key(window, 41, true, events),
            11 => button(window, false, events),
            12 if self.action == Action::Cancel => {
                button(window, false, events);
                key(window, 41, false, events);
            }
            14 | 17 if self.action != Action::Cancel => {
                let code = if age == 14 { 29 } else { 28 };
                key(window, 224, true, events);
                key(window, code, true, events);
                key(window, code, false, events);
                key(window, 224, false, events);
            }
            _ => {}
        }
        self.native_events += events.len() - began;
        Ok(true)
    }
}
