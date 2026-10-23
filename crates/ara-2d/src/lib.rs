#![warn(missing_docs)]
//! Reflected 2D authoring and pure extraction inputs, independent of the GPU.
pub mod commands;
mod sprite;
pub mod tilemap;
use ara_schema::SchemaDb;
pub use sprite::*;
pub use tilemap::{GridLayout, Tile, TileDefinition, TileLayer, TileRule, Tilemap};

/// Registers the full 2D component/asset vocabulary in the shared Schema DB.
pub fn register(db: &mut SchemaDb) {
    db.register::<Sprite>()
        .register::<Camera2d>()
        .register::<Light2d>()
        .register::<Flipbook>()
        .register::<Tilemap>()
        .register::<Atlas>();
}

/// Installs typed ECS bridges before constructing edit/play worlds.
pub fn register_sim(registry: &mut ara_sim::SimRegistry) -> Result<(), ara_sim::SimError> {
    registry.register_component::<Sprite>()?;
    registry.register_component::<Camera2d>()?;
    registry.register_component::<Light2d>()?;
    registry.register_component::<Flipbook>()?;
    registry.register_component::<Tilemap>()
}

/// Writes a reflected value with schema order and omitted defaults (A3).
pub fn canonical<T: ara_schema::Schema + serde::Serialize>(value: &T) -> anyhow::Result<String> {
    let mut db = SchemaDb::new();
    db.register::<T>();
    let value = db.canonical_value(
        &ara_schema::ValueType::Named(T::describe().name),
        &serde_json::to_value(value)?,
    );
    Ok(ara_json::to_canonical_string(&value))
}

/// Writes an unreflected manifest/report with recursively sorted canonical JSON keys.
pub fn canonical_json(value: &serde_json::Value) -> String {
    ara_json::to_canonical_string(&ara_json::sort_keys(value))
}
