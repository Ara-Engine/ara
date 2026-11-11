//! Drop to Surface (End): lowers each selected root straight down until its lowest
//! presented geometry rests on the mesh below, `viewport.surfaceOffset` above it, as one
//! undoable batch. The Scene's presented geometry answers the rays; a missing root, an
//! invalid transform or no support below rejects the whole selection.
use std::collections::BTreeSet;

use ara_doc::prefab::Resolved;
use glam::{Mat4, Vec3};
use serde_json::{Value, json};

use crate::Editor;
use crate::app::viewport::SceneView;
use crate::world::EditWorldView;

/// Whether `id` is `roots`-accepted or sits below one that is.
pub(crate) fn within(entities: &Resolved, id: &str, roots: impl Fn(&str) -> bool) -> bool {
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

/// The gap Drop to Surface leaves between geometry and its support.
/// How far to move a selection whose rendered `vertices` sit around `pivot` so its lowest
/// point along `normal` touches surface `point` at `offset` metres: a surface-snap contact.
pub(crate) fn contact_delta(
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

pub(crate) fn offset(editor: &Editor) -> f32 {
    editor
        .state
        .number("viewport.surfaceOffset", 0.)
        .clamp(-1_000_000., 1_000_000.) as f32
}

/// The `transform.set` commands that drop the selection onto what is below it, and the
/// revision they apply over; `None` when nothing moves. `target_of` maps a presented
/// renderer entity to its scene address (the Scene's channel map).
pub(crate) fn plan<'a>(
    editor: &Editor,
    scene: &SceneView,
    target_of: impl Fn(u64) -> Option<&'a str>,
) -> ara_ui::Result<Option<(Vec<Value>, u64)>> {
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
    let all =
        scene.snap_vertices(|id| target_of(id).is_some_and(|address| selected.contains(address)));
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
                target_of(vertex.entity)
                    .is_some_and(|address| within(view.entities(), address, |root| root == id))
            })
            .map(|vertex| vertex.position)
            .collect();
        let low =
            source.iter().copied().reduce(Vec3::min).ok_or(
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
        let address = target_of(hit.entity)
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
        if target_of(verify.entity) != Some(address)
            || Vec3::from(verify.position).distance(contact) > 0.01
        {
            return Err(
                "Selected support falls outside the target mesh; no transforms were changed".into(),
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
    Ok((!commands.is_empty()).then_some((commands, gesture.base)))
}
