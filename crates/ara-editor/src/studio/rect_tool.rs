//! The Rect tool (T), as Unity's: a rectangle round the selection's rendered bounds, in the
//! plane that faces the camera most, so XY in 2D mode. One object, or several sharing a
//! rotation, get the rectangle in their own frame (`frame_rotation`), so a drag resizes them
//! exactly as the rectangle shows; a selection of mixed rotations gets a world rectangle,
//! and each object scales only along its own axis closest to the dragged one
//! (`Gesture::rect_resize`). Drag an edge or a corner to resize, the opposite side staying put
//! (Alt resizes about the centre, Shift on a corner keeps the shape), or inside to move in
//! that plane. The host publishes the bounds in that frame (`Local::rect_bounds`) from the
//! presented scene; a drag is an editor gesture, so it previews live and commits as one undo
//! step.
use glam::{Mat4, Quat, Vec2, Vec3};
use stillkit::el::El;

use super::Studio;
use crate::overlay;
use crate::state::{Camera, Tool};
use ara_ui::paint::Box2;

/// What a press grabbed: per in-plane axis, the side it moves (-1 the low one, 1 the high
/// one, 0 neither). Both 0 is the inside, which moves the rect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grip {
    /// Along the rect's first axis.
    pub u: i8,
    /// Along its second.
    pub v: i8,
}

/// The rect in its plane: `origin + axes[0] * x + axes[1] * y` for `x, y` within
/// `min..max`; `axes[2]` is the plane's normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// In-plane axes and the normal, unit length.
    pub axes: [Vec3; 3],
    /// A point on the plane: the bounds' centre.
    pub origin: Vec3,
    /// The rect's low corner, in plane coordinates.
    pub min: Vec2,
    /// Its high corner.
    pub max: Vec2,
}

/// The selection's rendered bounds as the host publishes them: a box in the frame
/// `rotation` (its axes `rotation * X, Y, Z`), from `min` to `max` in that frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectBounds {
    /// The frame: the shared rotation of the selection, or identity (world).
    pub rotation: Quat,
    /// The box's low corner, in the frame.
    pub min: Vec3,
    /// Its high corner.
    pub max: Vec3,
}

/// The Rect tool's frame for `selection`: the world rotation its roots (those under no other
/// selected entity) share, or identity when their rotations differ (a world rectangle).
pub(crate) fn frame_rotation(entities: &ara_doc::prefab::Resolved, selection: &[String]) -> Quat {
    let under_selected = |id: &str| {
        let mut at = entities.get(id).and_then(|e| e.parent.clone());
        while let Some(parent) = at {
            if selection.contains(&parent) {
                return true;
            }
            at = entities.get(&parent).and_then(|e| e.parent.clone());
        }
        false
    };
    let mut rotations = selection
        .iter()
        .filter(|id| !under_selected(id))
        .filter_map(|id| crate::world::world_transform(entities, id))
        .map(|world| world.to_scale_rotation_translation().1);
    let Some(first) = rotations.next() else {
        return Quat::IDENTITY;
    };
    match rotations.all(|r| r.dot(first).abs() > 0.99999) {
        true => first,
        false => Quat::IDENTITY,
    }
}

impl Frame {
    /// The rect round `bounds`, in the plane among its frame's axes whose normal is nearest
    /// the camera's view direction.
    pub fn around(bounds: RectBounds, camera: &Camera) -> Option<Self> {
        let RectBounds { rotation, .. } = bounds;
        let forward = (camera.target - camera.position()).normalize_or_zero();
        let all = [Vec3::X, Vec3::Y, Vec3::Z].map(|a| rotation * a);
        let normal = (0..3).max_by(|&a, &b| {
            forward
                .dot(all[a])
                .abs()
                .total_cmp(&forward.dot(all[b]).abs())
        })?;
        // Keep X before Y before Z across the screen.
        let (u, v) = match normal {
            0 => (all[2], all[1]),
            1 => (all[0], all[2]),
            _ => (all[0], all[1]),
        };
        let corners = [bounds.min, bounds.max];
        let origin = rotation * ((bounds.min + bounds.max) * 0.5);
        let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for corner in 0..8 {
            let p = rotation
                * Vec3::new(
                    corners[corner & 1].x,
                    corners[(corner >> 1) & 1].y,
                    corners[(corner >> 2) & 1].z,
                )
                - origin;
            let at = Vec2::new(p.dot(u), p.dot(v));
            min = min.min(at);
            max = max.max(at);
        }
        (origin.is_finite() && min.is_finite() && max.is_finite()).then_some(Self {
            axes: [u, v, all[normal]],
            origin,
            min,
            max,
        })
    }

