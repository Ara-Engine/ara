//! Repair typed scene references to children moved behind a new prefab boundary.
use super::*;
use ara_schema::{SchemaDb, ValueType};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn replacement(address: &str, remap: &BTreeMap<String, String>) -> Option<String> {
    remap.get(address).cloned().or_else(|| {
        address
            .split_once('/')
            .and_then(|(head, inner)| remap.get(head).map(|head| format!("{head}/{inner}")))
    })
}

/// Keep references outside the new asset on its scene instance, while internal
/// references become local to the new asset's namespace.
pub(super) fn rewrite_record(
    db: &SchemaDb,
    record: &mut Record,
    source_ids: &BTreeMap<String, String>,
    scene_ids: &BTreeMap<String, String>,
    copied: &str,
    root: bool,
    overrides: &mut Map<String, Value>,
) {
    for (name, value) in record {
        if name == field::PARENT {
            if let Some(next) = value
                .as_str()
                .and_then(|address| replacement(address, source_ids))
            {
                *value = json!(next);
            }
        } else if ara_doc::is_component_key(name) {
            source_value(
                db,
                &ValueType::Named(name.clone()),
                value,
                source_ids,
                scene_ids,
                &override_key(copied, root, name),
                overrides,
            );
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
                    source_value(
                        db,
                        &resolved.ty,
                        value,
                        source_ids,
                        scene_ids,
                        &override_key(copied, root, key),
                        overrides,
                    );
                }
            }
        }
    }
}

fn override_key(copied: &str, root: bool, key: &str) -> String {
    if root && !key.contains('/') {
        key.into()
    } else {
        format!("{copied}/{key}")
    }
}

fn source_value(
    db: &SchemaDb,
    ty: &ValueType,
    value: &mut Value,
    source_ids: &BTreeMap<String, String>,
    scene_ids: &BTreeMap<String, String>,
    key: &str,
    overrides: &mut Map<String, Value>,
) {
    let mut external = false;
    db.visit(ty, value, key, &mut |ty, value, _| {
        external |= matches!(ty, ValueType::Entity)
            && value
                .as_str()
                .is_some_and(|address| replacement(address, source_ids).is_none());
    });
    if external {
        let mut scene = value.clone();
        replace(db, ty, &mut scene, scene_ids);
        overrides.insert(key.into(), scene);
    }
    super::super::references::map_refs(db, ty, value, &|address| {
        Some(replacement(address, source_ids).map_or(Value::Null, Value::String))
    });
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
    super::super::references::map_refs(db, ty, value, &|address| {
        replacement(address, remap).map(Value::String)
    })
}
