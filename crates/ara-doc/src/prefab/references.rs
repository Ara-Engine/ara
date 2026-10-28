//! Typed entity references follow the same namespace change as prefab addresses.
use crate::{Record, is_component_key};
use ara_schema::{SchemaDb, ValueType};
use serde_json::Value;

pub(super) fn remap(db: &SchemaDb, record: &mut Record, map: &impl Fn(&str) -> Option<String>) {
    for (name, value) in record.iter_mut().filter(|(name, _)| is_component_key(name)) {
        value_refs(db, &ValueType::Named(name.clone()), value, map);
    }
}

fn value_refs(
    db: &SchemaDb,
    ty: &ValueType,
    value: &mut Value,
    map: &impl Fn(&str) -> Option<String>,
) {
    match ty {
        ValueType::Entity => {
            if let Some(next) = value.as_str().and_then(map) {
                *value = Value::String(next);
            }
        }
        ValueType::Option(inner) => value_refs(db, inner, value, map),
        ValueType::List(inner) | ValueType::Keyed(inner) => {
            for value in value.as_array_mut().into_iter().flatten() {
                value_refs(db, inner, value, map);
            }
        }
        ValueType::Map(inner) => {
            for value in value
                .as_object_mut()
                .into_iter()
                .flat_map(|map| map.values_mut())
            {
                value_refs(db, inner, value, map);
            }
        }
        ValueType::Named(name) => {
            if let Some(desc) = db.get(name)
                && let Some(object) = value.as_object_mut()
            {
                for field in desc.fields() {
                    if let Some(value) = object.get_mut(&field.name) {
                        value_refs(db, &field.ty, value, map);
                    }
                }
            }
        }
        _ => {}
    }
}