    /// The world point at plane coordinates `at`.
    fn point(&self, at: Vec2) -> Vec3 {
        self.origin + self.axes[0] * at.x + self.axes[1] * at.y
    }

    /// The plane coordinates of world point `p` (on the plane).
    fn coords(&self, p: Vec3) -> Vec2 {
        let d = p - self.origin;
        Vec2::new(d.dot(self.axes[0]), d.dot(self.axes[1]))
    }

    /// The corners, counter-clockwise from the low one.
    fn corners(&self) -> [Vec3; 4] {
        [
            self.point(self.min),
            self.point(Vec2::new(self.max.x, self.min.y)),
            self.point(self.max),
            self.point(Vec2::new(self.min.x, self.max.y)),
        ]
    }

    /// The world map taking this rect onto `to` (same plane): each axis scales by the
    /// ratio of their extents and shifts with the low side. A flat axis only shifts.
    pub fn map_to(&self, to: &Frame) -> Mat4 {
        let (from, onto) = (self.max - self.min, to.max - to.min);
        let ratio = |a: f32, b: f32| if a.abs() > 1e-6 { b / a } else { 1.0 };
        let scale = Vec3::new(ratio(from.x, onto.x), ratio(from.y, onto.y), 1.0);
        let basis = Mat4::from_cols(
            self.axes[0].extend(0.0),
            self.axes[1].extend(0.0),
            self.axes[2].extend(0.0),
            self.origin.extend(1.0),
        );
        basis
            * Mat4::from_translation(to.min.extend(0.0))
            * Mat4::from_scale(scale)
            * Mat4::from_translation(-self.min.extend(0.0))
            * basis.inverse()
    }

    /// This rect with `grip` dragged by `by` (plane units). `centre` resizes about the
    /// middle, `keep` (corners) holds the aspect; no side passes the opposite one.
    pub fn dragged(&self, grip: Grip, by: Vec2, centre: bool, keep: bool) -> Self {
        let mut next = *self;
        if grip == INSIDE {
            next.min += by;
            next.max += by;
            return next;
        }
        let size = self.max - self.min;
        let mid = (self.min + self.max) * 0.5;
        // How far each axis grows: a high side grows with the drag, a low side against it.
        let grow = Vec2::new(f32::from(grip.u), f32::from(grip.v)) * by;
        let mut extent = size + grow * if centre { 2.0 } else { 1.0 };
        if keep && grip.u != 0 && grip.v != 0 && size.x > 1e-6 && size.y > 1e-6 {
            let k = extent / size;
            let k = if (k.x - 1.0).abs() > (k.y - 1.0).abs() {
                k.x
            } else {
                k.y
            };
            extent = size * k;
        }
        let extent = extent.max(size.min(Vec2::splat(1e-3)));
        for (axis, side) in [(0, grip.u), (1, grip.v)] {
            if side == 0 {
                continue;
            }
            let (low, high) = if centre {
                (
                    mid[axis] - extent[axis] * 0.5,
                    mid[axis] + extent[axis] * 0.5,
                )
            } else if side > 0 {
                (self.min[axis], self.min[axis] + extent[axis])
            } else {
                (self.max[axis] - extent[axis], self.max[axis])
            };
            next.min[axis] = low;
            next.max[axis] = high;
        }
        next
    }
}

/// A Rect drag: the grip, the rect at the press and where it was pressed (plane units).
#[derive(Clone, Debug)]
pub struct Drag {
    grip: Grip,
    start: Frame,
    from: Vec2,
    /// The rect now, drawn while the drag runs.
    now: Frame,
    rect: Box2,
}

