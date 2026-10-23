//! The Ara Schema DB: one registry of reflected types that every format, tool and
//! binding derives from (decision A2).
//!
//! - Register Rust types with `#[derive(Schema)]` and [`SchemaDb::register`], or
//!   data-only types from JSON with [`SchemaDb::add_json_types`].
//! - Each field carries its type, default, unit, range, docs, examples,
//!   `since`/`deprecated`, aliases, replication policy and editor hints.
//! - [`SchemaDb::reflection`] emits `reflection.json`; [`SchemaDb::type_json_schema`]
//!   and [`SchemaDb::doc_json_schema`] emit JSON Schema 2020-12.
//! - [`Validator`] checks values; [`SchemaDb::canonical_value`] gives the canonical
//!   (default-free, reflection-ordered) form; [`FieldPath`] addresses data inside
//!   records.
//! - [`SchemaDb::struct_layout`] gives the canonical C layout of blittable structs,
//!   which ECS columns and generated C# bindings share.
//!
//! Type identity is the stable PascalCase name plus aliases for renames; `version` and
//! an upgrade hook carry stored data forward.

mod canon;
mod db;
mod desc;
pub mod grammar;
mod jsonschema;
mod layout;
pub mod path;
mod reflect;
mod traits;
mod ty;
mod validate;

pub use ara_schema_derive::Schema;
pub use db::SchemaDb;
pub use desc::{
    Audience, DocKindDesc, EditorHints, FieldDesc, Interpolate, Range, RecordInfo, Replication,
    Shape, StructRole, TypeDesc, Unit, UpgradeFn, VariantDesc,
};
pub use jsonschema::DIALECT;
pub use layout::{FieldLayout, StructLayout, ValueLayout};
pub use path::{FieldPath, Resolved, Segment, Step};
pub use reflect::{REFLECTION_SCHEMA, TYPES_SCHEMA, type_from_json};
pub use traits::{AssetRef, Color, EntityRef, FieldType, Quat, Schema};
pub use ty::ValueType;
pub use validate::Validator;

/// Re-exports used by `#[derive(Schema)]` expansions. Not a public API.
#[doc(hidden)]
pub mod __private {
    pub use serde_json::Value;
}
