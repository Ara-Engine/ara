//! Vertex and surface snapping for Scene view moves, as Unity's V and surface snap do. While
//! a Translate drag runs with either on (Snapping overlay, or V held for vertex), the gizmo
//! posts its pointer (`Local::snap`) instead of moving by grid steps, and the host, which
//! holds the presented geometry, answers here: the dragged meshes' vertices are captured
//! once, then each move snaps a vertex of theirs onto the target vertex under the pointer, or
//! rests their lowest point on the surface under it (at `viewport.surfaceOffset`).
use std::collections::{BTreeMap, BTreeSet};

use ara_ui::paint::Box2;
use glam::{Mat4, Vec3};

use crate::Editor;
use crate::app::viewport::SceneView;
use crate::surface_drop::{contact_delta, offset, within};
use crate::world::EditWorldView as _;

/// How near the pointer a target vertex must be (view px).
const VERTEX_RADIUS: f32 = 18.0;

/// A snap request from the gizmo: the pointer and the press (view px), the view's rect,
/// the drag's pivot, and whether it snaps vertices (else surfaces).
#[derive(Clone, Copy, Debug)]
pub struct Request {
    /// The pointer now.
    pub at: [f32; 2],
    /// Where the drag was pressed (the source vertex is the one under it).
    pub start: [f32; 2],
    /// The Scene view, view px.
    pub rect: Box2,
    /// The gizmo's pivot.
    pub pivot: Vec3,
    /// Vertex to vertex, else onto the surface.
    pub vertex: bool,
}

/// One drag's captured source: the moved roots and their presented instances, their
/// vertices (for surface contact) and the vertex under the press (for vertex snaps).
pub(super) struct Snap {
    doc: String,
    roots: BTreeMap<String, Mat4>,
    excluded: BTreeSet<u64>,
    vertices: Vec<Vec3>,
    source_vertex: Option<Vec3>,
}

impl Snap {
    /// Captures the drag's source from the presented scene; `None` until it presents the
    /// edited document, or when a moved entity has no world transform.
    pub fn capture(
        editor: &Editor,
        view: &crate::world::DocumentView,
        scene: &SceneView,
        target_of: &dyn Fn(u64) -> Option<String>,
        request: &Request,
    ) -> Option<Self> {
        if !scene.snap_ready(&editor.state.doc) {
            return None;
        }
        let gesture = editor.gesture.as_ref()?;
        let roots: BTreeMap<String, Mat4> = gesture
            .initial
            .keys()
            .filter_map(|id| {
                Some((
                    id.clone(),
                    crate::world::world_transform(view.entities(), id)?,
                ))
            })
            .collect();
        if roots.len() != gesture.initial.len() {
            return None;
        }
        let moved: BTreeSet<String> = view
            .entities()
            .keys()
            .filter(|id| within(view.entities(), id, |root| roots.contains_key(root)))
            .cloned()
            .collect();
        let source = scene.snap_vertices(|id| target_of(id).is_some_and(|a| moved.contains(&a)));
        let excluded: BTreeSet<u64> = source.iter().map(|v| v.entity).collect();
        let vertices = source.iter().map(|v| v.position).collect();
        let source_vertex = scene
            .snap_source_vertex(
                &editor.state.camera,
                request.rect,
                request.start,
                f32::MAX,
                &excluded,
            )
            .map(|v| v.position);
        Some(Self {
            doc: editor.state.doc.clone(),
            roots,
            excluded,
            vertices,
            source_vertex,
        })
    }

    /// Whether this capture still describes the drag (same document, roots unmoved).
    pub fn live(&self, editor: &Editor, view: &crate::world::DocumentView) -> bool {
        self.doc == editor.state.doc
            && self.roots.iter().all(|(id, world)| {
                crate::world::world_transform(view.entities(), id) == Some(*world)
            })
    }

    /// The world move that snaps the source for `request`, and a readout of what it
    /// snapped to; `Err` says why nothing snaps there.
    pub fn delta(
        &self,
        editor: &Editor,
        scene: &SceneView,
        request: &Request,
    ) -> Result<(Vec3, String), String> {
        let camera = &editor.state.camera;
        let gap = offset(editor);
        if request.vertex {
            let source = self
                .source_vertex
                .ok_or("Vertex snap: press on a vertex of the selection")?;
            let target = scene
                .snap_vertex(
                    camera,
                    request.rect,
                    request.at,
                    VERTEX_RADIUS,
                    &self.excluded,
                )
                .ok_or("Vertex snap: no vertex under the pointer")?;
            let delta = target.position + target.normal * gap - source;
            return delta
                .is_finite()
                .then(|| (delta, "Vertex to vertex".to_owned()))
                .ok_or_else(|| "Vertex snap: invalid contact".to_owned());
        }
        let hit = scene
            .surface_hit_excluding(camera, request.rect, request.at, &self.excluded)
            .ok_or("Surface snap: no surface under the pointer")?;
        let mut normal = Vec3::from(hit.normal);
        // The face toward the camera.
        let size = [
            request.rect.w.max(1.0) as u32,
            request.rect.h.max(1.0) as u32,
        ];
        let ndc = [
            request.at[0] / request.rect.w * 2.0 - 1.0,
            1.0 - request.at[1] / request.rect.h * 2.0,
        ];
        if let Some((_, direction)) = camera.ray(size, ndc)
            && normal.dot(direction) > 0.0
        {
            normal = -normal;
        }
        contact_delta(
            &self.vertices,
            request.pivot,
            Vec3::from(hit.position),
            normal,
            gap,
        )
        .map(|delta| (delta, "On the surface".to_owned()))
        .ok_or_else(|| "Surface snap: the selection has no rendered geometry".to_owned())
    }
}
