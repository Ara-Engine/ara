//! The canonical writer: `$schema` first, records sorted by ID, record fields in
//! reflection order, components sorted by name, defaults omitted, override values in
//! their target field's canonical form.

use std::collections::BTreeMap;

use ara_json::sort_keys;
use ara_schema::{FieldPath, SchemaDb, TypeDesc, ValueType};
use serde_json::{Map, Value};

use crate::document::{Document, Record, field, is_component_key};
use crate::refs::override_type;

/// Writes `doc` canonically. Reading the output and writing it again gives the same
/// bytes.
pub fn write(doc: &Document, db: &SchemaDb) -> String {
    ara_json::to_canonical_string(&to_value(doc, db))
}

/// The canonical JSON value of `doc`.
pub fn to_value(doc: &Document, db: &SchemaDb) -> Value {
    let kind = db.kind(&doc.kind);
    let mut versions = BTreeMap::new();
    let mut records = Map::new();
    for (id, rec) in doc.records() {
        let rt = kind.and_then(|k| db.record_type(k, id.prefix()));
        for name in used_types(rec) {
            if let Some(t) = db.get(name).filter(|t| t.version > 1) {
                versions.insert(t.name.clone(), Value::from(t.version));
            }
        }
        records.insert(id.to_string(), Value::Object(canonical_record(db, rt, rec)));
    }
    let mut root = Map::new();
    let uri = kind.map_or_else(
        || format!("ara://schema/{}/1", doc.kind),
        |k| k.schema_uri(),
    );
    root.insert("$schema".into(), uri.into());
    root.insert("id".into(), doc.id.to_string().into());
    if !versions.is_empty() {
        root.insert(
            "versions".into(),
            Value::Object(versions.into_iter().collect()),
        );
    }
    if !doc.conflicts.is_empty() {
        root.insert(
            "$conflicts".into(),
            Value::Array(doc.conflicts.iter().map(sort_conflict).collect()),
        );
    }
    root.insert("records".into(), Value::Object(records));
    Value::Object(root)
}

/// The canonical form of one record.
pub fn canonical_record(db: &SchemaDb, rt: Option<&TypeDesc>, rec: &Record) -> Record {
    let fields: Record = rec
        .iter()
        .filter(|(k, _)| !is_component_key(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut out = match rt {
        Some(rt) => db.canonical_struct(rt, &fields),
        None => sort_keys(&Value::Object(fields))
            .as_object()
            .cloned()
            .unwrap_or_default(),
    };
    if let (Some(rt), Some(Value::Object(overrides))) = (rt, out.get_mut(field::OVERRIDES)) {
        for (key, v) in overrides.iter_mut() {
            if let Some(ty) = override_type(db, rt, key) {
                *v = db.canonical_value(&ty, v);
            }
        }
    }
    let mut comps: Vec<&String> = rec.keys().filter(|k| is_component_key(k)).collect();
    comps.sort();
    for name in comps {
        let v = &rec[name];
        let cv = match db.get(name) {
            Some(t) if t.is_component() => db.canonical_value(&ValueType::Named(name.clone()), v),
            _ => sort_keys(v),
        };
        out.insert(name.clone(), cv);
    }
    out
}

/// Component types a record stores data for: inline components and whole-component
/// overrides.
fn used_types(rec: &Record) -> impl Iterator<Item = &str> {
    let inline = rec
        .keys()
        .filter(|k| is_component_key(k))
        .map(String::as_str);
    let overridden = rec
        .get(field::OVERRIDES)
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|o| o.keys())
        .filter_map(|k| {
            let path = k.rsplit_once('/').map_or(k.as_str(), |(_, p)| p);
            let parsed: FieldPath = path.parse().ok()?;
            (parsed.segments().len() == 1 && is_component_key(path)).then_some(path)
        });
    inline.chain(overridden)
}

/// Conflict entries keep a fixed key order: record, path, base, ours, theirs.
fn sort_conflict(c: &Value) -> Value {
    let Some(obj) = c.as_object() else {
        return c.clone();
    };
    let mut out = Map::new();
    const KEYS: [&str; 5] = ["record", "path", "base", "ours", "theirs"];
    for key in KEYS {
        if let Some(v) = obj.get(key) {
            out.insert(key.into(), sort_keys(v));
        }
    }
    for (k, v) in obj.iter().filter(|(k, _)| !KEYS.contains(&k.as_str())) {
        out.insert(k.clone(), sort_keys(v));
    }
    Value::Object(out)
}
