//! Exact contact previews are transient; release and End use ordinary transform commands.
use super::*;
use ara_doc::prefab::Resolved;
use ara_ui::paint::Box2;
use glam::Mat4;
use std::collections::BTreeMap;

const VERTEX_RADIUS: f32 = 18.;

pub(super) struct Drag {
    doc: String,
    surface: Option<String>,
    roots: BTreeMap<String, Mat4>,
    source_addresses: BTreeSet<String>,
    excluded: BTreeSet<u64>,
    vertices: Vec<Vec3>,
    source_vertex: Option<Vec3>,
    target: Option<(String, Mat4)>,
}

fn within(entities: &Resolved, id: &str, roots: impl Fn(&str) -> bool) -> bool {
    let mut at = Some(id);
    for _ in 0..=entities.len() {
        let Some(id) = at else {
            return false;
        };
        if roots(id) {
            return true;
        }
        at = entities.get(id).and_then(|entity| entity.parent.as_deref());
    }
    false
}

fn offset(editor: &Editor) -> f32 {
    editor
        .state
        .number("viewport.surfaceOffset", 0.)
        .clamp(-1_000_000., 1_000_000.) as f32
}

fn setting(editor: &Editor, key: &str) -> bool {
    editor
        .state
        .settings
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn contact_delta(
    vertices: &[Vec3],
    pivot: Vec3,
    point: Vec3,
    normal: Vec3,
    offset: f32,
) -> Option<Vec3> {
    if vertices.is_empty()
        || !pivot.is_finite()
        || !point.is_finite()
        || !normal.is_finite()
        || !offset.is_finite()
        || normal.length_squared() < 0.5
    {
        return None;
    }
    let support = vertices
        .iter()
        .map(|vertex| (*vertex - pivot).dot(normal))
        .min_by(f32::total_cmp)?;
    let delta = point - pivot + normal * (offset - support);
    delta.is_finite().then_some(delta)
}

impl Drag {
    pub fn capture(
        editor: &Editor,
        shell: &Shell,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
        start: [f32; 2],
    ) -> Option<Self> {
        if !scene.snap_ready(&editor.state.doc) {
            return None;
        }
        let gesture = editor.gesture.as_ref()?;
        let roots: BTreeMap<_, _> = gesture
            .initial
            .keys()
            .filter_map(|id| {
                Some((
                    id.to_owned(),
                    crate::world::world_transform(view.entities(), id)?,
                ))
            })
            .collect();
        if roots.len() != gesture.initial.len() {
            return None;
        }
        let source_addresses: BTreeSet<_> = view
            .entities()
            .keys()
            .filter(|id| within(view.entities(), id, |root| roots.contains_key(root)))
            .cloned()
            .collect();
        let source = scene.snap_vertices(|id| {
            shell
                .outlines
                .channels
                .target_of(id)
                .is_some_and(|address| source_addresses.contains(address))
        });
        let excluded: BTreeSet<_> = source.iter().map(|vertex| vertex.entity).collect();
        let vertices: Vec<_> = source.iter().map(|vertex| vertex.position).collect();
        let source_vertex = shell
            .viewport()
            .and_then(|rect| {
                scene.snap_source_vertex(&editor.state.camera, rect, start, 1_000_000., &excluded)
            })
            .map(|vertex| vertex.position);
        Some(Self {
            doc: editor.state.doc.clone(),
            surface: shell.current().map(str::to_owned),
            roots,
            source_addresses,
            excluded,
            vertices,
            source_vertex,
            target: None,
        })
    }

    fn live(&self, editor: &Editor, shell: &Shell, view: &crate::world::DocumentView) -> bool {
        self.doc == editor.state.doc
            && self.surface.as_deref() == shell.current()
            && self.roots.iter().all(|(id, world)| {
                crate::world::world_transform(view.entities(), id) == Some(*world)
            })
            && self
                .source_addresses
                .iter()
                .all(|id| view.entities().contains_key(id))
            && self.target.as_ref().is_none_or(|(id, world)| {
                crate::world::world_transform(view.entities(), id) == Some(*world)
            })
    }

    fn remember(&mut self, entity: u64, shell: &Shell, view: &crate::world::DocumentView) -> bool {
        let Some(address) = shell.outlines.channels.target_of(entity) else {
            return false;
        };
        if self.source_addresses.contains(address) {
            return false;
        }
        let Some(world) = crate::world::world_transform(view.entities(), address) else {
            return false;
        };
        match &mut self.target {
            Some((id, matrix)) if id == address => *matrix = world,
            _ => self.target = Some((address.to_owned(), world)),
        }
        true
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "snap input borrows the current host, authority and presented geometry independently"
    )]
    fn preview(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
        position: [f32; 2],
        vertex: bool,
        pivot: Vec3,
    ) -> (Vec3, String) {
        self.target = None;
        let Some(rect) = shell.viewport() else {
            return (Vec3::ZERO, "Snap · Scene view unavailable".into());
        };
        let gap = offset(editor);
        if vertex {
            let Some(source) = self.source_vertex else {
                return (Vec3::ZERO, "Vertex snap · no rendered source vertex".into());
            };
            let Some(target) = scene.snap_vertex(
                &editor.state.camera,
                rect,
                position,
                VERTEX_RADIUS,
                &self.excluded,
            ) else {
                return (Vec3::ZERO, "Vertex snap · no visible target vertex".into());
            };
            if (gap != 0. && target.normal.length_squared() < 0.5)
                || !self.remember(target.entity, shell, view)
            {
                return (
                    Vec3::ZERO,
                    "Vertex snap · target is no longer available".into(),
                );
            }
            let delta = target.position + target.normal * gap - source;
            if !delta.is_finite() {
                return (Vec3::ZERO, "Vertex snap · invalid contact".into());
            }
            return (
                delta,
                format!("Vertex → vertex · offset {gap:+.3} m · grid bypassed"),
            );
        }
        let Some(hit) =
            scene.surface_hit_excluding(&editor.state.camera, rect, position, &self.excluded)
        else {
            return (Vec3::ZERO, "Surface snap · no target surface".into());
        };
        let mut normal = Vec3::from(hit.normal);
        if let Some((_, direction)) = editor.state.camera.ray(
            [rect.w.max(1.) as u32, rect.h.max(1.) as u32],
            [
                (position[0] - rect.x) / rect.w * 2. - 1.,
                1. - (position[1] - rect.y) / rect.h * 2.,
            ],
        ) && normal.dot(direction) > 0.
        {
            normal = -normal;
        }
        if !self.remember(hit.entity, shell, view) {
            return (
                Vec3::ZERO,
                "Surface snap · target is no longer available".into(),
            );
        }
        let Some(delta) =
            contact_delta(&self.vertices, pivot, Vec3::from(hit.position), normal, gap)
        else {
            return (
                Vec3::ZERO,
                "Surface snap · no finite rendered support".into(),
            );
        };
        (
            delta,
            format!("Surface contact · offset {gap:+.3} m · orientation retained · grid bypassed"),
        )
    }
}

