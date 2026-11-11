//! Transform gizmo input in the Scene view: hover lights the handle under the pointer, a
//! press on one starts an editor gesture (`begin_gesture_tool`), moves drive it, release
//! commits it as one undo step and Escape or a lost capture cancels. Geometry and hit tests
//! are `crate::overlay`'s, the same the compositor draws; this file only maps pointer
//! movement to the gesture's delta (along an axis, across a plane, around a ring). A move
//! with vertex or surface snapping on asks the host instead (`studio::snapping`).
use glam::{Vec2, Vec3};

use super::Studio;
use crate::overlay::{self, Handle, Handles};
use crate::state::Tool;
use ara_ui::paint::Box2;

/// A handle being dragged.
#[derive(Clone, Debug)]
pub struct Drag {
    /// The grabbed handle.
    pub handle: Handle,
    /// Where the press was, view px.
    start: [f32; 2],
    /// The axis on screen, origin to end (axis handles).
    vector: Vec2,
    /// The arm's world length.
    length: f32,
    /// Ring drags: pivot, axis and the world point under the press.
    rotation: Option<(Vec3, Vec3, Vec3)>,
    /// Plane and centre moves: the plane normal and the world point under the press.
    plane: Option<(Vec3, Vec3)>,
    /// The gizmo's world axes.
    axes: [Vec3; 3],
    /// The pivot.
    pivot: Vec3,
    /// The view, view px.
    rect: Box2,
}

/// The view's rect for a size in view px.
fn rect(size: (f32, f32)) -> Box2 {
    Box2 {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    }
}

fn handles(s: &Studio, size: (f32, f32)) -> Option<Handles> {
    overlay::handles_in(&s.resolved, &s.editor.state, rect(size))
}

/// Lights the handle under `at`; whether that changed.
pub(super) fn hover(s: &mut Studio, at: [f32; 2], size: (f32, f32)) -> bool {
    let next = handles(s, size).and_then(|h| overlay::hit(&h, at));
    let changed = next != s.local.gizmo_hover;
    s.local.gizmo_hover = next;
    changed
}

/// Starts a drag when `at` is on a handle; whether it did.
pub(super) fn begin(s: &mut Studio, at: [f32; 2], size: (f32, f32)) -> bool {
    let Some(all) = handles(s, size) else {
        return false;
    };
    let Some(handle) = overlay::hit(&all, at) else {
        return false;
    };
    let tool = handle.tool(s.editor.state.tool);
    let h = all.for_tool(tool);
    if let Err(error) = s.editor.begin_gesture_tool(tool) {
        s.editor.report_failure(error.as_ref());
        return false;
    }
    let camera = s.editor.state.camera;
    let rect = rect(size);
    let forward = (camera.target - camera.position()).normalize_or_zero();
    let (vector, rotation, plane) = match handle.base() {
        Handle::Axis(axis) => (
            Vec2::from(h.ends[axis]) - Vec2::from(h.origin),
            (tool == Tool::Rotate)
                .then(|| {
                    overlay::plane_point(&camera, rect, at, h.pivot, h.axes[axis])
                        .map(|start| (h.pivot, h.axes[axis], start))
                })
                .flatten(),
            None,
        ),
        Handle::Plane(axis) => (
            Vec2::ONE,
            None,
            overlay::plane_point(&camera, rect, at, h.pivot, h.axes[axis])
                .map(|start| (h.axes[axis], start)),
        ),
        _ => (
            Vec2::new(1.0, -1.0),
            None,
            overlay::plane_point(&camera, rect, at, h.pivot, forward).map(|start| (forward, start)),
        ),
    };
    s.local.gizmo = Some(Drag {
        handle,
        start: at,
        vector,
        length: h.length,
        rotation,
        plane,
        axes: h.axes,
        pivot: h.pivot,
        rect,
    });
    true
}

/// Whether setting `key` is on.
fn on(s: &Studio, key: &str) -> bool {
    s.editor
        .state
        .settings
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Moves the gesture to the pointer at `at`: by the handle's geometry, or, moving with
/// vertex snapping (the overlay's, or V held) or surface snapping on, by the host's snap.
pub(super) fn drive(s: &mut Studio, at: [f32; 2]) {
    let Some(drag) = s.local.gizmo.clone() else {
        return;
    };
    let moving = s
        .editor
        .gesture
        .as_ref()
        .is_some_and(|g| matches!(g.tool, Tool::Translate | Tool::Universal));
    let vertex = s.local.vertex_key || on(s, "viewport.vertexSnap");
    if moving && (vertex || on(s, "viewport.surfaceSnap")) {
        s.local.snap = Some(super::snapping::Request {
            at,
            start: drag.start,
            rect: drag.rect,
            pivot: drag.pivot,
            vertex,
        });
        return;
    }
    let camera = s.editor.state.camera;
    let Some(gesture) = s.editor.gesture.as_mut() else {
        return;
    };
    let delta = match drag.handle.base() {
        Handle::Axis(axis) => {
            let movement = Vec2::from(at) - Vec2::from(drag.start);
            let along = movement.dot(drag.vector) / drag.vector.length_squared().max(0.01);
            let amount = match gesture.tool {
                Tool::Translate | Tool::Universal => along * drag.length,
                Tool::Rotate => match drag.rotation {
                    Some((pivot, normal, start)) => {
                        overlay::plane_point(&camera, drag.rect, at, pivot, normal)
                            .map(|point| {
                                let a = (start - pivot).normalize_or_zero();
                                let b = (point - pivot).normalize_or_zero();
                                normal.dot(a.cross(b)).atan2(a.dot(b)).to_degrees()
                            })
                            .unwrap_or(0.0)
                    }
                    None => along * 90.0,
                },
                Tool::Scale => along,
            };
            let mut delta = Vec3::ZERO;
            delta[axis] = amount;
            delta
        }
        Handle::Plane(_) | Handle::Centre if gesture.tool == Tool::Translate => {
            let world = drag
                .plane
                .and_then(|(normal, start)| {
                    overlay::plane_point(&camera, drag.rect, at, drag.pivot, normal)
                        .map(|p| p - start)
                })
                .unwrap_or(Vec3::ZERO);
            let mut delta = Vec3::new(
                world.dot(drag.axes[0]),
                world.dot(drag.axes[1]),
                world.dot(drag.axes[2]),
            );
            if let Handle::Plane(axis) = drag.handle {
                delta[axis] = 0.0;
            }
            delta
        }
        // The scale tool's centre: uniform scale.
        _ => {
            let movement = Vec2::from(at) - Vec2::from(drag.start);
            Vec3::splat((movement.x - movement.y) / 100.0)
        }
    };
    gesture.update(delta);
}

/// Ends the drag: commits it (one undo step) or cancels it.
pub(super) fn end(s: &mut Studio, cancel: bool) {
    if s.local.gizmo.take().is_none() {
        return;
    }
    match s.editor.finish_gesture(cancel) {
        Ok(_) => s.resolve(),
        Err(error) => s.editor.report_failure(&error),
    }
}
