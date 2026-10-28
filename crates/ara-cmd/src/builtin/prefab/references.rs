//! Rewrites schema-declared entity values without touching labels or asset references.
use ara_schema::{SchemaDb, ValueType};
use serde_json::Value;

pub(super) fn map_refs(
    db: &SchemaDb,
    ty: &ValueType,
    value: &mut Value,
    map: &impl Fn(&str) -> Option<Value>,
) -> bool {
    match ty {
        ValueType::Entity => {
            if let Some(next) = value.as_str().and_then(map) {
                let changed = *value != next;
                *value = next;
                changed
            } else {
                false
            }
        }
        ValueType::Option(inner) => map_refs(db, inner, value, map),
        ValueType::List(inner) | ValueType::Keyed(inner) => {
            let mut changed = false;
            for value in value.as_array_mut().into_iter().flatten() {
                changed |= map_refs(db, inner, value, map);
            }
            changed
        }
        ValueType::Map(inner) => {
            let mut changed = false;
            for value in value
                .as_object_mut()
                .into_iter()
                .flat_map(|map| map.values_mut())
            {
                changed |= map_refs(db, inner, value, map);
            }
            changed
        }
        ValueType::Named(name) => {
            let Some(desc) = db.get(name) else {
                return false;
            };
            let Some(object) = value.as_object_mut() else {
                return false;
            };
            let mut changed = false;
            for field in desc.fields() {
                if let Some(value) = object.get_mut(&field.name) {
                    changed |= map_refs(db, &field.ty, value, map);
                }
            }
            changed
        }
        _ => false,
    }
}
