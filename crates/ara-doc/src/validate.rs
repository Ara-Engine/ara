//! Document-level validation: records against the schema plus the rules the schema
//! can't express (order keys, instance shape, parents, cycles, root count, entity
//! references).

use ara_diag::{Diagnostic, FixAction, Location, codes};
use ara_schema::{DocKindDesc, SchemaDb, TypeDesc, Validator, ValueType};
use serde_json::Value;

use crate::document::{Document, Record, field, is_component_key, order_of, parent_of, prefab_of};
use crate::{OrderKey, RecordId};

/// The record type for `id` in a document of `kind`, or a diagnostic.
pub fn record_type<'a>(
    db: &'a SchemaDb,
    kind: &DocKindDesc,
    id: &RecordId,
) -> Result<&'a TypeDesc, Diagnostic> {
    db.record_type(kind, id.prefix()).ok_or_else(|| {
        let prefixes: Vec<String> = kind
            .records
            .iter()
            .filter_map(|r| db.get(r)?.record_info().map(|i| format!("`{}_`", i.prefix)))
            .collect();
        Diagnostic::error(
            codes::INVALID_ID,
            format!(
                "`{id}`: {} documents hold records with IDs starting {}",
                kind.name,
                prefixes.join(", ")
            ),
        )
        .record(id.as_str())
    })
}

/// Validates one record. `base` should carry the file or document.
pub fn check_record(
    db: &SchemaDb,
    kind: &DocKindDesc,
    id: &RecordId,
    rec: &Record,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) {
    let mut base = base.clone();
    base.record = Some(id.to_string());
    let rt = match record_type(db, kind, id) {
        Ok(rt) => rt,
        Err(d) => return out.push(d.within(&base)),
    };
    Validator::new(db, out).at(base.clone()).record(rt, rec);
    if let Some(order) = order_of(rec)
        && let Err(msg) = OrderKey::parse(order)
    {
        out.push(
            Diagnostic::error(codes::INVALID_ORDER, msg)
                .path(field::ORDER)
                .within(&base),
        );
    }
    let instance = prefab_of(rec).is_some();
    if instance {
        for key in rec.keys().filter(|k| is_component_key(k)) {
            out.push(
                Diagnostic::error(
                    codes::INSTANCE_COMPONENTS,
                    format!("prefab instance carries component `{key}` inline; set prefab values through `overrides`"),
                )
                .path(key.as_str())
                .within(&base),
            );
        }
    } else {
        for key in [field::OVERRIDES, field::REMOVED]
            .into_iter()
            .filter(|k| rec.contains_key(*k))
        {
            out.push(
                Diagnostic::error(
                    codes::NOT_AN_INSTANCE,
                    format!("`{key}` only applies to prefab instances"),
                )
                .path(key)
                .within(&base)
                .with_fix(
                    format!("Remove `{key}`"),
                    FixAction::Remove {
                        record: Some(id.to_string()),
                        path: key.to_owned(),
                    },
                ),
            );
        }
    }
}

/// Checks that parents exist, `parent` links never loop, and single-root kinds have
/// exactly one root.
pub fn check_structure(
    doc: &Document,
    kind: &DocKindDesc,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) {
    let records = doc.records();
    for (id, rec) in records {
        let Some(parent) = parent_of(rec) else {
            continue;
        };
        let head = parent.split('/').next().unwrap_or(parent);
        let problem = match records.get(head) {
            None => Some(format!("parent `{parent}` does not exist")),
            Some(p) if parent.contains('/') && prefab_of(p).is_none() => Some(format!(
                "parent `{parent}` points inside `{head}`, which is not a prefab instance"
            )),
            _ => None,
        };
        if let Some(msg) = problem {
            out.push(
                Diagnostic::error(codes::MISSING_PARENT, msg)
                    .record(id.as_str())
                    .path(field::PARENT)
                    .within(base)
                    .with_fix(
                        "Make it a root",
                        FixAction::Remove {
                            record: Some(id.to_string()),
                            path: field::PARENT.to_owned(),
                        },
                    ),
            );
        }
    }
    for id in records.keys() {
        if in_cycle(doc, id) {
            out.push(
                Diagnostic::error(codes::PARENT_CYCLE, format!("`{id}` is its own ancestor"))
                    .record(id.as_str())
                    .path(field::PARENT)
                    .within(base),
            );
        }
    }
    if kind.single_root {
        let roots = doc.children(None).len();
        if roots != 1 {
            out.push(
                Diagnostic::error(
                    codes::ROOT_COUNT,
                    format!(
                        "{} documents need exactly one root record; found {roots}",
                        kind.name
                    ),
                )
                .within(base),
            );
        }
    }
}

/// True if following `parent` links from `id` leads back to `id`.
pub fn in_cycle(doc: &Document, id: &RecordId) -> bool {
    let mut cur = id.as_str();
    for _ in 0..=doc.records().len() {
        let Some(parent) = doc.record(cur).and_then(parent_of) else {
            return false;
        };
        cur = parent.split('/').next().unwrap_or(parent);
        if cur == id.as_str() {
            return true;
        }
    }
    false
}

/// Warns about `entity` fields in components that point at records the document
/// doesn't have (typically left behind by a delete).
pub fn check_entity_refs(
    db: &SchemaDb,
    doc: &Document,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) {
    for (id, rec) in doc.records() {
        for (key, v) in rec.iter().filter(|(k, _)| is_component_key(k)) {
            if !db.get(key).is_some_and(TypeDesc::is_component) {
                continue;
            }
            db.visit(
                &ValueType::Named(key.clone()),
                v,
                key,
                &mut |ty, v, path| {
                    let (ValueType::Entity, Value::String(target)) = (ty, v) else {
                        return;
                    };
                    let head = target.split('/').next().unwrap_or(target);
                    if doc.record(head).is_none() {
                        out.push(
                            Diagnostic::warning(
                                codes::DANGLING_ENTITY_REF,
                                format!("`{target}` does not exist"),
                            )
                            .record(id.as_str())
                            .path(path)
                            .within(base)
                            .with_fix(
                                "Clear the reference",
                                FixAction::Set {
                                    record: Some(id.to_string()),
                                    path: path.to_owned(),
                                    value: Value::Null,
                                },
                            ),
                        );
                    }
                },
            );
        }
    }
}
