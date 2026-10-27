//! A skipped native document keeps its own record identities; never invent a match.
use super::*;
use crate::builtin::asset::JsonOwner;
use ara_schema::grammar::parse_ref;
use serde_json::Value;

pub(super) fn validate(
    workspace: &Workspace,
    schema: &SchemaDb,
    entries: &[ImportEntry],
    maps: &references::Mappings,
    documents: &[(String, Document)],
) -> Result<(), Diagnostic> {
    let skipped: BTreeMap<_, _> = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.disposition,
                ImportDisposition::Skipped | ImportDisposition::Merged
            )
        })
        .filter_map(|entry| {
            let foreign = entry.document.as_ref()?;
            let (id, _) = maps.assets.get(foreign.id.as_str())?;
            Some((id.to_string(), entry.destination.clone()))
        })
        .collect();
    if skipped.is_empty() {
        return Ok(());
    }
    let mut projected = workspace.clone();
    for (path, document) in documents {
        projected.insert(workspace.storage_path(path), document.clone());
    }
    for (path, document) in documents {
        for (id, record) in document.records() {
            for (field, value) in record {
                explicit(
                    &projected,
                    &skipped,
                    Some(&document.id),
                    value,
                    &workspace.storage_path(path),
                    &format!("{id}.{field}"),
                )?;
            }
        }
    }
    for entry in entries {
        if matches!(
            entry.disposition,
            ImportDisposition::Skipped | ImportDisposition::Merged
        ) {
            continue;
        }
        let Some(json) = &entry.json else { continue };
        let context = json
            .context
            .as_ref()
            .map(|id| maps.assets.get(id.as_str()).map_or(id, |(id, _)| id));
        let mut value = json.value.clone();
        maps.rewrite_json(json.owner, json.context.as_ref(), &mut value);
        let file = workspace.storage_path(&entry.destination);
        explicit(&projected, &skipped, context, &value, &file, "")?;
        if let Some(context) = context
            && let Some(kept) = skipped.get(context.as_str())
        {
            let resolved = projected.resolve(context, schema, &mut Vec::new());
            for (field, address) in entity_fields(json.owner, &value) {
                if !resolved.contains_key(address) {
                    return Err(missing(&file, &field, address, kept));
                }
            }
        }
    }
    Ok(())
}

fn missing(file: &str, field: &str, address: &str, kept: &str) -> Diagnostic {
    invalid(format!("incoming reference `{address}` is absent from skipped native asset `{kept}`; choose Replace or import matching native documents"))
        .file(file).path(field)
}

fn explicit(
    workspace: &Workspace,
    skipped: &BTreeMap<String, String>,
    context: Option<&AssetId>,
    value: &Value,
    file: &str,
    field: &str,
) -> Result<(), Diagnostic> {
    match value {
        Value::String(text) if text.starts_with('@') => {
            if let Ok(reference) = parse_ref(text)
                && let Some(id) = reference.id
            {
                let target = if let Some(path) = reference.path {
                    workspace.find(path)
                } else {
                    workspace.find(id).or(context)
                };
                if let Some(target) = target
                    && let Some(kept) = skipped.get(target.as_str())
                    && id != target.as_str()
                    && workspace
                        .doc(target.as_str())
                        .is_none_or(|doc| doc.record(id).is_none())
                {
                    return Err(missing(file, field, id, kept));
                }
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                explicit(
                    workspace,
                    skipped,
                    context,
                    value,
                    file,
                    &join(field, &index.to_string()),
                )?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                explicit(
                    workspace,
                    skipped,
                    context,
                    &Value::String(key.clone()),
                    file,
                    &join(field, key),
                )?;
                explicit(workspace, skipped, context, value, file, &join(field, key))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn join(parent: &str, field: &str) -> String {
    if parent.is_empty() {
        field.to_owned()
    } else {
        format!("{parent}.{field}")
    }
}

fn entity_fields(owner: JsonOwner, value: &Value) -> Vec<(String, &str)> {
    let mut fields = Vec::new();
    match owner {
        JsonOwner::UiBindings => {
            let (prefix, bindings) = if let Some(values) = value.as_array() {
                ("", Some(values))
            } else {
                ("bindings", value.get("bindings").and_then(Value::as_array))
            };
            if let Some(bindings) = bindings {
                for (index, binding) in bindings.iter().enumerate() {
                    if let Some(entity) = binding.get("entity").and_then(Value::as_str) {
                        fields.push((join(&join(prefix, &index.to_string()), "entity"), entity));
                    }
                }
            }
        }
        JsonOwner::UiResources => {
            if let Some(music) = value.get("music").and_then(Value::as_array) {
                for (index, track) in music.iter().enumerate() {
                    for field in ["entity", "target"] {
                        if let Some(entity) = track.get(field).and_then(Value::as_str) {
                            fields.push((format!("music.{index}.{field}"), entity));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    fields
}

#[cfg(test)]
mod tests;
