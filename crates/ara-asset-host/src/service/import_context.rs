//! Resolve imported UI entity owners before deterministic remapping.
use super::inventory::one;
use ara_cmd::builtin::ImportEntry;
use ara_diag::{Diagnostic, codes};
pub(super) fn bind_json_contexts(entries: &mut [ImportEntry]) -> Result<(), Vec<Diagnostic>> {
    let owners: Vec<_> = entries
        .iter()
        .filter_map(|entry| entry.document.as_ref())
        .filter(|document| matches!(document.kind.as_str(), "scene" | "prefab"))
        .map(|document| {
            (
                document.id.clone(),
                document
                    .records()
                    .keys()
                    .map(ToString::to_string)
                    .collect::<std::collections::BTreeSet<_>>(),
            )
        })
        .collect();
    for entry in entries {
        let Some(json) = entry.json.as_mut() else {
            continue;
        };
        if json.owner == ara_cmd::builtin::JsonOwner::Generic {
            continue;
        }
        let mut references = std::collections::BTreeSet::new();
        entity_references(&json.value, &mut references);
        let candidates: Vec<_> = owners
            .iter()
            .filter(|(_, records)| {
                references
                    .iter()
                    .any(|reference| records.contains(reference))
            })
            .collect();
        if candidates.len() > 1 {
            return Err(one(Diagnostic::error(codes::INVALID_PARAMS, "incoming UI bindings/music target more than one scene with overlapping entity identities; import each scene and its UI files as a separate selection").file(&entry.source_reference)));
        }
        json.context = candidates
            .first()
            .map(|(id, _)| id.clone())
            .or_else(|| (owners.len() == 1).then(|| owners[0].0.clone()));
    }
    Ok(())
}

pub(super) fn validate_native_ids(entries: &[ImportEntry]) -> Result<(), Vec<Diagnostic>> {
    let mut ids = std::collections::BTreeSet::new();
    for entry in entries {
        if entry
            .document
            .as_ref()
            .is_some_and(|document| document.kind == "import")
        {
            let media = entry
                .destination
                .strip_suffix(".import.json")
                .ok_or_else(|| {
                    one(Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "an import sidecar must keep its paired media extension",
                    ))
                })?;
            if !entries.iter().any(|incoming| {
                incoming.destination == media
                    && incoming.state.kind == ara_doc::file_state::FileKind::File
            }) {
                return Err(one(Diagnostic::error(codes::INVALID_PARAMS, "select the source media with its import sidecar so its settings and dependencies can be validated together").file(&entry.source_reference)));
            }
        }
        if let Some(document) = &entry.document
            && !ids.insert(document.id.clone())
        {
            return Err(one(Diagnostic::error(codes::INVALID_PARAMS, "incoming native assets repeat an asset ID; import separate selections to avoid ambiguous references").file(&entry.source_reference)));
        }
    }
    Ok(())
}

fn entity_references(
    value: &serde_json::Value,
    references: &mut std::collections::BTreeSet<String>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                if matches!(name.as_str(), "entity" | "target")
                    && let Some(reference) = value.as_str()
                {
                    references.insert(reference.split('/').next().unwrap_or(reference).to_owned());
                }
                entity_references(value, references);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                entity_references(value, references);
            }
        }
        _ => {}
    }
}
