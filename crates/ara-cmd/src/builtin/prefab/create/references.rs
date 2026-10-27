//! Repair typed scene references to children moved behind a new prefab boundary.
use super::*;
use ara_schema::{SchemaDb, Shape, ValueType};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn replacement(address: &str, remap: &BTreeMap<String, String>) -> Option<String> {
    remap.get(address).cloned().or_else(|| {
        address
            .split_once('/')
            .and_then(|(head, inner)| remap.get(head).map(|head| format!("{head}/{inner}")))
    })
}

pub(super) fn rewrite_record(db: &SchemaDb, record: &mut Record, remap: &BTreeMap<String, String>) {
    for (name, value) in record {
        if name == field::PARENT {
            if let Some(next) = value
                .as_str()
                .and_then(|address| replacement(address, remap))
            {
                *value = json!(next);
            }
        } else if ara_doc::is_component_key(name) {
            replace(db, &ValueType::Named(name.clone()), value, remap);
        } else if name == field::OVERRIDES {
            for (key, value) in value
                .as_object_mut()
                .into_iter()
                .flat_map(|map| map.iter_mut())
            {
                let path = key.rsplit_once('/').map_or(key.as_str(), |(_, path)| path);
                if let Some(resolved) = path.parse::<FieldPath>().ok().and_then(|path| {
                    db.get("Entity")
                        .and_then(|entity| db.resolve_path(entity, &path).ok())
                }) {
                    replace(db, &resolved.ty, value, remap);
                }
            }
        }
    }
}

pub(super) fn remap_scene(
    ctx: &mut EditCtx<'_, '_>,
    copied: &BTreeSet<&str>,
    remap: &BTreeMap<String, String>,
) -> Result<(), Diagnostic> {
    let read = ctx.read();
    let doc = read.doc().id.clone();
    let mut edits = Vec::new();
    for (id, record) in read
        .doc()
        .records()
        .iter()
        .filter(|(id, _)| !copied.contains(id.as_str()))
    {
        for (name, value) in record {
            if ara_doc::is_component_key(name) {
                let mut value = value.clone();
                if replace(
                    read.schema(),
                    &ValueType::Named(name.clone()),
                    &mut value,
                    remap,
                ) {
                    edits.push(Edit::Set {
                        doc: doc.clone(),
                        id: id.clone(),
                        path: FieldPath::field(name),
                        value: Some(value),
                    });
                }
            } else if name == field::OVERRIDES {
                for (key, value) in value.as_object().into_iter().flatten() {
                    let path = key.rsplit_once('/').map_or(key.as_str(), |(_, path)| path);
                    let Some(resolved) = path.parse::<FieldPath>().ok().and_then(|path| {
                        read.schema()
                            .resolve_path(read.entity_type().ok()?, &path)
                            .ok()
                    }) else {
                        continue;
                    };
                    let mut value = value.clone();
                    if replace(read.schema(), &resolved.ty, &mut value, remap) {
                        edits.push(Edit::Set {
                            doc: doc.clone(),
                            id: id.clone(),
                            path: FieldPath::field(field::OVERRIDES).key(key),
                            value: Some(value),
                        });
                    }
                }
            }
        }
    }
    for edit in edits {
        ctx.apply_edit(edit)?;
    }
    Ok(())
}

fn replace(
    db: &SchemaDb,
    ty: &ValueType,
    value: &mut Value,
    remap: &BTreeMap<String, String>,
) -> bool {
    match ty {
        ValueType::Entity => {
            if let Some(replacement) = value
                .as_str()
                .and_then(|address| replacement(address, remap))
            {
                *value = json!(replacement);
                true
            } else {
                false
            }
        }
        ValueType::Option(inner) => replace(db, inner, value, remap),
        ValueType::List(inner) | ValueType::Keyed(inner) => value
            .as_array_mut()
            .into_iter()
            .flatten()
            .fold(false, |changed, value| {
                replace(db, inner, value, remap) || changed
            }),
        ValueType::Map(inner) => value
            .as_object_mut()
            .into_iter()
            .flat_map(|map| map.values_mut())
            .fold(false, |changed, value| {
                replace(db, inner, value, remap) || changed
            }),
        ValueType::Named(name) => {
            let Some(Shape::Struct { fields, .. }) = db.get(name).map(|ty| &ty.shape) else {
                return false;
            };
            let Some(map) = value.as_object_mut() else {
                return false;
            };
            fields.iter().fold(false, |changed, field| {
                map.get_mut(&field.name)
                    .is_some_and(|value| replace(db, &field.ty, value, remap))
                    || changed
            })
        }
        _ => false,
    }
}
