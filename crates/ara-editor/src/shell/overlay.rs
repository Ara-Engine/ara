//! Grid/handle projection and picking use the actual renderer camera and ID channel.
use crate::{
    gpu::Overlay,
    state::{Camera, EditorState, Tool},
    world::{DocumentView, EditWorldView},
};
use ara_ui::{paint::Box2, theme::EditorTheme};
use glam::{Vec2, Vec3};
/// Resource handle of the viewport image in the native UI display list.
pub fn viewport_image() -> u32 {
    ara_ui::source::stable_id("editor-viewport") as u32 & 0x7fff_ffff
}
/// Resource handle of the renderer's entity-ID image.
pub const ENTITY_IMAGE: u32 = 0x7fff_0001;
/// Project a world point into a logical UI viewport, rejecting points behind the camera.
pub fn project(camera: &Camera, rect: Box2, point: Vec3) -> Option<[f32; 2]> {
    let camera = camera
        .spec()
        .resolve([rect.w.max(1.) as u32, rect.h.max(1.) as u32])
        .ok()?;
    let clip = camera.projection * camera.view * point.extend(1.);
    if clip.w <= 0.01 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some([
        rect.x + (ndc.x + 1.) * 0.5 * rect.w,
        rect.y + (1. - ndc.y) * 0.5 * rect.h,
    ])
}
/// Intersect a pointer ray with the ground plane for drag-to-place.
pub fn ground_point(camera: &Camera, rect: Box2, point: [f32; 2]) -> Option<Vec3> {
    let c = camera
        .spec()
        .resolve([rect.w.max(1.) as u32, rect.h.max(1.) as u32])
        .ok()?;
    let inverse = (c.projection * c.view).inverse();
    let x = (point[0] - rect.x) / rect.w * 2. - 1.;
    let y = 1. - (point[1] - rect.y) / rect.h * 2.;
    let near = inverse * glam::Vec4::new(x, y, 1., 1.);
    let near = near.truncate() / near.w;
    let direction = (near - c.position).normalize();
    if direction.y.abs() < 0.00001 {
        return None;
    }
    let t = -c.position.y / direction.y;
    (t > 0.).then_some(c.position + direction * t)
}
/// One grabbable part of the transform gizmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// An axis arrow (or rotation ring, or scale cube) by index: X, Y, Z.
    Axis(usize),
    /// The plane square whose normal is this axis (moves in the other two).
    Plane(usize),
    /// The centre: free move in the view plane, or uniform scale.
    Centre,
}
/// Half the minimum hit target: every handle is grabbable within 16 px or more.
pub const HIT: f32 = 9.;
/// Gizmo arm length on screen at 100% gizmo size (Appendix C).
pub const ARM: f32 = 88.;
/// Projected axis handles and pivot, shared by drawing and pointer hit testing.
pub struct Handles {
    /// World pivot.
    pub pivot: Vec3,
    /// Image-space pivot.
    pub origin: [f32; 2],
    /// Image-space endpoints for the X, Y and Z handles.
    pub ends: [[f32; 2]; 3],
    /// Handle extent in world metres (constant on screen).
    pub length: f32,
    /// World-space axes, including the active entity's local orientation.
    pub axes: [Vec3; 3],
    /// World metres per logical pixel at the pivot.
    pub per_pixel: f32,
    tool: Tool,
    camera: Camera,
    rect: Box2,
}
impl Handles {
    /// One projected rotation-ring segment, shared by drawing and pointer hit testing.
    pub fn ring_segment(&self, axis: usize, n: usize) -> Option<[[f32; 2]; 2]> {
        let u = self.axes[(axis + 1) % 3];
        let v = self.axes[axis].cross(u);
        let a = n as f32 / 48. * std::f32::consts::TAU;
        let b = (n + 1) as f32 / 48. * std::f32::consts::TAU;
        let point = |angle: f32| self.pivot + (u * angle.cos() + v * angle.sin()) * self.length;
        Some([
            project(&self.camera, self.rect, point(a))?,
            project(&self.camera, self.rect, point(b))?,
        ])
    }
    /// World point on the rotation ring around `axis` at `angle` radians.
    fn ring_point(&self, axis: usize, angle: f32) -> Vec3 {
        let u = self.axes[(axis + 1) % 3];
        let v = self.axes[axis].cross(u);
        self.pivot + (u * angle.cos() + v * angle.sin()) * self.length
    }
    /// The plane handle with normal `axis`: the square between 20 % and 40 % along the
    /// other two arms, in their world plane, projected (a parallelogram on screen).
    /// `None` while either arm is foreshortened, when the square would be a sliver.
    pub fn plane_quad(&self, axis: usize) -> Option<[[f32; 2]; 4]> {
        let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
        let o = Vec2::from(self.origin);
        if (Vec2::from(self.ends[a]) - o).length() < 20.
            || (Vec2::from(self.ends[b]) - o).length() < 20.
        {
            return None;
        }
        let (u, v) = (self.axes[a] * self.length, self.axes[b] * self.length);
        let corner = |s: f32, t: f32| project(&self.camera, self.rect, self.pivot + u * s + v * t);
        Some([
            corner(0.2, 0.2)?,
            corner(0.4, 0.2)?,
            corner(0.4, 0.4)?,
            corner(0.2, 0.4)?,
        ])
    }
    /// Unit screen direction of an axis, or zero when it points at the camera.
    fn screen_dir(&self, axis: usize) -> Vec2 {
        (Vec2::from(self.ends[axis]) - Vec2::from(self.origin)).normalize_or_zero()
    }
}
/// Intersect a logical pointer ray with the chosen gizmo plane.
pub fn plane_point(
    camera: &Camera,
    rect: Box2,
    pointer: [f32; 2],
    pivot: Vec3,
    normal: Vec3,
) -> Option<Vec3> {
    let c = camera
        .spec()
        .resolve([rect.w.max(1.) as u32, rect.h.max(1.) as u32])
        .ok()?;
    let inverse = (c.projection * c.view).inverse();
    let near = inverse
        * glam::Vec4::new(
            (pointer[0] - rect.x) / rect.w * 2. - 1.,
            1. - (pointer[1] - rect.y) / rect.h * 2.,
            1.,
            1.,
        );
    let direction = (near.truncate() / near.w - c.position).normalize_or_zero();
    let denominator = direction.dot(normal);
    if denominator.abs() < 0.0001 {
        return None;
    }
    let t = (pivot - c.position).dot(normal) / denominator;
    (t > 0.).then_some(c.position + direction * t)
}
/// Resolve the selected pivot and orientation into image-space transform handles.
pub fn handles(view: &DocumentView, state: &EditorState, rect: Box2) -> Option<Handles> {
    if state.hide_gizmos || state.game_view {
        return None;
    }
    // Only the selection's transforms: composing the whole scene cost ~0.5 ms a frame.
    let world = |id: &str| crate::world::world_transform(view.entities(), id);
    let matrix = world(state.selection.first()?)?;
    let pivot = if state.centre {
        state
            .selection
            .iter()
            .filter_map(|s| world(s))
            .map(|m| m.transform_point3(Vec3::ZERO))
            .sum::<Vec3>()
            / state.selection.len() as f32
    } else {
        matrix.transform_point3(Vec3::ZERO)
    };
    // Constant screen size: convert the arm's pixels to metres at the pivot's depth.
    let depth = (pivot - state.camera.position()).length().max(0.01);
    let per_pixel = 2. * depth * (50f32.to_radians() * 0.5).tan() / rect.h.max(1.);
    let size = state.number("viewport.gizmoSize", 100.) as f32 / 100.;
    let length = ARM * size * per_pixel;
    let origin = project(&state.camera, rect, pivot)?;
    let (_, rotation, _) = matrix.to_scale_rotation_translation();
    let axes =
        [Vec3::X, Vec3::Y, Vec3::Z].map(|axis| if state.local { rotation * axis } else { axis });
    let ends =
        axes.map(|axis| project(&state.camera, rect, pivot + axis * length).unwrap_or(origin));
    Some(Handles {
        pivot,
        origin,
        ends,
        length,
        axes,
        per_pixel,
        tool: state.tool,
        camera: state.camera,
        rect,
    })
}
/// The handle under the pointer within the hit target, nearest first.
pub fn hit(handles: &Handles, pointer: [f32; 2]) -> Option<Handle> {
    let p = Vec2::from(pointer);
    let o = Vec2::from(handles.origin);
    if handles.tool == Tool::Rotate {
        return (0..3)
            .flat_map(|axis| {
                (0..48).filter_map(move |n| {
                    let [a, b] = handles.ring_segment(axis, n)?;
                    let (a, b) = (Vec2::from(a), Vec2::from(b));
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_squared().max(0.001)).clamp(0., 1.);
                    let d = p.distance(a + ab * t);
                    (d < HIT).then_some((Handle::Axis(axis), d))
                })
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| handle);
    }
    if p.distance(o) < HIT {
        return Some(Handle::Centre);
    }
    if handles.tool == Tool::Translate {
        for axis in 0..3 {
            if handles
                .plane_quad(axis)
                .is_some_and(|quad| inside(&quad, p, 2.))
            {
                return Some(Handle::Plane(axis));
            }
        }
    }
    (0..3)
        .filter_map(|axis| {
            let b = Vec2::from(handles.ends[axis]);
            let ab = b - o;
            if ab.length() < 4. {
                return None;
            }
            // Extend past the tip to cover the arrowhead.
            let t = ((p - o).dot(ab) / ab.length_squared()).clamp(0.15, 1.15);
            let d = p.distance(o + ab * t);
            (d < HIT).then_some((Handle::Axis(axis), d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| handle)
}
/// Whether `p` lies inside the convex quad, or within `margin` pixels of it.
fn inside(quad: &[[f32; 2]; 4], p: Vec2, margin: f32) -> bool {
    let corners = quad.map(Vec2::from);
    let area: f32 = (0..4)
        .map(|n| corners[n].perp_dot(corners[(n + 1) % 4]))
        .sum();
    (0..4).all(|n| {
        let (a, b) = (corners[n], corners[(n + 1) % 4]);
        let edge = b - a;
        edge.length() < 0.001 || area.signum() * edge.perp_dot(p - a) / edge.length() > -margin
    })
}
/// The corners of a screen square of half-size `half` around `centre`.
fn square(centre: Vec2, half: f32) -> [[f32; 2]; 4] {
    [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)]
        .map(|(x, y)| (centre + Vec2::new(x, y) * half).into())
}
/// `corners` pushed `by` pixels away from their centroid: the dark halo under a fill.
fn grow(corners: [[f32; 2]; 4], by: f32) -> [[f32; 2]; 4] {
    let centre = corners.iter().map(|c| Vec2::from(*c)).sum::<Vec2>() / 4.;
    corners.map(|c| {
        let c = Vec2::from(c);
        (c + (c - centre).normalize_or_zero() * by).into()
    })
}
/// Gizmo interaction state for drawing: hovered and dragged handles.
#[derive(Clone, Copy, Debug, Default)]
pub struct GizmoState {
    /// Handle under the pointer.
    pub hover: Option<Handle>,
    /// Handle being dragged; others fade and a constraint line appears.
    pub active: Option<Handle>,
}
/// Create semantic-token grid, silhouette review tints and transform handles.
pub fn build(
    view: &DocumentView,
    state: &EditorState,
    rect: Box2,
    renderer: &ara_render::Renderer,
    changed: &std::collections::BTreeMap<u64, &str>,
    gizmo: GizmoState,
) -> ara_ui::Result<Vec<Overlay>> {
    let tokens = if state.light {
        EditorTheme::Light
    } else {
        EditorTheme::Dark
    }
    .tokens()?;
    let color = |token: &str, alpha: f32| -> ara_ui::Result<[f32; 4]> {
        let hex = tokens
            .get(&format!("--{token}"))
            .ok_or("theme token missing")?
            .trim_start_matches('#');
        let number = u32::from_str_radix(hex, 16)?;
        Ok([
            ((number >> 16) & 255) as f32 / 255.,
            ((number >> 8) & 255) as f32 / 255.,
            (number & 255) as f32 / 255.,
            alpha,
        ])
    };
    let mut overlays = Vec::new();
    if state.game_view {
        return Ok(overlays);
    }
    let show = |key: &str| {
        state
            .settings
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    };
    let p = state.camera.position();
    if show("viewport.grid") {
        let spacing = 10f32.powf((state.camera.distance / 20.).max(0.01).log10().floor());
        overlays.push(Overlay::Grid {
            camera: [p.x, p.y, p.z, state.camera.yaw],
            parameters: [
                state.camera.pitch,
                25f32.to_radians().tan(),
                spacing,
                state.camera.distance * 6.,
            ],
            color: color("muted-foreground", 0.32)?,
            axes: [
                color("destructive-foreground", 0.42)?,
                color("info-foreground", 0.42)?,
            ],
        });
    }
    let outlined: Vec<u64> = if show("viewport.outline") {
        state
            .selection
            .iter()
            .map(|s| crate::world::render_id(s))
            .collect()
    } else {
        Vec::new()
    };
    for (index, (id, _)) in renderer.entities().iter().enumerate() {
        if outlined.contains(id) || changed.contains_key(id) {
            let token = changed.get(id).copied().unwrap_or("accent");
            overlays.push(Overlay::Outline {
                index: index as u32 + 1,
                color: color(token, 1.)?,
            });
        }
    }
    let Some(handles) = handles(view, state, rect) else {
        return Ok(overlays);
    };
    let halo = [0., 0., 0., 0.6];
    let hover = color("warning-foreground", 1.)?;
    let tints = [
        color("destructive-foreground", 1.)?,
        color("success-foreground", 1.)?,
        color("info-foreground", 1.)?,
    ];
    let faded = |c: [f32; 4], handle: Handle| -> [f32; 4] {
        match gizmo.active {
            Some(active) if active != handle => [c[0], c[1], c[2], c[3] * 0.15],
            _ if gizmo.hover == Some(handle) || gizmo.active == Some(handle) => hover,
            _ => c,
        }
    };
    let stroke =
        |overlays: &mut Vec<Overlay>, from: [f32; 2], to: [f32; 2], width: f32, c: [f32; 4]| {
            overlays.push(Overlay::Line {
                from,
                to,
                width: width + 1.,
                color: [halo[0], halo[1], halo[2], halo[3] * c[3]],
            });
            overlays.push(Overlay::Line {
                from,
                to,
                width,
                color: c,
            });
        };
    // Opaque fills (cones, cubes) sit on a 1 px dark halo, like the strokes.
    let solid = |overlays: &mut Vec<Overlay>, corners: [[f32; 2]; 4], c: [f32; 4]| {
        overlays.push(Overlay::Quad {
            corners: grow(corners, 1.),
            color: [halo[0], halo[1], halo[2], halo[3] * c[3]],
        });
        overlays.push(Overlay::Quad { corners, color: c });
    };
    let lit = |handle: Handle| gizmo.hover == Some(handle) || gizmo.active == Some(handle);
    let o = Vec2::from(handles.origin);
    // While dragging, a thin constraint line through the pivot along each moving axis.
    let constrained = match gizmo.active {
        Some(Handle::Axis(axis)) if state.tool != Tool::Rotate => [Some(axis), None],
        Some(Handle::Plane(axis)) => [Some((axis + 1) % 3), Some((axis + 2) % 3)],
        _ => [None, None],
    };
    for axis in constrained.into_iter().flatten() {
        let d = handles.screen_dir(axis) * 2000.;
        let mut line = tints[axis];
        line[3] = 0.5;
        overlays.push(Overlay::Line {
            from: (o - d).into(),
            to: (o + d).into(),
            width: 0.5,
            color: line,
        });
    }
    if state.tool == Tool::Rotate {
        // The half of each ring facing away from the camera draws at 25 %, so the three
        // rings read as a sphere rather than a tangle.
        let toward = (handles.pivot - handles.camera.position()).normalize_or_zero();
        for (axis, tint) in tints.iter().enumerate() {
            let c = faded(*tint, Handle::Axis(axis));
            let width = if lit(Handle::Axis(axis)) { 1.25 } else { 0.75 };
            for n in 0..48 {
                let Some([from, to]) = handles.ring_segment(axis, n) else {
                    continue;
                };
                let middle = (n as f32 + 0.5) / 48. * std::f32::consts::TAU;
                let behind = (handles.ring_point(axis, middle) - handles.pivot).dot(toward) > 0.;
                let mut c = c;
                if behind && !lit(Handle::Axis(axis)) {
                    c[3] *= 0.25;
                }
                stroke(&mut overlays, from, to, width, c);
            }
        }
        return Ok(overlays);
    }
    if state.tool == Tool::Translate {
        // Plane squares first, under the arms: 20 % fill (45 % lit), a 1 px edge.
        for (axis, tint) in tints.iter().enumerate() {
            let handle = Handle::Plane(axis);
            let Some(quad) = handles.plane_quad(axis) else {
                continue;
            };
            let c = faded(*tint, handle);
            let mut fill = c;
            fill[3] *= if lit(handle) { 0.45 } else { 0.2 };
            overlays.push(Overlay::Quad {
                corners: quad,
                color: fill,
            });
            for n in 0..4 {
                stroke(&mut overlays, quad[n], quad[(n + 1) % 4], 0.5, c);
            }
        }
    }
    for (axis, tint) in tints.iter().enumerate() {
        let handle = Handle::Axis(axis);
        let c = faded(*tint, handle);
        let width = if lit(handle) { 1.5 } else { 1. };
        let end = Vec2::from(handles.ends[axis]);
        let dir = handles.screen_dir(axis);
        if dir == Vec2::ZERO || o.distance(end) < 8. {
            continue; // Axes nearly parallel to the view fade out entirely.
        }
        if state.tool == Tool::Scale {
            // An 8 px cube (10 px lit) at the arm's end.
            stroke(
                &mut overlays,
                handles.origin,
                (end - dir * 4.).into(),
                width,
                c,
            );
            solid(
                &mut overlays,
                square(end, if lit(handle) { 5. } else { 4. }),
                c,
            );
        } else {
            // A 12 px solid cone.
            let base = end - dir * 12.;
            let side = Vec2::new(-dir.y, dir.x) * if lit(handle) { 5.5 } else { 4.5 };
            stroke(&mut overlays, handles.origin, base.into(), width, c);
            solid(
                &mut overlays,
                [
                    end.into(),
                    (base + side).into(),
                    (base - side).into(),
                    end.into(),
                ],
                c,
            );
        }
    }
    let centre = if lit(Handle::Centre) {
        hover
    } else {
        color("foreground", 0.75)?
    };
    if state.tool == Tool::Scale {
        // A 10 px square for uniform scale.
        solid(&mut overlays, square(o, 5.), centre);
    } else {
        // A 10 px screen-space disc for free movement in the view plane.
        stroke(&mut overlays, handles.origin, handles.origin, 5., centre);
    }
    Ok(overlays)
}
