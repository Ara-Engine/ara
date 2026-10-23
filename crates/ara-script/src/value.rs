//! JSON ↔ bytes for blittable schema structs, using the canonical C layout
//! ([`ara_schema::SchemaDb::struct_layout`]). Agents and tools speak JSON; ECS
//! columns and C# speak bytes.
//!
//! Runtime entities are written as `"<index>v<generation>"` (`"12v1"`, bevy's text
//! form) or `null`; on input a raw `u64` (`Entity::to_bits`) is also accepted.

use ara_diag::{Diagnostic, codes};
use ara_schema::{FieldDesc, SchemaDb, Shape, StructLayout, ValueType};
use bevy_ecs::entity::Entity;
use serde_json::{Map, Value};

/// The bits of C#'s `Entity.Null` (`default(Entity)`): bevy stores the index as a
/// `NonMaxU32` (complemented), so all-zero bits are never a valid entity.
pub const NULL_ENTITY: u64 = 0;

/// Formats runtime entity bits as `"12v1"`, or `null` for invalid bits.
pub fn entity_json(bits: u64) -> Value {
    Entity::try_from_bits(bits).map_or(Value::Null, |e| {
        Value::String(format!("{}v{}", e.index().index(), e.generation().to_bits()))
    })
}

/// Parses `"12v1"`, a raw `u64` or `null` into entity bits.
pub fn parse_entity(v: &Value) -> Option<u64> {
    match v {
        Value::Null => Some(NULL_ENTITY),
        Value::Number(n) => n.as_u64(),
        Value::String(s) => {
            let (i, g) = s.split_once('v')?;
            let index = bevy_ecs::entity::EntityIndex::from_raw_u32(i.parse().ok()?)?;
            let generation = bevy_ecs::entity::EntityGeneration::from_bits(g.parse().ok()?);
            Some(Entity::from_index_and_generation(index, generation).to_bits())
        }
        _ => None,
    }
}

/// The entity for bits, if they are valid bevy entity bits.
pub fn entity_of(bits: u64) -> Option<Entity> {
    Entity::try_from_bits(bits)
}

/// Encodes `value` (an object; missing fields take their schema defaults) into a
/// buffer of `layout.size` bytes. `required` fields must be present. Errors carry
/// field paths under `path`.
pub fn encode(
    db: &SchemaDb,
    layout: &StructLayout,
    value: &Value,
    path: &str,
) -> Result<Vec<u8>, Diagnostic> {
    let mut out = vec![0u8; layout.size];
    encode_into(db, layout, value, path, &mut out)?;
    Ok(out)
}

/// The bytes of a struct with every field at its schema default.
pub fn default_bytes(db: &SchemaDb, layout: &StructLayout) -> Vec<u8> {
    let mut out = vec![0u8; layout.size];
    let _ = encode_into(
        db,
        layout,
        &Value::Object(Map::new()),
        "",
        &mut out,
    );
    out
}

