use super::*;
use crate::shell::Binding;

pub(super) fn require(ok: bool, message: &str) -> ara_ui::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.to_owned().into())
    }
}
pub(super) fn selection(editor: &Editor) -> Value {
    let mut hash = blake3::Hasher::new();
    for id in &editor.state.selection {
        hash.update(id.as_bytes());
        hash.update(&[0]);
    }
    json!({"count":editor.state.selection.len(),"first":editor.state.selection.first(),"last":editor.state.selection.last(),
        "orderedHash":hash.finalize().to_hex().to_string()})
}
pub(super) fn focus(shell: &Shell) -> Option<String> {
    let id = shell.input.focused()?;
    match shell.bindings.get(&id) {
        Some(Binding::Select(entity)) => Some(entity.clone()),
        _ => shell
            .ui
            .by_id(id)
            .map(|i| shell.ui.nodes[i].element.id.clone()),
    }
}
pub(super) fn rows(shell: &Shell) -> Value {
    let mut rows = Vec::new();
    for (id, binding) in &shell.bindings {
        let Binding::Select(entity) = binding else {
            continue;
        };
        let Some(index) = shell.ui.by_id(*id).filter(|i| shell.ui.shown(*i)) else {
            continue;
        };
        let node = &shell.ui.nodes[index];
        if node.rect.intersection(node.clip).h <= 2. {
            continue;
        }
        let padding = node
            .element
            .children
            .iter()
            .filter_map(|child| shell.ui.nodes[*child].element.attrs.get("style"))
            .find_map(|style| {
                style.split(';').find_map(|part| {
                    part.trim()
                        .strip_prefix("padding-left:")
                        .and_then(|value| value.trim().strip_suffix("px"))
                        .and_then(|value| value.parse::<f32>().ok())
                })
            });
        rows.push((
            node.rect.y,
            json!({"entity":entity,"stableId":id,"y":node.rect.y,"paddingLeft":padding}),
        ));
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    json!(rows.into_iter().map(|(_, value)| value).collect::<Vec<_>>())
}
pub(super) fn row_ids(shell: &Shell) -> Vec<String> {
    rows(shell)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["entity"].as_str().map(str::to_owned))
        .collect()
}
pub(super) fn children(entities: &ara_doc::prefab::Resolved, parent: &str) -> Vec<String> {
    let mut list: Vec<_> = entities
        .iter()
        .filter(|(_, e)| e.parent.as_deref() == Some(parent))
        .collect();
    list.sort_by(|(a, ea), (b, eb)| ea.order.cmp(&eb.order).then(a.cmp(b)));
    list.into_iter().map(|(id, _)| id.clone()).collect()
}
pub(super) fn unchanged(check: &HierarchyCheck, editor: &Editor) -> ara_ui::Result<()> {
    require(
        editor.host.store().revision() == check.before.revision,
        "A navigation-only hierarchy action authored content ops",
    )?;
    require(
        fixture::records(editor)? == check.before.records,
        "A navigation-only hierarchy action changed scene content",
    )?;
    require(
        editor.state.camera == check.before.camera,
        "A hierarchy action moved the camera",
    )
}
pub(super) fn flags(actual: &BTreeSet<String>, before: &BTreeSet<String>, added: &[&str]) -> bool {
    actual.len() == before.len() + added.iter().filter(|id| !before.contains(**id)).count()
        && before.is_subset(actual)
        && added.iter().all(|id| actual.contains(*id))
}
pub(super) fn exact_range(editor: &Editor, start: usize, end: usize) -> bool {
    if editor.state.selection.len() != end.saturating_sub(start) {
        return false;
    }
    let mut seen = vec![false; end.saturating_sub(start)];
    for id in &editor.state.selection {
        let Some(index) = id
            .strip_prefix("e_hstress")
            .and_then(|index| index.parse::<usize>().ok())
        else {
            return false;
        };
        if !(start..end).contains(&index) || seen[index - start] {
            return false;
        }
        seen[index - start] = true;
    }
    true
}
pub(super) fn same_world(check: &HierarchyCheck, entities: &ara_doc::prefab::Resolved) -> bool {
    check.before.world.iter().all(|(id, before)| {
        crate::world::world_transform(entities, id).is_some_and(|matrix| {
            before
                .iter()
                .zip(matrix.to_cols_array())
                .all(|(a, b)| (a - b).abs() < 0.0001 * a.abs().max(1.))
        })
    })
}