/// The view's rect for a size in view px.
fn view(size: (f32, f32)) -> Box2 {
    Box2 {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    }
}

/// The rect to show: the drag's, else the one round the published bounds while the Rect
/// tool is on with something selected.
fn frame(s: &Studio) -> Option<Frame> {
    if let Some(drag) = &s.local.rect_drag {
        return Some(drag.now);
    }
    let state = &s.editor.state;
    if state.tool != Tool::Rect || state.hide_gizmos || state.game_view {
        return None;
    }
    Frame::around(s.local.rect_bounds?, &state.camera)
}

/// The rect shown now: the drag's, or the one round the published bounds (tests).
#[cfg(test)]
pub(super) fn shown(s: &Studio) -> Option<Frame> {
    frame(s)
}

/// The rect's corners on screen (view px), when all are in front of the camera.
fn screen(frame: &Frame, camera: &Camera, rect: Box2) -> Option<[Vec2; 4]> {
    let c = frame.corners();
    Some([
        overlay::project(camera, rect, c[0])?.into(),
        overlay::project(camera, rect, c[1])?.into(),
        overlay::project(camera, rect, c[2])?.into(),
        overlay::project(camera, rect, c[3])?.into(),
    ])
}

/// Corner i's sides, counter-clockwise from the low one (as `Frame::corners`).
const SIDES: [Grip; 4] = [
    Grip { u: -1, v: -1 },
    Grip { u: 1, v: -1 },
    Grip { u: 1, v: 1 },
    Grip { u: -1, v: 1 },
];
/// The inside: a move.
const INSIDE: Grip = Grip { u: 0, v: 0 };

/// Edge i, from corner i to i + 1: the side those corners share.
fn edge(i: usize) -> Grip {
    let (a, b) = (SIDES[i], SIDES[(i + 1) % 4]);
    Grip {
        u: if a.u == b.u { a.u } else { 0 },
        v: if a.v == b.v { a.v } else { 0 },
    }
}

/// The grip at `at` (view px): a corner, then an edge, then the inside.
fn hit(corners: &[Vec2; 4], at: Vec2) -> Option<Grip> {
    if let Some(i) = (0..4).find(|&i| corners[i].distance(at) <= overlay::HIT + 2.0) {
        return Some(SIDES[i]);
    }
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let t = ((at - a).dot(b - a) / (b - a).length_squared().max(1e-6)).clamp(0.0, 1.0);
        if (a + (b - a) * t).distance(at) <= overlay::HIT {
            return Some(edge(i));
        }
    }
    // Inside: the same side of every edge.
    let side = |i: usize| (corners[(i + 1) % 4] - corners[i]).perp_dot(at - corners[i]);
    let first = side(0);
    (0..4).all(|i| side(i) * first >= 0.0).then_some(INSIDE)
}

/// The shown rect's corners in a view of `size` (view px), counter-clockwise from the low
/// one.
pub(super) fn corners(s: &Studio, size: (f32, f32)) -> Option<[Vec2; 4]> {
    screen(&frame(s)?, &s.editor.state.camera, view(size))
}

/// Lights the grip under `at`; whether that changed.
pub(super) fn hover(s: &mut Studio, at: [f32; 2], size: (f32, f32)) -> bool {
    let next = corners(s, size).and_then(|c| hit(&c, at.into()));
    let changed = next != s.local.rect_hover;
    s.local.rect_hover = next;
    changed
}

/// Starts a drag when `at` is on the rect; whether it did.
pub(super) fn begin(s: &mut Studio, at: [f32; 2], size: (f32, f32)) -> bool {
    let rect = view(size);
    let camera = s.editor.state.camera;
    let Some(start) = frame(s) else {
        return false;
    };
    let Some(grip) = screen(&start, &camera, rect).and_then(|c| hit(&c, at.into())) else {
        return false;
    };
    let Some(from) = overlay::plane_point(&camera, rect, at, start.origin, start.axes[2]) else {
        return false;
    };
    if let Err(error) = s.editor.begin_gesture_tool(Tool::Rect) {
        s.editor.report_failure(error.as_ref());
        return false;
    }
    s.local.rect_drag = Some(Drag {
        grip,
        start,
        from: start.coords(from),
        now: start,
        rect,
    });
    true
}

