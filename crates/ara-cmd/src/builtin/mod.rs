//! The starter command set.
//!
//! | Name | Kind |
//! |---|---|
//! | `entity.create`, `entity.delete`, `entity.reparent` | command |
//! | `component.add`, `component.remove` | command |
//! | `property.set`, `transform.set` | command |
//! | `prefab.instantiate`, `prefab.override.set`, `prefab.override.revert` | command |
//! | `entity.inspect` | query |
//!
//! Entities are addressed by record ID or entity address (`e_4f9a/e_m4tz`); edits to
//! prefab instances and the entities inside them become overrides automatically.

mod component;
mod entity;
mod prefab;

use ara_schema::SchemaDb;

pub use component::{
    Changed, ComponentAdd, ComponentAddParams, ComponentRemove, ComponentRemoveParams, PropertySet,
    PropertySetParams, TransformSet, TransformSetParams,
};
pub use entity::{
    Created, Deleted, EntityCreate, EntityCreateParams, EntityDelete, EntityDeleteParams,
    EntityInspect, EntityInspectParams, EntityReparent, EntityReparentParams, Moved,
};
pub use prefab::{
    OverrideRevert, OverrideRevertParams, OverrideSet, OverrideSetParams, PrefabInstantiate,
    PrefabInstantiateParams, Reverted,
};

use crate::Registry;

/// Registers the starter set and its params types.
pub fn register(reg: &mut Registry, db: &mut SchemaDb) {
    reg.command::<EntityCreate>(db)
        .command::<EntityDelete>(db)
        .command::<EntityReparent>(db)
        .command::<ComponentAdd>(db)
        .command::<ComponentRemove>(db)
        .command::<PropertySet>(db)
        .command::<TransformSet>(db)
        .command::<PrefabInstantiate>(db)
        .command::<OverrideSet>(db)
        .command::<OverrideRevert>(db)
        .query::<EntityInspect>(db);
}
