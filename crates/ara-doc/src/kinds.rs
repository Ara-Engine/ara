//! The built-in record type (`Entity`) and document kinds (`scene`, `prefab`).

use std::collections::BTreeMap;

use ara_schema::{AssetRef, DocKindDesc, EntityRef, Schema, SchemaDb};
use serde_json::Value;

/// An entity in a scene or prefab. Components are stored inline under their type
/// names; a prefab instance stores `prefab` plus overrides instead.
#[derive(Schema)]
#[ara(record(prefix = "e", components), module = "core")]
#[allow(dead_code)] // A schema declaration; records are handled as JSON.
pub struct Entity {
    /// Display name in the hierarchy.
    pub name: String,
    /// Parent entity, or none for a root. May point inside a prefab instance
    /// (`e_4f9a/m4tz`).
    pub parent: Option<EntityRef>,
    /// Fractional-index key ordering the entity among its siblings.
    pub order: Option<String>,
    /// The prefab this entity instantiates. Instances carry overrides, not components.
    #[ara(asset_kind = "prefab")]
    pub prefab: Option<AssetRef>,
    /// Prefab overrides keyed by `[<id path>/]<field path>`, such as
    /// `m4tz/MeshRenderer.mesh` or `Transform.position`.
    pub overrides: BTreeMap<String, Value>,
    /// Prefab entities (`m4tz`) or components (`m4tz/Light`) removed from this instance.
    pub removed: Vec<String>,
}

/// Registers `Entity` and the `scene` and `prefab` document kinds.
pub fn register(db: &mut SchemaDb) {
    db.register::<Entity>();
    let kinds = [
        DocKindDesc {
            name: "scene".into(),
            version: 1,
            doc: "A level or sub-level: a flat map of entities.".into(),
            id_prefix: "sc".into(),
            extension: "scene".into(),
            records: vec!["Entity".into()],
            single_root: false,
        },
        DocKindDesc {
            name: "prefab".into(),
            version: 1,
            doc: "A reusable entity subtree with exactly one root, instantiated by reference."
                .into(),
            id_prefix: "pf".into(),
            extension: "prefab".into(),
            records: vec!["Entity".into()],
            single_root: true,
        },
    ];
    for kind in kinds {
        // Registering twice is harmless; verify() still sees the original.
        let _ = db.add_kind(kind);
    }
}
