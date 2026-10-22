//! Registration from Rust: the [`Schema`] trait (implemented by `#[derive(Schema)]`)
//! and [`FieldType`], which maps Rust field types to [`ValueType`]s.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{SchemaDb, TypeDesc, ValueType};

/// A Rust type registered in the Schema DB. Derive it; don't implement it by hand.
///
/// ```ignore
/// /// Hit points of a damageable entity.
/// #[derive(Schema)]
/// #[ara(component)]
/// pub struct Health {
///     /// Maximum hit points.
///     #[ara(default = 100, range(min = 1, max = 10000))]
///     pub max: f32,
/// }
/// db.register::<Health>();
/// ```
pub trait Schema: 'static {
    /// Describes the type.
    fn describe() -> TypeDesc;
    /// Registers the types its fields refer to.
    fn register_deps(db: &mut SchemaDb);
}

/// A Rust type usable as a reflected field.
pub trait FieldType {
    /// The schema type of the field.
    fn value_type() -> ValueType;
    /// Registers any named types the field refers to.
    fn register(db: &mut SchemaDb) {
        let _ = db;
    }
}

macro_rules! primitive {
    ($($rust:ty => $variant:ident),* $(,)?) => {
        $(impl FieldType for $rust {
            fn value_type() -> ValueType {
                ValueType::$variant
            }
        })*
    };
}

primitive! {
    bool => Bool, i32 => I32, i64 => I64, u8 => U8, u16 => U16, u32 => U32, u64 => U64,
    f32 => F32, f64 => F64, String => String, [f32; 2] => Vec2, [f32; 3] => Vec3,
    [f32; 4] => Vec4, serde_json::Value => Json,
    EntityRef => Entity, Color => Color, Quat => Quat,
}

impl FieldType for AssetRef {
    fn value_type() -> ValueType {
        ValueType::Asset(None)
    }
}

impl<T: FieldType> FieldType for Option<T> {
    fn value_type() -> ValueType {
        ValueType::Option(Box::new(T::value_type()))
    }
    fn register(db: &mut SchemaDb) {
        T::register(db);
    }
}

impl<T: FieldType> FieldType for Vec<T> {
    fn value_type() -> ValueType {
        ValueType::List(Box::new(T::value_type()))
    }
    fn register(db: &mut SchemaDb) {
        T::register(db);
    }
}

impl<T: FieldType> FieldType for BTreeMap<String, T> {
    fn value_type() -> ValueType {
        ValueType::Map(Box::new(T::value_type()))
    }
    fn register(db: &mut SchemaDb) {
        T::register(db);
    }
}

/// An entity in the same document: a record ID or entity address (`e_4f9a/m4tz`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityRef(pub String);

/// An `@path#id` asset reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetRef(pub String);

/// A linear RGBA color; defaults to opaque white.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub [f32; 4]);

/// A rotation quaternion `[x, y, z, w]`; defaults to identity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Quat(pub [f32; 4]);