impl Controller {
    pub(super) fn vertex_snap(&self, editor: &Editor, shell: &Shell) -> bool {
        let field = shell
            .input
            .focused()
            .and_then(|id| shell.ui.by_id(id))
            .is_some_and(|index| {
                matches!(
                    shell.ui.nodes[index].element.tag.as_str(),
                    "field" | "input"
                )
            });
        !field
            && !shell.modal_active()
            && !shell.choice_open()
            && !shell.modifiers[1]
            && !self.keys.contains(&224)
            && !self.keys.contains(&228)
            && !self.keys.contains(&227)
            && !self.keys.contains(&231)
            && (self.keys.contains(&25) || setting(editor, "viewport.vertexSnap"))
    }

    pub(super) fn capture_snap(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
    ) {
        if editor
            .gesture
            .as_ref()
            .is_none_or(|gesture| gesture.tool != crate::state::Tool::Translate)
        {
            return;
        }
        if let Some(drag) = &mut self.gizmo {
            drag.snap = Drag::capture(editor, shell, view, scene, drag.start);
        }
    }

    pub(super) fn begin_vertex(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        if !self.vertex_snap(editor, shell) {
            return Ok(false);
        }
        let Some(rect) = shell.viewport() else {
            return Ok(false);
        };
        if editor.state.selection.is_empty() {
            shell.show_readout(Some((
                "Vertex snap · select a source first".into(),
                shell.pointer,
            )));
            return Ok(true);
        }
        editor.begin_gesture_tool(crate::state::Tool::Translate)?;
        let Some(mut source) = Drag::capture(editor, shell, view, scene, shell.pointer) else {
            editor.finish_gesture(true)?;
            return Ok(true);
        };
        let Some(vertex) = source.source_vertex else {
            editor.finish_gesture(true)?;
            shell.show_readout(Some((
                "Vertex snap · no visible source vertex".into(),
                shell.pointer,
            )));
            return Ok(true);
        };
        let Some(projected) = overlay::project(&editor.state.camera, rect, vertex) else {
            editor.finish_gesture(true)?;
            return Ok(true);
        };
        if Vec2::from(projected).distance(Vec2::from(shell.pointer)) > VERTEX_RADIUS {
            editor.finish_gesture(true)?;
            shell.show_readout(Some((
                "Vertex snap · point at a selected mesh vertex".into(),
                shell.pointer,
            )));
            return Ok(true);
        }
        source.source_vertex = Some(vertex);
        let forward =
            (editor.state.camera.target - editor.state.camera.position()).normalize_or_zero();
        self.gizmo = Some(GizmoDrag {
            handle: overlay::Handle::Centre,
            start: shell.pointer,
            vector: Vec2::ONE,
            length: 1.,
            rotation: None,
            plane: Some((forward, vertex)),
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            pivot: vertex,
            typed: String::new(),
            duplicate: false,
            snap: Some(source),
        });
        Ok(true)
    }