/// Resizes or moves to the pointer at `at`; Alt resizes about the centre, Shift keeps a
/// corner drag's shape.
pub(super) fn drive(s: &mut Studio, at: [f32; 2], alt: bool, shift: bool) {
    let camera = s.editor.state.camera;
    let Some(drag) = s.local.rect_drag.as_mut() else {
        return;
    };
    let start = drag.start;
    let Some(point) = overlay::plane_point(&camera, drag.rect, at, start.origin, start.axes[2])
    else {
        return;
    };
    let now = start.dragged(drag.grip, start.coords(point) - drag.from, alt, shift);
    drag.now = now;
    let size = now.max - now.min;
    if let Some(gesture) = s.editor.gesture.as_mut() {
        let (from, to) = (start.max - start.min, now.max - now.min);
        let ratio = |a: f32, b: f32| if a.abs() > 1e-6 { b / a } else { 1.0 };
        gesture.rect_resize(
            start.map_to(&now),
            [start.axes[0], start.axes[1]],
            Vec2::new(ratio(from.x, to.x), ratio(from.y, to.y)),
        );
    }
    super::note(s, format!("{:.2} × {:.2} m", size.x, size.y));
}

/// Ends the drag: commits it (one undo step) or cancels it.
pub(super) fn end(s: &mut Studio, cancel: bool) {
    if s.local.rect_drag.take().is_none() {
        return;
    }
    match s.editor.finish_gesture(cancel) {
        Ok(_) => s.resolve(),
        Err(error) => s.editor.report_failure(&error),
    }
}

