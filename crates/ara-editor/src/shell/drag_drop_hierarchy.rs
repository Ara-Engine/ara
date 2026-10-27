//! Validate hierarchy gestures and lower selection roots to one ordered command batch.
use ara_doc::prefab::Resolved;
use ara_ui::drag_drop::Placement;
use glam::{EulerRot, Mat4};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn siblings<'a>(resolved: &'a Resolved, parent: Option<&str>) -> Vec<&'a String> {
    let mut siblings: Vec<_> = resolved
        .iter()
        .filter(|(_, entity)| entity.parent.as_deref() == parent)
        .map(|(id, entity)| (id, &entity.order))
        .collect();
    siblings.sort_by(|(a, order_a), (b, order_b)| order_a.cmp(order_b).then(a.cmp(b)));
    siblings.into_iter().map(|(id, _)| id).collect()
}

/// Parent and sibling insertion slot before any source is removed from that sibling list.
pub(super) fn destination(
    resolved: &Resolved,
    target: Option<&str>,
    placement: Placement,
) -> Result<(Option<String>, Option<usize>), String> {
    let Some(target) = target else {
        return Ok((None, None));
    };
    let entity = resolved
        .get(target)
        .ok_or("The destination entity was deleted")?;
    if placement == Placement::Inside {
        return Ok((Some(target.into()), None));
    }
    let parent = entity.parent.as_deref();
    let index = siblings(resolved, parent)
        .iter()
        .position(|id| id.as_str() == target)
        .ok_or("The destination is no longer in the hierarchy")?;
    Ok((
        entity.parent.clone(),
        Some(index + usize::from(placement == Placement::After)),
    ))
}

/// Use the selection when it contains the pressed row; descendants travel with their selected root.
pub(super) fn commands(
    resolved: &Resolved,
    selection: &[String],
    pressed: &str,
    target: Option<&str>,
    placement: Placement,
    locked: &BTreeSet<String>,
) -> Result<Vec<Value>, String> {
    if !resolved.contains_key(pressed) {
        return Err("The dragged entity was deleted".into());
    }
    let selected: BTreeSet<&str> = if selection.iter().any(|id| id == pressed) {
        selection.iter().map(String::as_str).collect()
    } else {
        BTreeSet::from([pressed])
    };
    if selected.iter().any(|id| !resolved.contains_key(*id)) {
        return Err("A selected entity was deleted; select the surviving entities again".into());
    }
    if selected.iter().any(|id| locked.contains(*id)) {
        return Err("Unlock the selected entities before moving them".into());
    }
    // Sort only the selected paths, not the entire scene. Their ancestor order keys
    // establish tree display order without rescanning every sibling group per pointer move.
    let mut roots = Vec::new();
    for &id in &selected {
        if id.contains('/') {
            return Err("Move the prefab instance instead of an entity inside it".into());
        }
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
            path.reverse();
            roots.push((path, id));
        }
    }
    roots.sort_by(|(a, _), (b, _)| a.cmp(b));
    let roots: Vec<_> = roots.into_iter().map(|(_, id)| id).collect();
    if roots.is_empty() {
        return Err("No movable entities are selected".into());
    }
    let (parent, index) = destination(resolved, target, placement)?;
    if target.is_some_and(|target| selected.contains(target)) {
        return Err("Drop beside an unselected row or inside another entity".into());
    }
    if parent.as_ref().is_some_and(|p| locked.contains(p)) {
        return Err("The destination is locked".into());
    }
    let mut ancestor = parent.as_deref();
    let mut seen = BTreeSet::new();
    while let Some(id) = ancestor {
        if roots.contains(&id) {
            return Err("An entity cannot be moved inside itself or its descendants".into());
        }
        if !seen.insert(id) {
            return Err("The hierarchy contains a parent cycle".into());
        }
        ancestor = resolved.get(id).and_then(|e| e.parent.as_deref());
    }
    // Simulate each command's sibling list, because each command excludes only its own source.
    // This keeps adjacent and interleaved multi-selections stable in either drag direction.
    let mut destination: Vec<&str> = siblings(resolved, parent.as_deref())
        .into_iter()
        .map(String::as_str)
        .collect();
    let mut insert = index.unwrap_or(destination.len());
    let parent_world = match parent.as_deref() {
        Some(parent) => crate::world::world_transform(resolved, parent)
            .ok_or("The destination parent was deleted")?,
        None => Mat4::IDENTITY,
    };
    let mut calls = Vec::with_capacity(roots.len() * 2);
    for source in roots {
        let preserved = if resolved
            .get(source)
            .is_some_and(|entity| entity.parent == parent)
        {
            None
        } else {
            if !parent_world.is_finite() || parent_world.determinant().abs() < 1e-20 {
                return Err("The destination has a singular transform; restore a nonzero scale before reparenting".into());
            }
            let world = crate::world::world_transform(resolved, source)
                .ok_or("The dragged entity was deleted")?;
            let local = parent_world.inverse() * world;
            if !local.is_finite() || local.determinant().abs() < 1e-20 {
                return Err("The dragged transform cannot be preserved; restore a nonzero scale before reparenting".into());
            }
            let (scale, rotation, position) = local.to_scale_rotation_translation();
            let rebuilt = Mat4::from_scale_rotation_translation(scale, rotation, position);
            let error = local
                .to_cols_array()
                .into_iter()
                .zip(rebuilt.to_cols_array())
                .map(|(a, b)| (a - b).abs() / a.abs().max(1.))
                .fold(0_f32, f32::max);
            if !scale.is_finite() || !rotation.is_finite() || !position.is_finite() || error > 0.001
            {
                return Err("This parent would shear the object; use a parent with uniform scale to preserve its world pose".into());
            }
            let (x, y, z) = rotation.to_euler(EulerRot::XYZ);
            Some(
                json!({"cmd":"transform.set","args":{"entities":[source],"position":position.to_array(),"rotation":[x.to_degrees(),y.to_degrees(),z.to_degrees()],"scale":scale.to_array()}}),
            )
        };
        if let Some(old) = destination.iter().position(|id| *id == source) {
            destination.remove(old);
            if old < insert {
                insert = insert.saturating_sub(1);
            }
        }
        insert = insert.min(destination.len());
        let command_index =
            u32::try_from(insert).map_err(|_| "The hierarchy has too many siblings")?;
        calls.push(json!({"cmd":"entity.reparent","args":{"entity":source,"parent":parent,"index":command_index}}));
        if let Some(preserved) = preserved {
            calls.push(preserved);
        }
        destination.insert(insert, source);
        insert += 1;
    }
    Ok(calls)
}
