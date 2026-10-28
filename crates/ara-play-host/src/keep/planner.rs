//! Pure Keep eligibility shared by read-only previews and final authoring applies.
use super::*;
use ara_schema::{FieldPath, Validator};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Candidate {
    pub entity: String,
    pub path: String,
    pub play_start: Option<Value>,
    pub play: Option<Value>,
    pub edit: Option<Value>,
    pub driven: bool,
    pub eligible: bool,
    pub status: &'static str,
    pub reason: Option<String>,
}
pub(super) struct Plan {
    pub candidates: Vec<Candidate>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(super) fn classify(
    store: &ara_ops::Store,
    scene: &str,
    mut changes: Vec<ara_runtime::live::Change>,
    entities: Option<&[String]>,
) -> Result<Plan, Vec<Diagnostic>> {
    let schema = store.schema();
    let doc = store.workspace().doc(scene).ok_or_else(|| {
        fail(
            codes::REQUEST,
            format!("Scene `{scene}` is no longer loaded"),
        )
    })?;
    let mut diagnostics = Vec::new();
    changes.retain(|change| entities.is_none_or(|entities| entities.contains(&change.entity)));
    let roots: std::collections::BTreeSet<_> = changes
        .iter()
        .map(|change| change.entity.split('/').next().unwrap_or(&change.entity))
        .collect();
    let mut resolved = ara_doc::prefab::Resolved::new();
    for root in roots {
        let Ok(root) = ara_doc::RecordId::parse(root) else {
            continue;
        };
        if let Some(entities) = ara_doc::prefab::resolve_record_reporting(
            doc,
            &root,
            schema,
            store.workspace(),
            &ara_diag::Location::default(),
            Some(&mut diagnostics),
        ) {
            resolved.extend(entities);
        }
    }
    let candidates = changes
        .into_iter()
        .map(|change| {
            let record = resolved.get(&change.entity).map(|entity| &entity.record);
            let edit = record.and_then(|record| current_value(record, schema, &change.path));
            let reason = if change.path.is_empty() {
                Some(("unsupported", "Entity was deleted in Play".into()))
            } else if change.value.is_none() {
                Some((
                    "unsupported",
                    "Component or field was removed in Play".into(),
                ))
            } else if record.is_none() {
                Some((
                    "conflict",
                    "Entity no longer exists in the authored scene".into(),
                ))
            } else if same(&edit, &change.value) {
                Some(("unchanged", "Authored value already matches runtime".into()))
            } else if !same(&edit, &change.prev) {
                Some((
                    "conflict",
                    "Authored value changed since Play started".into(),
                ))
            } else {
                unsupported(schema, record, &change)
            };
            Candidate {
                entity: change.entity,
                path: change.path,
                play_start: change.prev,
                play: change.value,
                edit,
                driven: change.driven,
                eligible: reason.is_none(),
                status: reason.as_ref().map_or("eligible", |(status, _)| *status),
                reason: reason.map(|(_, reason)| reason),
            }
        })
        .collect();
    Ok(Plan {
        candidates,
        diagnostics,
    })
}

fn unsupported(
    schema: &SchemaDb,
    record: Option<&ara_doc::Record>,
    change: &ara_runtime::live::Change,
) -> Option<(&'static str, String)> {
    let result: Result<(), String> = (|| {
        let path: FieldPath = change.path.parse()?;
        let entity = schema
            .get("Entity")
            .ok_or("Scene entity schema is unavailable")?;
        let field = schema
            .resolve_path(entity, &path)
            .map_err(|diagnostic| diagnostic.message)?;
        if path.head() != "name" && !ara_doc::is_component_key(path.head()) {
            return Err("This structural change needs its dedicated authoring command".into());
        }
        if ara_doc::is_component_key(path.head())
            && record.is_none_or(|record| !record.contains_key(path.head()))
        {
            return Err("Component is not present in the authored scene".into());
        }
        let value = change.value.as_ref().ok_or("Runtime value is absent")?;
        let mut diagnostics = Vec::new();
        Validator::new(schema, &mut diagnostics).value(&field.ty, field.field, value, &change.path);
        if let Some(diagnostic) = diagnostics.into_iter().find(Diagnostic::is_error) {
            return Err(diagnostic.message);
        }
        Ok(())
    })();
    result.err().map(|reason| ("unsupported", reason))
}

pub(super) fn selected(params: &PlayKeepParams, candidate: &Candidate) -> bool {
    (params.include_driven.unwrap_or(false) || !candidate.driven)
        && params.fields.as_ref().is_none_or(|fields| {
            fields
                .iter()
                .any(|field| field.entity == candidate.entity && field.path == candidate.path)
        })
        && params
            .paths
            .as_ref()
            .is_none_or(|paths| paths.iter().any(|path| candidate.path.starts_with(path)))
}

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
