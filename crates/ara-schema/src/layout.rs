//! The canonical C layout of schema types: how a blittable struct sits in an ECS
//! column and in a C# `[StructLayout]` view.
//!
//! Fields are placed in reflection order with C alignment rules, so a Rust
//! `#[repr(C)]` struct and a generated C# struct with the same fields agree byte for
//! byte. Only fixed-size types are blittable:
//!
//! | Schema type | Size / align | C# view |
//! |---|---|---|
//! | `bool`, `u8` | 1 / 1 | `bool`, `byte` |
//! | `u16` | 2 / 2 | `ushort` |
//! | `i32`, `u32`, `f32` | 4 / 4 | `int`, `uint`, `float` |
//! | `i64`, `u64`, `f64` | 8 / 8 | `long`, `ulong`, `double` |
//! | `vec2` / `vec3` | 8 / 4, 12 / 4 | `Vector2`, `Vector3` |
//! | `vec4`, `quat`, `color` | 16 / 4 | `Vector4`, `Quaternion`, `Color` |
//! | `entity` | 8 / 8 | runtime `Entity` handle (index + generation) |
//! | enum | 4 / 4 | variant index as `uint` |
//! | struct | C rules over its fields | nested struct |
//!
//! `string`, `asset`, `json`, `option`, `list`, `keyed` and `map` are not blittable.

use crate::{SchemaDb, Shape, ValueType};

/// Size and alignment of a blittable value, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueLayout {
    /// Size in bytes, a multiple of `align`.
    pub size: usize,
    /// Alignment in bytes (a power of two).
    pub align: usize,
}

/// One field of a [`StructLayout`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayout {
    /// Field name (camelCase, as in the schema).
    pub name: String,
    /// Field type.
    pub ty: ValueType,
    /// Byte offset from the start of the struct.
    pub offset: usize,
    /// Size and alignment of the field's value.
    pub layout: ValueLayout,
}

/// The C layout of a blittable struct type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    /// Type name.
    pub name: String,
    /// Total size including trailing padding.
    pub size: usize,
    /// Alignment: the largest field alignment, at least 1.
    pub align: usize,
    /// Fields in reflection order.
    pub fields: Vec<FieldLayout>,
}

impl StructLayout {
    /// A field by name.
    pub fn field(&self, name: &str) -> Option<&FieldLayout> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// Nesting deeper than this is treated as a cycle.
const MAX_DEPTH: usize = 16;

impl SchemaDb {
    /// The C layout of a value of type `ty`, or why it is not blittable.
    pub fn value_layout(&self, ty: &ValueType) -> Result<ValueLayout, String> {
        self.value_layout_at(ty, 0)
    }

    /// The C layout of struct type `name`: fields in reflection order, each aligned to
    /// its own alignment, the total padded to the struct's alignment. Errors name the
    /// first non-blittable field.
    pub fn struct_layout(&self, name: &str) -> Result<StructLayout, String> {
        self.struct_layout_at(name, 0)
    }

    fn value_layout_at(&self, ty: &ValueType, depth: usize) -> Result<ValueLayout, String> {
        let prim = |size: usize| Ok(ValueLayout { size, align: size });
        let vector = |n: usize| {
            Ok(ValueLayout {
                size: 4 * n,
                align: 4,
            })
        };
        match ty {
            ValueType::Bool | ValueType::U8 => prim(1),
            ValueType::U16 => prim(2),
            ValueType::I32 | ValueType::U32 | ValueType::F32 => prim(4),
            ValueType::I64 | ValueType::U64 | ValueType::F64 | ValueType::Entity => prim(8),
            ValueType::Vec2 => vector(2),
            ValueType::Vec3 => vector(3),
            ValueType::Vec4 | ValueType::Quat | ValueType::Color => vector(4),
            ValueType::Named(n) => match self.get(n).map(|t| &t.shape) {
                Some(Shape::Enum { .. }) => prim(4),
                Some(Shape::Struct { .. }) => {
                    let s = self.struct_layout_at(n, depth + 1)?;
                    Ok(ValueLayout {
                        size: s.size,
                        align: s.align,
                    })
                }
                None => Err(format!("unknown type `{n}`")),
            },
            other => Err(format!("`{other}` is not blittable")),
        }
    }

    fn struct_layout_at(&self, name: &str, depth: usize) -> Result<StructLayout, String> {
        if depth > MAX_DEPTH {
            return Err(format!("`{name}` nests itself"));
        }
        let t = self
            .get(name)
            .ok_or_else(|| format!("unknown type `{name}`"))?;
        let Shape::Struct { fields, .. } = &t.shape else {
            return Err(format!("`{name}` is not a struct"));
        };
        let mut offset = 0usize;
        let mut align = 1usize;
        let mut out = Vec::with_capacity(fields.len());
        for f in fields {
            let layout = self
                .value_layout_at(&f.ty, depth)
                .map_err(|e| format!("`{name}.{}`: {e}", f.name))?;
            offset = offset.next_multiple_of(layout.align);
            align = align.max(layout.align);
            out.push(FieldLayout {
                name: f.name.clone(),
                ty: f.ty.clone(),
                offset,
                layout,
            });
            offset += layout.size;
        }
        Ok(StructLayout {
            name: name.to_owned(),
            size: offset.next_multiple_of(align),
            align,
            fields: out,
        })
    }
}
