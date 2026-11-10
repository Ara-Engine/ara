//! Runtime component storage: inline scalar/vector fields plus owned variable values.

use crate::{Result, SimError};
use ara_schema::{SchemaDb, TypeDesc, ValueType};
use bevy_ecs::{
    component::{ComponentCloneBehavior, ComponentDescriptor, ComponentId, StorageType},
    prelude::*,
    ptr::OwningPtr,
};
use serde_json::{Map, Value};
use std::{
    alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error},
    ptr::NonNull,
};

/// One field's byte offset in a runtime component; offsets include the private header.
#[derive(Debug, Clone)]
pub struct LayoutField {
    /// Schema field name.
    pub name: String,
    /// Schema value type.
    pub ty: ValueType,
    /// Offset from the component's base pointer.
    pub offset: usize,
    /// Bytes occupied by the field (variable values store an eight-byte slot index).
    pub size: usize,
}

/// A runtime schema's aligned layout. Scalars/vectors are inline; other values are owned.
#[derive(Debug, Clone)]
pub struct DynamicLayout {
    pub(crate) name: String,
    layout: Layout,
    fields: Vec<LayoutField>,
}

// All tail values are Send + Sync and move with the header. No stored self-pointers.
struct Header {
    tail: Vec<Value>,
}

impl DynamicLayout {
    pub(crate) fn new(desc: &TypeDesc) -> Result<Self> {
        let mut fields = Vec::new();
        let mut layout = Layout::new::<Header>();
        for field in desc.fields() {
            let (size, align) = match field.ty {
                ValueType::Bool | ValueType::U8 => (1, 1),
                ValueType::U16 => (2, 2),
                ValueType::I32 | ValueType::U32 | ValueType::F32 => (4, 4),
                ValueType::Vec2 => (8, 4),
                ValueType::Vec3 => (12, 4),
                ValueType::Vec4 | ValueType::Quat | ValueType::Color => (16, 4),
                _ => (8, 8),
            };
            let field_layout =
                Layout::from_size_align(size, align).map_err(|e| SimError(e.to_string()))?;
            let (next, offset) = layout
                .extend(field_layout)
                .map_err(|e| SimError(e.to_string()))?;
            layout = next;
            fields.push(LayoutField {
                name: field.name.clone(),
                ty: field.ty.clone(),
                offset,
                size,
            });
        }
        Ok(Self {
            name: desc.name.clone(),
            layout: layout.pad_to_align(),
            fields,
        })
    }

    /// The table element layout passed to Bevy's ComponentDescriptor.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Fields in schema declaration order.
    pub fn fields(&self) -> &[LayoutField] {
        &self.fields
    }

    pub(crate) fn register(&self, world: &mut World) -> ComponentId {
        // SAFETY: the layout contains an initialized Header followed by plain bytes;
        // drop_header drops exactly the Header. Every contained value is Send + Sync.
        let desc = unsafe {
            ComponentDescriptor::new_with_layout(
                format!("ara.dynamic.{}", self.name),
                StorageType::Table,
                self.layout,
                Some(drop_header),
                true,
                false,
                ComponentCloneBehavior::Ignore,
                None,
            )
        };
        world.register_component_with_descriptor(desc)
    }

    pub(crate) fn insert(
        &self,
        world: &mut World,
        id: ComponentId,
        entity: Entity,
        value: &Value,
    ) -> Result<()> {
        // Encode before allocation: a validation/encoding error cannot leak a Header.
        let mut bytes = vec![0u8; self.layout.size()];
        let mut tail = Vec::new();
        for f in &self.fields {
            let v = value
                .get(&f.name)
                .ok_or_else(|| SimError(format!("missing materialized field {}", f.name)))?;
            let out = &mut bytes[f.offset..f.offset + f.size];
            encode(&f.ty, v, out, &mut tail)?;
        }
        let mut target = world
            .get_entity_mut(entity)
            .map_err(|e| SimError(e.to_string()))?;
        // SAFETY: layout is nonzero (it contains a Header), with valid size/alignment.
        let allocation = unsafe { alloc_zeroed(self.layout) };
        let Some(ptr) = NonNull::new(allocation) else {
            handle_alloc_error(self.layout)
        };
        // SAFETY: allocation has the registered layout. We initialize Header and all
        // field bytes before transferring ownership to Bevy, then free only the moved
        // allocation. No references into it escape and no Header is dropped twice.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), allocation, bytes.len());
            allocation.cast::<Header>().write(Header { tail });
            target.insert_by_id(id, OwningPtr::new(ptr));
            dealloc(allocation, self.layout);
        }
        Ok(())
    }

    pub(crate) fn read(
        &self,
        world: &World,
        id: ComponentId,
        entity: Entity,
    ) -> Result<Option<Value>> {
        let Some(ptr) = world.get_by_id(entity, id) else {
            return Ok(None);
        };
        // SAFETY: id was registered from this layout in this world. The component is
        // initialized only by insert; the shared World borrow prevents mutation.
        let (bytes, header) = unsafe {
            (
                std::slice::from_raw_parts(ptr.as_ptr(), self.layout.size()),
                &*ptr.as_ptr().cast::<Header>(),
            )
        };
        let mut out = Map::new();
        for f in &self.fields {
            out.insert(
                f.name.clone(),
                decode(&f.ty, &bytes[f.offset..f.offset + f.size], &header.tail)?,
            );
        }
        Ok(Some(Value::Object(out)))
    }
}

