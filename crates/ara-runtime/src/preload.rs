//! Model preparation follows the selected scene's document references. Inactive
//! cells of its world are included so activating them never decodes inside a tick.
use crate::{Result, assets};
use ara_doc::{AssetId, Workspace};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) fn models(workspace: &Workspace, scene: &AssetId) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    collect(workspace, BTreeSet::from([scene.to_string()]), &mut out)?;
    Ok(out)
}

/// Live prefab/world references are prepared at the authoring control boundary,
/// just like direct MeshRenderer/Collider references, before ECS adoption.
pub(crate) fn extend(
    workspace: &Workspace,
    value: &Value,
    models: &mut BTreeSet<String>,
) -> Result<()> {
    let mut pending = BTreeSet::new();
    references(workspace, value, &mut pending);
    collect(workspace, pending, models)
}

fn collect(
    workspace: &Workspace,
    mut pending: BTreeSet<String>,
    models: &mut BTreeSet<String>,
) -> Result<()> {
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop_first() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(document) = workspace.doc(&id) else {
            continue;
        };
        if matches!(document.kind.as_str(), "project" | "import") {
            continue;
        }
        for (record_id, record) in document.records() {
            for (key, value) in record {
                assets::model_references(key, value, models)?;
                references(workspace, value, &mut pending);
            }
            if document.kind == "foliage"
                && record_id.as_str().starts_with("r_")
                && let Some(reference) = record.get("asset").and_then(Value::as_str)
            {
                let reference = ara_schema::AssetRef(reference.to_owned());
                let asset = assets::asset_id(&reference)?;
                if !workspace.doc(asset).is_some_and(|doc| doc.kind == "prefab") {
                    models.insert(asset.to_owned());
                }
            }
        }
    }
    Ok(())
}

fn references(workspace: &Workspace, value: &Value, pending: &mut BTreeSet<String>) {
    match value {
        Value::String(text) if text.starts_with('@') => {
            let id = workspace.find(text).or_else(|| {
                ara_schema::grammar::parse_ref(text)
                    .ok()?
                    .path
                    .and_then(|path| workspace.find(path))
            });
            if let Some(id) = id {
                pending.insert(id.to_string());
            }
        }
        Value::Array(values) => {
            for value in values {
                references(workspace, value, pending);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                references(workspace, value, pending);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
