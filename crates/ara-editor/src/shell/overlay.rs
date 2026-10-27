//! Grid/handle projection and picking use the actual renderer camera and ID channel.
mod gizmo;
use crate::{
    gpu::Overlay,
    state::{Camera, EditorState, Tool},
    world::{DocumentView, EditWorldView},
};
use ara_ui::paint::Box2;
use gizmo::draw_handles;
use glam::{Vec2, Vec3};
/// Resource handle of the viewport image in the native UI display list.
pub fn viewport_image() -> u32 {
    ara_ui::source::stable_id("editor-viewport") as u32 & 0x7fff_ffff
}
/// Resource handle of the renderer's entity-ID image.
pub const ENTITY_IMAGE: u32 = 0x7fff_0001;
/// Resource handle of the outline group table (`NativeUi::outline_groups`).
pub const OUTLINE_GROUPS: u32 = 0x7fff_0002;

/// Selection, review and hover outlines by entity, not by renderer channel: an entity
/// drawn as several instances (a terrain's chunks, a model's parts) outlines as one
/// shape, and a terrain chunk hovers and outlines as its whole terrain. Holds the
/// channel map and the group table for the outline shader; both rebuild only when the
/// scene, the renderer's entities, the selection or the review tints change.
#[derive(Default)]
pub struct Outlines {
    /// Renderer channel → scene entity (also what Scene clicks select).
    pub channels: crate::world::channels::Channels,
    key: u64,
    /// Outline colour groups: group `n + 1` is `groups[n]` (token, and whether any
    /// channel is in it: only those get an outline pass).
    groups: Vec<(String, bool)>,
    upload: Option<ara_ui_render::gpu::Image>,
}
impl Outlines {
    /// Refresh the channel map and group table; the outline overlays, one per colour group.
    fn refresh(
        &mut self,
        view: &DocumentView,
        revision: u64,
        state: &EditorState,
        renderer: &ara_render::Renderer,
        changed: &std::collections::BTreeMap<u64, &str>,
        selection: bool,
    ) {
        let entities = renderer.entities();
        self.channels.update(view.entities(), revision, entities);
        let key = state
            .selection
            .iter()
            .map(|s| crate::world::render_id(s))
            .chain(
                changed
                    .iter()
                    .flat_map(|(id, token)| [*id, crate::world::render_id(token)]),
            )
            .chain([u64::from(selection), self.channels.key()])
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, id| {
                (hash ^ id).wrapping_mul(0x0000_0100_0000_01b3)
            });
        if key == self.key {
            return;
        }
        self.key = key;
        let channels = &self.channels;
        let selected: Vec<bool> = channels
            .addresses
            .iter()
            .map(|a| selection && state.selection.contains(a))
            .collect();
        let mut groups = vec![("accent".to_owned(), false)];
        // Texel 0 is the background; channel `n` is texel `n + 1`, as the renderer writes.
        let width = 4096usize;
        let height = (entities.len() + 1).div_ceil(width).max(1);
        let mut rgba = vec![0u8; width * height * 4];
        for (n, (id, _)) in entities.iter().enumerate() {
            let owner = channels.owners[n];
            let colour = if owner.is_some_and(|o| channels.within(o, &selected)) {
                1
            } else if let Some(token) = changed.get(id).or_else(|| {
                owner.and_then(|o| {
                    changed.get(&crate::world::render_id(&channels.addresses[o as usize]))
                })
            }) {
                match groups.iter().position(|(g, _)| g == token) {
                    Some(at) => at + 1,
                    None if groups.len() < 127 => {
                        groups.push(((*token).to_owned(), false));
                        groups.len()
                    }
                    None => 0,
                }
            } else {
                0
            };
            if colour > 0 {
                groups[colour - 1].1 = true;
            }
            // Hover groups are 16-bit: a scene past 65 535 entities hovers single channels.
            let hover = channels.targets[n]
                .map(|t| t as usize + 1)
                .filter(|h| *h <= 0xffff)
                .unwrap_or(0);
            let texel = (n + 1) * 4;
            rgba[texel] = (hover & 0xff) as u8;
            rgba[texel + 1] = (hover >> 8) as u8;
            // Blue: the colour group, and bit 7 for ground (terrain).
            rgba[texel + 2] = colour as u8 | if channels.ground[n] { 0x80 } else { 0 };
            rgba[texel + 3] = 255;
        }
        self.groups = groups;
        self.upload = Some(ara_ui_render::gpu::Image {
            width: width as u32,
            height: height as u32,
            rgba,
        });
    }
    /// Upload a rebuilt group table and point the outline shader at it.
    pub fn upload(
        &mut self,
        ui: &mut ara_ui_render::gpu::Renderer,
        gpu: &mut crate::gpu::NativeUi,
    ) -> ara_ui::Result<()> {
        if let Some(table) = self.upload.take() {
            ui.image(OUTLINE_GROUPS, table)?;
        }
        gpu.outline_groups(Some(OUTLINE_GROUPS));
        Ok(())
    }
}
/// Project a world point into a logical UI viewport, rejecting points behind the camera.
pub fn project(camera: &Camera, rect: Box2, point: Vec3) -> Option<[f32; 2]> {
    let orthographic = camera.projection == crate::state::CameraProjection::Orthographic;
    let camera = camera
        .spec()
        .resolve([rect.w.max(1.) as u32, rect.h.max(1.) as u32])
        .ok()?;
    let clip = camera.projection * camera.view * point.extend(1.);
    if clip.w <= 0.01 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() || (orthographic && !(0. ..=1.).contains(&ndc.z)) {
        return None;
    }
    Some([
        rect.x + (ndc.x + 1.) * 0.5 * rect.w,
        rect.y + (1. - ndc.y) * 0.5 * rect.h,
    ])
}
/// Intersect a pointer ray with the ground plane for drag-to-place.
pub fn ground_point(camera: &Camera, rect: Box2, point: [f32; 2]) -> Option<Vec3> {
    if !rect.w.is_finite() || !rect.h.is_finite() || rect.w <= 0. || rect.h <= 0. {
        return None;
    }
    let x = (point[0] - rect.x) / rect.w * 2. - 1.;
    let y = 1. - (point[1] - rect.y) / rect.h * 2.;
    let (origin, direction) = camera.ray([rect.w.max(1.) as u32, rect.h.max(1.) as u32], [x, y])?;
    if direction.y.abs() < 0.00001 {
        return None;
    }
    let t = -origin.y / direction.y;
    (t.is_finite() && t > 0.).then_some(origin + direction * t)
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
    /// A rotation ring in the universal tool.
    RotateAxis(usize),
    /// A scale cube in the universal tool.
    ScaleAxis(usize),
}
impl Handle {
    /// Operation selected by this handle; ordinary handles use the active tool.
    pub fn tool(self, active: Tool) -> Tool {
        match self {
            Self::RotateAxis(_) => Tool::Rotate,
            Self::ScaleAxis(_) => Tool::Scale,
            _ if active == Tool::Universal => Tool::Translate,
            _ => active,
        }
    }
    /// Shape used by the single-operation drag and drawing code.
    pub fn base(self) -> Self {
        match self {
            Self::RotateAxis(a) | Self::ScaleAxis(a) => Self::Axis(a),
            _ => self,
        }
    }
}
/// Half the minimum hit target: every handle is grabbable within 16 px or more.
pub const HIT: f32 = 9.;
/// Gizmo arm length on screen at 100% gizmo size (Appendix C).
pub const ARM: f32 = 88.;
/// Projected axis handles and pivot, shared by drawing and pointer hit testing.
#[derive(Clone, Copy)]
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
    /// Geometry of one operation in the universal tool. Scale cubes sit inside
    /// the move arrows; rotation rings surround them without sharing hit targets.
    pub fn for_tool(&self, tool: Tool) -> Self {
        if self.tool != Tool::Universal {
            return *self;
        }
        let factor = match tool {
            Tool::Rotate => 0.85,
            Tool::Scale => 0.62,
            _ => 1.,
        };
        let length = self.length * factor;
        let ends = self.axes.map(|axis| {
            project(&self.camera, self.rect, self.pivot + axis * length).unwrap_or(self.origin)
        });
        Self {
            tool,
            length,
            ends,
            ..*self
        }
    }
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
    if !rect.w.is_finite()
        || !rect.h.is_finite()
        || rect.w <= 0.
        || rect.h <= 0.
        || !pivot.is_finite()
        || !normal.is_finite()
    {
        return None;
    }
    let (origin, direction) = camera.ray(
        [rect.w.max(1.) as u32, rect.h.max(1.) as u32],
        [
            (pointer[0] - rect.x) / rect.w * 2. - 1.,
            1. - (pointer[1] - rect.y) / rect.h * 2.,
        ],
    )?;
    let denominator = direction.dot(normal);
    if denominator.abs() < 0.0001 {
        return None;
    }
    let t = (pivot - origin).dot(normal) / denominator;
    (t.is_finite() && t > 0.).then_some(origin + direction * t)
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
    let per_pixel = state.camera.world_per_pixel(pivot, rect.h)?;
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
    if handles.tool == Tool::Universal {
        let scale = handles.for_tool(Tool::Scale);
        let p = Vec2::from(pointer);
        for axis in 0..3 {
            let end = Vec2::from(scale.ends[axis]);
            if end.distance(Vec2::from(scale.origin)) >= 8. && p.distance(end) < HIT {
                return Some(Handle::ScaleAxis(axis));
            }
        }
        let move_handles = handles.for_tool(Tool::Translate);
        let movement = hit(&move_handles, pointer);
        if matches!(movement, Some(Handle::Centre | Handle::Plane(_))) {
            return movement;
        }
        let rotation = hit(&handles.for_tool(Tool::Rotate), pointer);
        return rotation
            .map(|h| match h {
                Handle::Axis(a) => Handle::RotateAxis(a),
                _ => h,
            })
            .or(movement);
    }
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
/// Create semantic-token grid, silhouette review tints and transform handles. Outlines
/// go by entity (`outlines`, refreshed here; upload its table with `Outlines::upload`).
#[allow(clippy::too_many_arguments)]
pub fn build(
    view: &DocumentView,
    state: &EditorState,
    rect: Box2,
    renderer: &ara_render::Renderer,
    changed: &std::collections::BTreeMap<u64, &str>,
    gizmo: GizmoState,
    outlines: &mut Outlines,
    revision: u64,
    theme: &ara_ui::style::Sheet,
) -> ara_ui::Result<Vec<Overlay>> {
    let color = |token: &str, alpha: f32| -> ara_ui::Result<[f32; 4]> {
        let color = theme
            .color_token(&format!("--{token}"))
            .ok_or("theme token missing")?;
        Ok([
            color.0 as f32 / 255.,
            color.1 as f32 / 255.,
            color.2 as f32 / 255.,
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
    // One outline per colour group: the selection (with everything under it) and each
    // review tint, each as one shape however many channels it spans.
    outlines.refresh(
        view,
        revision,
        state,
        renderer,
        changed,
        show("viewport.outline"),
    );
    for (n, (token, used)) in outlines.groups.iter().enumerate() {
        if *used {
            overlays.push(Overlay::Outline {
                index: n as u32 + 1,
                color: color(token, 1.)?,
            });
        }
    }
    let Some(handles) = handles(view, state, rect) else {
        return Ok(overlays);
    };
    if state.tool == Tool::Universal {
        for tool in [Tool::Rotate, Tool::Translate, Tool::Scale] {
            let belongs = |handle: &Handle| handle.tool(Tool::Universal) == tool;
            let other_active = gizmo.active.is_some_and(|h| !belongs(&h));
            let operation_gizmo = GizmoState {
                hover: gizmo.hover.filter(belongs).map(Handle::base),
                active: gizmo.active.filter(belongs).map(Handle::base),
            };
            let tint = |name: &str, alpha: f32| {
                color(name, if other_active { alpha * 0.15 } else { alpha })
            };
            draw_handles(
                &mut overlays,
                &handles.for_tool(tool),
                operation_gizmo,
                &tint,
                tool != Tool::Scale,
            )?;
        }
    } else {
        draw_handles(&mut overlays, &handles, gizmo, &color, true)?;
    }
    Ok(overlays)
}