/// The rect over the Scene view: its outline, its corners as handles and a dot at its
/// centre, the grabbed or hovered part lit. It projects at paint time, at the view's size.
pub(super) fn layer(s: &Studio) -> Option<El<Studio>> {
    let shown = frame(s)?;
    let camera = s.editor.state.camera;
    let lit = s
        .local
        .rect_drag
        .as_ref()
        .map(|d| d.grip)
        .or(s.local.rect_hover);
    let ink = stillkit::Rgba::new(1.0, 1.0, 1.0, 0.95);
    // A dark rim under every mark keeps it legible on a light sky or floor.
    let rim = stillkit::Rgba::new(0.0, 0.0, 0.0, 0.45);
    let accent = s.look.c.primary.0;
    let paint = stillkit::el::paint(move |f, rect, _| {
        let k = f.px(1.0);
        let size = (rect.width() / k, rect.height() / k);
        let Some(corners) = screen(&shown, &camera, view(size)) else {
            return;
        };
        let at = |p: Vec2| stillkit::Point::new(rect.left + p.x * k, rect.top + p.y * k);
        let dot = |f: &mut stillkit::Frame, p: Vec2, r: f32, on: bool| {
            let c = at(p);
            for (r, color) in [(r + 1.0, rim), (r, if on { accent } else { ink })] {
                let r = r * k;
                f.fill(
                    stillkit::Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r),
                    r,
                    color,
                );
            }
        };
        for i in 0..4 {
            let on = lit == Some(edge(i));
            let width = if on { 2.5 } else { 1.5 };
            for (width, color) in [(width + 2.0, rim), (width, if on { accent } else { ink })] {
                f.push(stillkit::Op::Line {
                    from: at(corners[i]),
                    to: at(corners[(i + 1) % 4]),
                    width: width * k,
                    color,
                });
            }
        }
        for (i, corner) in corners.iter().enumerate() {
            let on = lit == Some(SIDES[i]);
            dot(f, *corner, if on { 5.0 } else { 4.0 }, on);
        }
        dot(f, (corners[0] + corners[2]) * 0.5, 2.5, lit == Some(INSIDE));
    });
    Some(paint.absolute(0.0, 0.0).w_full().h_full())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Frame {
        Frame {
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            origin: Vec3::ZERO,
            min: Vec2::splat(-1.0),
            max: Vec2::splat(1.0),
        }
    }

    #[test]
    fn a_right_edge_drag_keeps_the_left_edge_and_scales_x() {
        let start = square();
        let now = start.dragged(Grip { u: 1, v: 0 }, Vec2::new(2.0, 5.0), false, false);
        assert_eq!(
            (now.min, now.max),
            (Vec2::new(-1.0, -1.0), Vec2::new(3.0, 1.0))
        );
        let map = start.map_to(&now);
        // The left edge stays; the centre moves to the new middle; x doubles.
        assert!(
            map.transform_point3(Vec3::new(-1.0, 0.0, 0.0))
                .abs_diff_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-5)
        );
        assert!(
            map.transform_point3(Vec3::ZERO)
                .abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-5)
        );
        assert!(
            map.transform_vector3(Vec3::X)
                .abs_diff_eq(Vec3::X * 2.0, 1e-5)
        );
        assert!(map.transform_vector3(Vec3::Z).abs_diff_eq(Vec3::Z, 1e-5));
    }

    #[test]
    fn alt_resizes_about_the_centre_and_shift_keeps_a_corner_square() {
        let start = square();
        let centred = start.dragged(Grip { u: 1, v: 0 }, Vec2::new(1.0, 0.0), true, false);
        assert_eq!((centred.min.x, centred.max.x), (-2.0, 2.0));
        let kept = start.dragged(Grip { u: 1, v: 1 }, Vec2::new(2.0, 0.5), false, true);
        let size = kept.max - kept.min;
        assert!((size.x - size.y).abs() < 1e-5, "{size}");
        assert_eq!(kept.min, start.min);
    }

    #[test]
    fn a_side_never_passes_the_opposite_one_and_the_inside_moves() {
        let start = square();
        let crushed = start.dragged(Grip { u: -1, v: 0 }, Vec2::new(10.0, 0.0), false, false);
        assert!(crushed.max.x - crushed.min.x > 0.0);
        assert_eq!(crushed.max.x, 1.0);
        let moved = start.dragged(Grip { u: 0, v: 0 }, Vec2::new(0.5, -0.5), false, false);
        assert!(
            start
                .map_to(&moved)
                .transform_point3(Vec3::ZERO)
                .abs_diff_eq(Vec3::new(0.5, -0.5, 0.0), 1e-5)
        );
    }

    #[test]
    fn the_plane_faces_the_camera_and_flat_bounds_only_shift() {
        let camera = Camera {
            yaw: 0.0,
            pitch: 0.0,
            ..Camera::default()
        };
        let frame = Frame::around(
            RectBounds {
                rotation: Quat::IDENTITY,
                min: Vec3::new(-1.0, 0.0, -2.0),
                max: Vec3::new(1.0, 0.0, 2.0),
            },
            &camera,
        )
        .expect("a frame");
        let normal = frame.axes[2];
        let forward = (camera.target - camera.position()).normalize();
        assert!(normal.dot(forward).abs() > 0.7, "{normal} vs {forward}");
        // A flat axis has no extent to scale: dragging its edge shifts instead.
        let flat = Frame {
            min: Vec2::new(-1.0, 0.0),
            max: Vec2::new(1.0, 0.0),
            ..square()
        };
        let map =
            flat.map_to(&flat.dragged(Grip { u: 0, v: 1 }, Vec2::new(0.0, 1.0), false, false));
        assert!(map.is_finite());
    }

    #[test]
    fn hits_prefer_corners_then_edges_then_the_inside() {
        let c = [
            Vec2::new(0.0, 100.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(0.0, 0.0),
        ];
        assert_eq!(hit(&c, Vec2::new(2.0, 98.0)), Some(Grip { u: -1, v: -1 }));
        assert_eq!(hit(&c, Vec2::new(50.0, 100.0)), Some(Grip { u: 0, v: -1 }));
        assert_eq!(hit(&c, Vec2::new(100.0, 50.0)), Some(Grip { u: 1, v: 0 }));
        assert_eq!(hit(&c, Vec2::new(50.0, 50.0)), Some(Grip { u: 0, v: 0 }));
        assert_eq!(hit(&c, Vec2::new(150.0, 50.0)), None);
    }
}
