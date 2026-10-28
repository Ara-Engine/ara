//! Pure hierarchy planning; callers commit only a completely validated batch.
use ara_doc::prefab::Resolved;
use glam::{EulerRot, Mat4};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Selected top-level roots in tree display order; descendants travel with their root.
pub(in crate::shell) fn selection_roots<'a>(
    resolved: &Resolved,
    selected: &BTreeSet<&'a str>,
) -> Result<Vec<&'a str>, String> {
    // Ancestor order keys establish display order without sorting the entire scene.
    let mut roots = Vec::new();
    for &id in selected {
        let mut ancestor = Some(id);
        let mut seen = BTreeSet::new();
        let mut path = Vec::new();
        let mut covered = false;
        while let Some(current) = ancestor {
            if !seen.insert(current) {
                return Err("The hierarchy contains a parent cycle".into());
            }
            if current != id && selected.contains(current) {
                covered = true;
                break;
            }
            let entity = resolved
                .get(current)
                .ok_or("A selected entity has a missing parent")?;
            path.push((&entity.order, current));
            ancestor = entity.parent.as_deref();
        }
        if !covered {
            if id.contains('/') {
                return Err("Move the prefab instance instead of an entity inside it".into());
            }
            path.reverse();
            roots.push((path, id));
        }
    }
    roots.sort_by(|(a, _), (b, _)| a.cmp(b));
    let roots: Vec<_> = roots.into_iter().map(|(_, id)| id).collect();
    if roots.is_empty() {
        return Err("No movable entities are selected".into());
    }
    Ok(roots)
}

/// Lower an exact world-to-parent pose, rejecting singular or sheared local transforms.
pub(in crate::shell) fn preserved_transform(
    resolved: &Resolved,
    source: &str,
    parent_world: Mat4,
) -> Result<Value, String> {
    if !parent_world.is_finite() || parent_world.determinant().abs() < 1e-20 {
        return Err(
            "The destination has a singular transform; restore a nonzero scale before reparenting"
                .into(),
        );
    }
    let world =
        crate::world::world_transform(resolved, source).ok_or("The dragged entity was deleted")?;
    let local = parent_world.inverse() * world;
    if !local.is_finite() || local.determinant().abs() < 1e-20 {
        return Err(
            "The dragged transform cannot be preserved; restore a nonzero scale before reparenting"
                .into(),
        );
    }
    let (scale, rotation, position) = local.to_scale_rotation_translation();
    let rebuilt = Mat4::from_scale_rotation_translation(scale, rotation, position);
    let error = local
        .to_cols_array()
        .into_iter()
        .zip(rebuilt.to_cols_array())
        .map(|(a, b)| (a - b).abs() / a.abs().max(1.))
        .fold(0_f32, f32::max);
    if !scale.is_finite() || !rotation.is_finite() || !position.is_finite() || error > 0.001 {
        return Err("This parent would shear the object; use a parent with uniform scale to preserve its world pose".into());
    }
    let (x, y, z) = rotation.to_euler(EulerRot::XYZ);
    Ok(
        json!({"cmd":"transform.set","args":{"entities":[source],"position":position.to_array(),"rotation":[x.to_degrees(),y.to_degrees(),z.to_degrees()],"scale":scale.to_array()}}),
    )
}

fn ancestors<'a>(resolved: &'a Resolved, id: &'a str) -> Result<Vec<&'a str>, String> {
    let mut path = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = Some(id);
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err("The hierarchy contains a parent cycle".into());
        }
        let entity = resolved
            .get(id)
            .ok_or("A selected entity has a missing parent")?;
        path.push(id);
        current = entity.parent.as_deref();
    }
    Ok(path)
}

/// Create an identity container at the nearest shared ancestor, at the first source's slot.
/// Existing sibling locals remain exact; cross-parent roots use the drag pose adapter.
pub(super) fn group_commands(
    resolved: &Resolved,
    selection: &[String],
    locked: &BTreeSet<String>,
) -> Result<Vec<Value>, String> {
    let selected: BTreeSet<_> = selection.iter().map(String::as_str).collect();
    if selected.iter().any(|id| !resolved.contains_key(*id)) {
        return Err("A selected entity was deleted; select the surviving entities again".into());
    }
    if selected.iter().any(|id| locked.contains(*id)) {
        return Err("Unlock the selected entities before grouping them".into());
    }
    let roots = selection_roots(resolved, &selected)?;
    let first_path = ancestors(resolved, roots[0])?;
    let mut common: Vec<_> = first_path.iter().skip(1).copied().collect();
    for root in roots.iter().skip(1) {
        let path = ancestors(resolved, root)?;
        common.retain(|candidate| path.iter().skip(1).any(|id| id == candidate));
    }
    let parent = common.first().copied();
    if parent.is_some_and(|id| locked.contains(id)) {
        return Err("Unlock the destination parent before grouping".into());
    }
    let branch = first_path
        .iter()
        .find(|id| resolved[**id].parent.as_deref() == parent)
        .copied()
        .ok_or("The selected hierarchy branch was deleted")?;
    let branch_entity = &resolved[branch];
    let index = resolved
        .iter()
        .filter(|(id, entity)| {
            entity.parent.as_deref() == parent
                && (&entity.order, id.as_str()) < (&branch_entity.order, branch)
        })
        .count();
    let index = u32::try_from(index).map_err(|_| "The hierarchy has too many siblings")?;
    let parent_world = parent
        .map_or(Some(Mat4::IDENTITY), |id| {
            crate::world::world_transform(resolved, id)
        })
        .ok_or("The destination parent was deleted")?;
    let mut calls = Vec::with_capacity(1 + roots.len() * 2);
    calls.push(json!({"cmd":"entity.create","args":{"id":"$group","name":"Group","parent":parent,"index":index,"components":{"Transform":{}}}}));
    for (index, source) in roots.into_iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| "Too many selected entities to group")?;
        calls.push(json!({"cmd":"entity.reparent","args":{"entity":source,"parent":"$group","index":index}}));
        if resolved[source].parent.as_deref() != parent {
            calls.push(preserved_transform(resolved, source, parent_world)?);
        }
    }
    Ok(calls)
}

/// Full, deduplicated target set. Presentation flags also support inner prefab entities.
pub(super) fn view_targets(
    resolved: &Resolved,
    selected: &[String],
    descendants: bool,
) -> Result<Vec<String>, String> {
    if selected.is_empty() {
        return Err("Select an entity".into());
    }
    for id in selected {
        ancestors(resolved, id)?;
    }
    let mut found: BTreeSet<&str> = selected.iter().map(String::as_str).collect();
    if descendants {
        let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (id, entity) in resolved {
            if let Some(parent) = entity.parent.as_deref() {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut pending: Vec<_> = found.iter().copied().collect();
        while let Some(id) = pending.pop() {
            for child in children.get(id).into_iter().flatten() {
                if found.insert(child) {
                    pending.push(child);
                }
            }
        }
    }
    Ok(found.into_iter().map(str::to_owned).collect())
}