    pub(super) fn drive_snap(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
    ) -> Option<String> {
        let vertex = self.vertex_snap(editor, shell);
        let active = vertex || setting(editor, "viewport.surfaceSnap");
        let drag = self.gizmo.as_ref()?;
        if !active
            || !drag.typed.is_empty()
            || editor
                .gesture
                .as_ref()
                .is_none_or(|gesture| gesture.tool != crate::state::Tool::Translate)
        {
            if let Some(source) = self.gizmo.as_mut().and_then(|drag| drag.snap.as_mut()) {
                source.target = None;
            }
            return self.drive_gizmo(editor, shell.pointer, shell.viewport());
        }
        if drag
            .snap
            .as_ref()
            .is_none_or(|source| !source.live(editor, shell, view))
        {
            self.gizmo = None;
            if let Err(error) = editor.finish_gesture(true) {
                editor.report_failure(&error);
            }
            return Some("Snap cancelled · source, target or Scene host changed".into());
        }
        let drag = self.gizmo.as_mut()?;
        let (delta, text) = drag.snap.as_mut()?.preview(
            editor,
            shell,
            view,
            scene,
            shell.pointer,
            vertex,
            drag.pivot,
        );
        if !editor.gesture.as_mut()?.translate_world(delta) {
            return Some("Snap · invalid contact; preview unchanged".into());
        }
        Some(text)
    }

    pub(super) fn finish_snap(&mut self, editor: &mut Editor, shell: &Shell) -> ara_ui::Result<()> {
        let live = editor.view();
        let valid = live.as_ref().is_ok_and(|view| {
            self.gizmo
                .as_ref()
                .and_then(|drag| drag.snap.as_ref())
                .is_none_or(|source| source.live(editor, shell, view))
        });
        if !valid {
            self.gizmo = None;
            editor.finish_gesture(true)?;
            if let Err(error) = live {
                editor.report_failure(&*error);
            }
            return Ok(());
        }
        if let Err(error) = self.finish_gizmo(editor) {
            editor.report_failure(&*error);
        }
        Ok(())
    }