unsafe fn drop_header(ptr: OwningPtr<'_>) {
    // SAFETY: each dynamic descriptor has a fully initialized Header at offset zero.
    unsafe { std::ptr::drop_in_place(ptr.as_ptr().cast::<Header>()) };
}

fn encode(ty: &ValueType, v: &Value, out: &mut [u8], tail: &mut Vec<Value>) -> Result<()> {
    let number = || {
        v.as_f64()
            .ok_or_else(|| SimError("expected finite number".into()))
    };
    let unsigned = || {
        v.as_u64()
            .ok_or_else(|| SimError("expected unsigned integer".into()))
    };
    match ty {
        ValueType::Bool => {
            out[0] = u8::from(
                v.as_bool()
                    .ok_or_else(|| SimError("expected boolean".into()))?,
            )
        }
        ValueType::U8 => out[0] = unsigned()? as u8,
        ValueType::U16 => out.copy_from_slice(&(unsigned()? as u16).to_ne_bytes()),
        ValueType::U32 => out.copy_from_slice(&(unsigned()? as u32).to_ne_bytes()),
        ValueType::U64 => out.copy_from_slice(&unsigned()?.to_ne_bytes()),
        ValueType::I32 | ValueType::I64 => {
            let i = v
                .as_i64()
                .ok_or_else(|| SimError("expected signed integer".into()))?;
            if *ty == ValueType::I32 {
                out.copy_from_slice(&(i as i32).to_ne_bytes());
            } else {
                out.copy_from_slice(&i.to_ne_bytes());
            }
        }
        ValueType::F32 => out.copy_from_slice(&(number()? as f32).to_ne_bytes()),
        ValueType::F64 => out.copy_from_slice(&number()?.to_ne_bytes()),
        t if t.vector_len().is_some() => {
            let values = v
                .as_array()
                .ok_or_else(|| SimError("expected vector".into()))?;
            for (slot, value) in out.as_chunks_mut::<4>().0.iter_mut().zip(values) {
                slot.copy_from_slice(
                    &(value
                        .as_f64()
                        .ok_or_else(|| SimError("expected vector number".into()))?
                        as f32)
                        .to_ne_bytes(),
                );
            }
        }
        _ => {
            out.copy_from_slice(&(tail.len() as u64).to_ne_bytes());
            tail.push(v.clone());
        }
    }
    Ok(())
}

fn decode(ty: &ValueType, bytes: &[u8], tail: &[Value]) -> Result<Value> {
    macro_rules! read {
        ($t:ty) => {
            <$t>::from_ne_bytes(
                bytes
                    .try_into()
                    .map_err(|_| SimError("invalid dynamic field width".into()))?,
            )
        };
    }
    Ok(match ty {
        ValueType::Bool => Value::from(bytes[0] != 0),
        ValueType::U8 => Value::from(bytes[0]),
        ValueType::U16 => Value::from(read!(u16)),
        ValueType::U32 => Value::from(read!(u32)),
        ValueType::U64 => Value::from(read!(u64)),
        ValueType::I32 => Value::from(read!(i32)),
        ValueType::I64 => Value::from(read!(i64)),
        ValueType::F32 => serde_json::to_value(read!(f32))?,
        ValueType::F64 => serde_json::to_value(read!(f64))?,
        t if t.vector_len().is_some() => Value::Array(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| {
                    let a = [b[0], b[1], b[2], b[3]];
                    Value::from(f32::from_ne_bytes(a))
                })
                .collect(),
        ),
        _ => tail
            .get(read!(u64) as usize)
            .cloned()
            .ok_or_else(|| SimError("invalid dynamic field slot".into()))?,
    })
}

