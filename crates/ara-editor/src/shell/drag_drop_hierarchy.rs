//! Validate hierarchy gestures and lower selection roots to one ordered command batch.
use crate::shell::hierarchy_actions::{preserved_transform, selection_roots};
use ara_doc::prefab::Resolved;
use ara_ui::drag_drop::Placement;
use glam::Mat4;
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
    let roots = selection_roots(resolved, &selected)?;
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
            Some(preserved_transform(resolved, source, parent_world)?)
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