    /// End lowers every selected rendered root onto a real support surface in one batch.
    /// A missing root, invalid transform or unsupported target rejects the whole selection.
    pub(in crate::app) fn drop_to_surface(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        scene: &viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        if editor.gesture.is_some() || self.gizmo.is_some() {
            return Err("Finish the active transform before Drop to Surface".into());
        }
        if shell.modal_active() || shell.choice_open() {
            return Err("Close the active dialog before Drop to Surface".into());
        }
        if shell.viewport().is_none() {
            return Err("Drop to Surface requires the hosting Scene view".into());
        }
        if !scene.snap_ready(&editor.state.doc) {
            return Err("Drop to Surface is waiting for this scene's geometry; the previous scene cannot be used".into());
        }
        let view = editor.view()?;
        let gesture = crate::world::gesture::Gesture::begin_tool(
            view.entities(),
            &editor.state,
            editor.host.store().revision(),
            crate::state::Tool::Translate,
        )?;
        if editor.state.selection.iter().any(|id| {
            !within(view.entities(), id, |root| {
                gesture.initial.contains_key(root)
            })
        }) {
            return Err("Every selected root must support a transform; no roots were moved".into());
        }
        let selected: BTreeSet<_> = view
            .entities()
            .keys()
            .filter(|id| {
                within(view.entities(), id, |root| {
                    gesture.initial.contains_key(root)
                })
            })
            .map(String::as_str)
            .collect();
        let all = scene.snap_vertices(|id| {
            shell
                .outlines
                .channels
                .target_of(id)
                .is_some_and(|address| selected.contains(address))
        });
        let excluded: BTreeSet<_> = all.iter().map(|vertex| vertex.entity).collect();
        let gap = offset(editor);
        if !gap.is_finite() {
            return Err("Drop to Surface offset must be finite".into());
        }
        let mut commands = Vec::new();
        for id in gesture.initial.keys() {
            let source: Vec<_> = all
                .iter()
                .filter(|vertex| {
                    shell
                        .outlines
                        .channels
                        .target_of(vertex.entity)
                        .is_some_and(|address| within(view.entities(), address, |root| root == id))
                })
                .map(|vertex| vertex.position)
                .collect();
            let low = source.iter().copied().reduce(Vec3::min).ok_or(
                "Every selected root must have presented mesh geometry for Drop to Surface",
            )?;
            let high = source
                .iter()
                .copied()
                .reduce(Vec3::max)
                .ok_or("Source geometry is unavailable")?;
            let origin = Vec3::new(
                (low.x + high.x) * 0.5,
                high.y + 0.001,
                (low.z + high.z) * 0.5,
            );
            let hit = scene
                .ray_hit_excluding(origin, -Vec3::Y, &excluded)
                .ok_or("Drop to Surface found no supporting mesh below a selected root")?;
            let address = shell
                .outlines
                .channels
                .target_of(hit.entity)
                .filter(|address| view.entities().contains_key(*address))
                .ok_or("Drop target was removed or is unsupported")?;
            let normal = if hit.normal[1] < 0. {
                -Vec3::from(hit.normal)
            } else {
                Vec3::from(hit.normal)
            };
            if normal.y < 0.0001 {
                return Err("Drop to Surface cannot rest on a vertical surface".into());
            }
            let point = Vec3::from(hit.position);
            let support = source
                .iter()
                .copied()
                .min_by(|a, b| a.dot(normal).total_cmp(&b.dot(normal)))
                .ok_or("Source support is unavailable")?;
            let dy = (gap - (support - point).dot(normal)) / normal.y;
            let delta = Vec3::Y * dy;
            let contact = support + delta - normal * gap;
            let verify = scene
                .ray_hit_excluding(contact + Vec3::Y * 0.002, -Vec3::Y, &excluded)
                .ok_or("Selected geometry does not contact the finite target surface")?;
            if shell.outlines.channels.target_of(verify.entity) != Some(address)
                || Vec3::from(verify.position).distance(contact) > 0.01
            {
                return Err(
                    "Selected support falls outside the target mesh; no transforms were changed"
                        .into(),
                );
            }
            let entity = view.entities().get(id).ok_or("Selected root was deleted")?;
            let parent = entity
                .parent
                .as_deref()
                .map(|parent| crate::world::world_transform(view.entities(), parent))
                .unwrap_or(Some(Mat4::IDENTITY))
                .ok_or("Selected parent was deleted")?;
            if !parent.is_finite() || parent.determinant().abs() < 1e-20 || !delta.is_finite() {
                return Err("Drop to Surface encountered an invalid transform".into());
            }
            let start = crate::world::local_transform(&entity.record)
                .w_axis
                .truncate();
            let position = start + parent.inverse().transform_vector3(delta);
            if !position.is_finite() {
                return Err("Drop to Surface position is not finite".into());
            }
            if position.distance_squared(start) > 1e-12 {
                commands.push(json!({"cmd":"transform.set","args":{"entities":[id],"position":position.to_array()}}));
            }
        }
        if commands.is_empty() {
            return Ok(false);
        }
        editor.apply(commands, "Drop selection to surface", Some(gesture.base))?;
        shell.rebuild(editor)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