/// Copies the source once, then fills defaults and normalizes its owned values.
pub(crate) fn materialize(db: &SchemaDb, ty: &ValueType, value: &Value) -> Value {
    let mut complete = value.clone();
    materialize_in_place(db, ty, &mut complete);
    complete
}

/// Preserves existing key order and unknown values while visiting reflected fields.
fn materialize_in_place(db: &SchemaDb, ty: &ValueType, value: &mut Value) {
    match ty {
        ValueType::I32 | ValueType::I64 if value.as_i64().is_none() => {
            if let Some(number) = value.as_f64() {
                *value = Value::from(number as i64);
            }
        }
        ValueType::U8 | ValueType::U16 | ValueType::U32 | ValueType::U64
            if value.as_u64().is_none() =>
        {
            if let Some(number) = value.as_f64() {
                *value = Value::from(number as u64);
            }
        }
        ValueType::Named(name) => {
            if let (Some(desc), Some(object)) = (db.get(name), value.as_object_mut()) {
                for field in desc.fields() {
                    let slot = object
                        .entry(field.name.clone())
                        .or_insert_with(|| db.field_default(field));
                    materialize_in_place(db, &field.ty, slot);
                }
            }
        }
        ValueType::Option(inner) if !value.is_null() => materialize_in_place(db, inner, value),
        ValueType::List(inner) | ValueType::Keyed(inner) => {
            if let Some(items) = value.as_array_mut() {
                for item in items {
                    materialize_in_place(db, inner, item);
                }
            }
        }
        ValueType::Map(inner) => {
            if let Some(items) = value.as_object_mut() {
                for item in items.values_mut() {
                    materialize_in_place(db, inner, item);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ara_schema::Schema;
    use serde_json::json;

    use super::*;

    /// Nested defaults and numeric normalization must survive every container.
    #[derive(Schema)]
    #[allow(dead_code)]
    struct Child {
        /// Integral stored value.
        #[ara(default = 7)]
        amount: u64,
        /// Floating point payload, including signed zero.
        signed: f64,
    }

    /// Complete reflected input with owned variable data.
    #[derive(Schema)]
    #[allow(dead_code)]
    struct Payload {
        /// Nested value.
        child: Child,
        /// Optional nested value.
        optional: Option<Child>,
        /// Ordered values.
        items: Vec<Child>,
        /// Named values.
        entries: BTreeMap<String, Child>,
        /// Unreflected JSON remains untouched.
        raw: Value,
    }

    #[test]
    fn nested_materialization_preserves_source_order_unknowns_defaults_and_float_bits() {
        let mut db = SchemaDb::new();
        db.register::<Child>();
        db.register::<Payload>();
        let source = json!({
            "raw":{"keep":[2.0,-0.0]}, "child":{"signed":-0.0,"extra":"retained"},
            "items":[{"amount":2.0},{}], "entries":{"first":{"amount":3.0}},
            "optional":{"amount":4.0}, "unknown":["unchanged"]
        });
        let original = source.clone();
        let complete = materialize(&db, &ValueType::Named("Payload".into()), &source);
        assert_eq!(source, original, "materialization never edits its source");
        assert_eq!(complete["child"]["amount"], 7);
        assert_eq!(complete["child"]["extra"], "retained");
        assert_eq!(complete["items"][0]["amount"], 2);
        assert_eq!(complete["items"][1]["amount"], 7);
        assert_eq!(complete["entries"]["first"]["amount"], 3);
        assert_eq!(complete["optional"]["amount"], 4);
        assert_eq!(complete["unknown"], source["unknown"]);
        assert_eq!(complete["raw"], source["raw"]);
        assert_eq!(
            complete["child"]["signed"]
                .as_f64()
                .expect("float")
                .to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            complete
                .as_object()
                .expect("object")
                .keys()
                .collect::<Vec<_>>(),
            source
                .as_object()
                .expect("object")
                .keys()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            materialize(
                &db,
                &ValueType::Option(Box::new(ValueType::Named("Child".into()))),
                &Value::Null
            ),
            Value::Null
        );
        let keyed = materialize(
            &db,
            &ValueType::Keyed(Box::new(ValueType::Named("Child".into()))),
            &json!([{"id":"s_a","amount":6.0}]),
        );
        assert_eq!(keyed[0]["id"], "s_a");
        assert_eq!(keyed[0]["amount"], 6);
        assert_eq!(materialize(&db, &ValueType::I64, &json!(-3.0)), json!(-3));
        assert_eq!(
            materialize(&db, &ValueType::Named("Missing".into()), &source),
            source
        );
    }
}
