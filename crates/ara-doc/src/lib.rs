//! Authoring documents: scenes and prefabs as flat maps of records.
//!
//! - [`read`] parses strict JSON, dispatches on `$schema`, normalizes old names and
//!   versions, validates against the Schema DB and repairs `@path#id` references.
//! - [`write`] emits the canonical form; read → write is byte-identical.
//! - [`prefab`] flattens instances (ID-path overrides, removals, nesting).
//! - [`Edit`] / [`Op`] are the only way to change a [`Document`]; ara-ops groups them
//!   into transactions.
//! - [`Workspace`] holds loaded documents and the project's [`AssetIndex`].
//!
//! ```json
//! {
//!   "$schema": "ara://schema/scene/1",
//!   "id": "sc_7kq2m9ab",
//!   "records": {
//!     "e_8b21": { "name": "Crate", "parent": "e_1c0d", "order": "a0", "Transform": { "position": [2, 0, 3] } }
//!   }
//! }
//! ```

mod document;
mod id;
mod kinds;
mod op;
pub mod order;
pub mod prefab;
mod read;
pub mod refs;
pub mod validate;
mod workspace;
mod write;

pub use document::{
    Document, Record, field, is_component_key, order_key, order_of, parent_of, prefab_of,
};
pub use id::{AssetId, IdGen, RecordId};
pub use kinds::{Entity, register};
pub use op::{Edit, Op};
pub use order::{OrderKey, key_between};
pub use read::{ReadResult, read, read_header};
pub use refs::AssetIndex;
pub use workspace::Workspace;
pub use write::{canonical_record, to_value, write};
