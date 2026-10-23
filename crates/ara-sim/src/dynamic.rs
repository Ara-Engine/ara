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

pub(crate) fn materialize(db: &SchemaDb, ty: &ValueType, value: &Value) -> Value {
    match (ty, value) {
        (ValueType::I32 | ValueType::I64, Value::Number(number)) if number.as_i64().is_none() => {
            number
                .as_f64()
                .map(|n| Value::from(n as i64))
                .unwrap_or_else(|| value.clone())
        }
        (
            ValueType::U8 | ValueType::U16 | ValueType::U32 | ValueType::U64,
            Value::Number(number),
        ) if number.as_u64().is_none() => number
            .as_f64()
            .map(|n| Value::from(n as u64))
            .unwrap_or_else(|| value.clone()),
        (ValueType::Named(name), Value::Object(object)) => {
            let Some(desc) = db.get(name) else {
                return value.clone();
            };
            let mut out = object.clone();
            for field in desc.fields() {
                let v = object
                    .get(&field.name)
                    .cloned()
                    .unwrap_or_else(|| db.field_default(field));
                out.insert(field.name.clone(), materialize(db, &field.ty, &v));
            }
            Value::Object(out)
        }
        (ValueType::Option(inner), v) if !v.is_null() => materialize(db, inner, v),
        (ValueType::List(inner) | ValueType::Keyed(inner), Value::Array(items)) => {
            Value::Array(items.iter().map(|v| materialize(db, inner, v)).collect())
        }
        (ValueType::Map(inner), Value::Object(items)) => Value::Object(
            items
                .iter()
                .map(|(k, v)| (k.clone(), materialize(db, inner, v)))
                .collect(),
        ),
        _ => value.clone(),
    }
}