fn encode_into(
    db: &SchemaDb,
    layout: &StructLayout,
    value: &Value,
    path: &str,
    out: &mut [u8],
) -> Result<(), Diagnostic> {
    let obj = value.as_object().ok_or_else(|| {
        Diagnostic::error(codes::TYPE_MISMATCH, format!("expected an object for `{}`", layout.name))
            .path(path)
    })?;
    let ty = db
        .get(&layout.name)
        .ok_or_else(|| Diagnostic::error(codes::UNKNOWN_TYPE, format!("unknown type `{}`", layout.name)))?;
    for key in obj.keys() {
        if ty.field(key).is_none() {
            return Err(Diagnostic::error(
                codes::UNKNOWN_FIELD,
                format!("`{}` has no field `{key}`", layout.name),
            )
            .path(join(path, key)));
        }
    }
    for (fl, f) in layout.fields.iter().zip(ty.fields()) {
        let at = join(path, &f.name);
        let v = match obj.get(&f.name) {
            Some(v) => v.clone(),
            None if f.required => {
                return Err(Diagnostic::error(
                    codes::MISSING_REQUIRED,
                    format!("`{}` is required", f.name),
                )
                .path(at));
            }
            None => db.field_default(f),
        };
        let slot = &mut out[fl.offset..fl.offset + fl.layout.size];
        encode_value(db, &f.ty, Some(f), &v, &at, slot)?;
    }
    Ok(())
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn mismatch(path: &str, expected: &str, v: &Value) -> Diagnostic {
    Diagnostic::error(codes::TYPE_MISMATCH, format!("expected {expected}, got `{v}`")).path(path)
}

fn number(v: &Value, f: Option<&FieldDesc>, ty: &ValueType, path: &str) -> Result<f64, Diagnostic> {
    let n = v.as_f64().ok_or_else(|| mismatch(path, "a number", v))?;
    if let Some((lo, hi)) = ty.int_bounds()
        && (n.fract() != 0.0 || (n as i128) < lo || (n as i128) > hi)
    {
        return Err(Diagnostic::error(
            codes::OUT_OF_RANGE,
            format!("`{n}` is not a valid `{ty}`"),
        )
        .path(path));
    }
    if let Some(r) = f.and_then(|f| f.range)
        && !r.contains(n)
    {
        return Err(
            Diagnostic::error(codes::OUT_OF_RANGE, format!("`{n}` is outside {r}")).path(path),
        );
    }
    Ok(n)
}

fn encode_value(
    db: &SchemaDb,
    ty: &ValueType,
    f: Option<&FieldDesc>,
    v: &Value,
    path: &str,
    out: &mut [u8],
) -> Result<(), Diagnostic> {
    macro_rules! put {
        ($t:ty) => {{
            let n = number(v, f, ty, path)? as $t;
            out.copy_from_slice(&n.to_le_bytes());
        }};
    }
    match ty {
        ValueType::Bool => out[0] = u8::from(v.as_bool().ok_or_else(|| mismatch(path, "a boolean", v))?),
        ValueType::U8 => put!(u8),
        ValueType::U16 => put!(u16),
        ValueType::I32 => put!(i32),
        ValueType::U32 => put!(u32),
        ValueType::I64 => put!(i64),
        ValueType::U64 => put!(u64),
        ValueType::F32 => put!(f32),
        ValueType::F64 => put!(f64),
        ValueType::Entity => {
            let bits = parse_entity(v).ok_or_else(|| mismatch(path, "an entity like `12v1`", v))?;
            out.copy_from_slice(&bits.to_le_bytes());
        }
        t if t.vector_len().is_some() => {
            let n = t.vector_len().unwrap_or(0);
            let items = v
                .as_array()
                .filter(|a| a.len() == n)
                .ok_or_else(|| mismatch(path, &format!("an array of {n} numbers"), v))?;
            for (i, item) in items.iter().enumerate() {
                let x = number(item, f, &ValueType::F32, &format!("{path}[#{i}]"))? as f32;
                out[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
            }
        }
        ValueType::Named(name) => match db.get(name).map(|t| &t.shape) {
            Some(Shape::Enum { variants }) => {
                let s = v.as_str().ok_or_else(|| mismatch(path, "a variant name", v))?;
                let i = variants
                    .iter()
                    .position(|x| x.name == s || x.aliases.iter().any(|a| a == s))
                    .ok_or_else(|| {
                        Diagnostic::error(codes::UNKNOWN_VARIANT, format!("`{s}` is not a `{name}`"))
                            .path(path)
                    })?;
                out.copy_from_slice(&u32::try_from(i).unwrap_or(u32::MAX).to_le_bytes());
            }
            Some(Shape::Struct { .. }) => {
                let nested = db
                    .struct_layout(name)
                    .map_err(|e| Diagnostic::error(codes::TYPE_MISMATCH, e).path(path))?;
                encode_into(db, &nested, v, path, out)?;
            }
            None => {
                return Err(Diagnostic::error(codes::UNKNOWN_TYPE, format!("unknown type `{name}`")).path(path));
            }
        },
        other => {
            return Err(Diagnostic::error(
                codes::TYPE_MISMATCH,
                format!("`{other}` is not blittable"),
            )
            .path(path));
        }
    }
    Ok(())
}

/// Decodes a struct's bytes (at least `layout.size` long) into JSON with every field.
pub fn decode(db: &SchemaDb, layout: &StructLayout, bytes: &[u8]) -> Value {
    let mut obj = Map::new();
    for fl in &layout.fields {
        let slot = bytes.get(fl.offset..fl.offset + fl.layout.size).unwrap_or(&[]);
        obj.insert(fl.name.clone(), decode_value(db, &fl.ty, slot));
    }
    Value::Object(obj)
}

/// An `f32` as the shortest JSON number that reads back to it (`0.1`, not `0.10000000149`).
fn f32_json(x: f32) -> Value {
    format!("{x}")
        .parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map_or(Value::Null, Value::Number)
}

fn decode_value(db: &SchemaDb, ty: &ValueType, b: &[u8]) -> Value {
    fn arr<const N: usize>(b: &[u8]) -> [u8; N] {
        b.get(..N).and_then(|s| s.try_into().ok()).unwrap_or([0; N])
    }
    match ty {
        ValueType::Bool => Value::Bool(b.first().is_some_and(|x| *x != 0)),
        ValueType::U8 => Value::from(b.first().copied().unwrap_or(0)),
        ValueType::U16 => Value::from(u16::from_le_bytes(arr(b))),
        ValueType::I32 => Value::from(i32::from_le_bytes(arr(b))),
        ValueType::U32 => Value::from(u32::from_le_bytes(arr(b))),
        ValueType::I64 => Value::from(i64::from_le_bytes(arr(b))),
        ValueType::U64 => Value::from(u64::from_le_bytes(arr(b))),
        ValueType::F32 => f32_json(f32::from_le_bytes(arr(b))),
        ValueType::F64 => serde_json::Number::from_f64(f64::from_le_bytes(arr(b)))
            .map_or(Value::Null, Value::Number),
        ValueType::Entity => entity_json(u64::from_le_bytes(arr(b))),
        t if t.vector_len().is_some() => Value::Array(
            (0..t.vector_len().unwrap_or(0))
                .map(|i| f32_json(f32::from_le_bytes(arr(b.get(i * 4..).unwrap_or(&[])))))
                .collect(),
        ),
        ValueType::Named(name) => match db.get(name).map(|t| &t.shape) {
            Some(Shape::Enum { variants }) => {
                let i = u32::from_le_bytes(arr(b)) as usize;
                variants
                    .get(i)
                    .map_or(Value::Null, |v| Value::String(v.name.clone()))
            }
            Some(Shape::Struct { .. }) => db
                .struct_layout(name)
                .map_or(Value::Null, |l| decode(db, &l, b)),
            None => Value::Null,
        },
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn db() -> SchemaDb {
        let mut db = SchemaDb::new();
        let problems = db.add_json_types(&json!({
            "$schema": "ara://schema/types/1",
            "types": {
                "Mode": { "kind": "enum", "doc": "Mode.", "variants": { "idle": { "doc": "I." }, "run": { "doc": "R." } } },
                "Probe": { "kind": "params", "doc": "Probe.", "fields": {
                    "who": { "type": "entity", "doc": "Target.", "required": true },
                    "hp": { "type": "u16", "doc": "Hit points.", "range": [0, 100], "default": 50 },
                    "at": { "type": "vec3", "doc": "Where." },
                    "mode": { "type": "Mode", "doc": "Mode." } } }
            }
        }));
        assert!(problems.is_empty(), "{problems:?}");
        db
    }

    #[test]
    fn round_trips_with_defaults_and_checks() {
        let db = db();
        let layout = db.struct_layout("Probe").expect("blittable");
        let bytes = encode(&db, &layout, &json!({ "who": "12v1", "at": [1, 0.5, -2], "mode": "run" }), "params")
            .expect("encodes");
        assert_eq!(
            decode(&db, &layout, &bytes),
            json!({ "who": "12v1", "hp": 50, "at": [1.0, 0.5, -2.0], "mode": "run" })
        );
        assert_eq!(entity_json(NULL_ENTITY), Value::Null);
        assert_eq!(parse_entity(&json!("12v1")).map(entity_json), Some(json!("12v1")));
        let err = encode(&db, &layout, &json!({ "who": null, "hp": 101 }), "params").expect_err("range");
        assert_eq!((err.code, err.location.path.as_deref()), (codes::OUT_OF_RANGE, Some("params.hp")));
        let err = encode(&db, &layout, &json!({}), "params").expect_err("required");
        assert_eq!(err.code, codes::MISSING_REQUIRED);
    }
}
