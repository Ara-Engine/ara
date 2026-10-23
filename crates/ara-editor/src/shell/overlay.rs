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
/// Projected axis handles and pivot, shared by drawing and pointer hit testing.
pub struct Handles {
    /// World pivot.
    pub pivot: Vec3,
    /// Image-space pivot.
    pub origin: [f32; 2],
    /// Image-space endpoints for the X, Y and Z handles.
    pub ends: [[f32; 2]; 3],
    /// Handle extent in world metres.
    pub length: f32,
    /// World-space axes, including the active entity's local orientation.
    pub axes: [Vec3; 3],
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
    let worlds = crate::world::world_transforms(view.entities());
    let id = state.selection.first()?;
    let matrix = worlds.get(id)?;
    let pivot = if state.centre {
        state
            .selection
            .iter()
            .filter_map(|s| worlds.get(s))
            .map(|m| m.transform_point3(Vec3::ZERO))
            .sum::<Vec3>()
            / state.selection.len() as f32
    } else {
        matrix.transform_point3(Vec3::ZERO)
    };
    let length = state.camera.distance * 0.12;
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
        tool: state.tool,
        camera: state.camera,
        rect,
    })
}
/// Return the nearest transform handle within a ten-pixel interaction target.
pub fn hit(handles: &Handles, pointer: [f32; 2]) -> Option<usize> {
    let p = Vec2::from(pointer);
    if handles.tool == Tool::Rotate {
        return (0..3)
            .flat_map(|axis| {
                (0..48).filter_map(move |n| {
                    let [a, b] = handles.ring_segment(axis, n)?;
                    let (a, b) = (Vec2::from(a), Vec2::from(b));
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_squared().max(0.001)).clamp(0., 1.);
                    let d = p.distance(a + ab * t);
                    (d < 10.).then_some((axis, d))
                })
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(axis, _)| axis);
    }
    let a = Vec2::from(handles.origin);
    handles
        .ends
        .iter()
        .enumerate()
        .filter_map(|(axis, end)| {
            let b = Vec2::from(*end);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(0.001)).clamp(0.2, 1.);
            let d = p.distance(a + ab * t);
            (d < 10.).then_some((axis, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(axis, _)| axis)
}
/// Create semantic-token grid, silhouette review tints and transform handles.
pub fn build(
    view: &DocumentView,
    state: &EditorState,
    rect: Box2,
    renderer: &ara_render::Renderer,
    changed: &std::collections::BTreeMap<u64, &str>,
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
    let p = state.camera.position();
    let spacing = 10f32.powf((state.camera.distance / 20.).max(0.01).log10().floor());
    overlays.push(Overlay::Grid {
        camera: [p.x, p.y, p.z, state.camera.yaw],
        parameters: [
            state.camera.pitch,
            25f32.to_radians().tan(),
            spacing,
            state.camera.distance * 6.,
        ],
        color: color("muted-foreground", 0.5)?,
        axes: [
            color("destructive-foreground", 0.7)?,
            color("info-foreground", 0.7)?,
        ],
    });
    for (index, (id, _)) in renderer.entities().iter().enumerate() {
        let selected = state
            .selection
            .iter()
            .any(|s| crate::world::render_id(s) == *id);
        if selected || changed.contains_key(id) {
            let token = changed.get(id).copied().unwrap_or("accent");
            overlays.push(Overlay::Outline {
                index: index as u32 + 1,
                color: color(token, 1.)?,
            });
        }
    }
    if let Some(handles) = handles(view, state, rect) {
        for (axis, token) in [
            "destructive-foreground",
            "success-foreground",
            "info-foreground",
        ]
        .iter()
        .enumerate()
        {
            let tint = color(token, 1.)?;
            if state.tool == Tool::Rotate {
                for n in 0..48 {
                    if let Some([from, to]) = handles.ring_segment(axis, n) {
                        overlays.push(Overlay::Line {
                            from,
                            to,
                            width: 1.,
                            color: tint,
                        });
                    }
                }
            } else {
                overlays.push(Overlay::Line {
                    from: handles.origin,
                    to: handles.ends[axis],
                    width: 1.5,
                    color: tint,
                });
                let end = handles.ends[axis];
                for offset in [-3., 3.] {
                    overlays.push(Overlay::Line {
                        from: [end[0] - 3., end[1] + offset],
                        to: [end[0] + 3., end[1] + offset],
                        width: 1.,
                        color: tint,
                    });
                }
            }
        }
    }
    Ok(overlays)
}
