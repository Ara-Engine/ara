use crate::{Result, SimError, SimRegistry};
use ara_doc::{
    AssetId, Document, Op, RecordId, Workspace,
    prefab::{Resolved, ResolvedEntity},
};
use bevy_ecs::{component::ComponentId, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Stable document address or deterministic runtime identity. Never reused in a session.
#[derive(Component, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Identity(
    /// Document address or deterministic runtime identity text.
    pub String,
);

/// Authoring metadata alongside the actual ChildOf relationship.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneMetadata {
    /// Display name, with omitted/default names represented as an empty string.
    pub name: String,
    /// Fractional-index sibling order; ties are resolved by stable Identity.
    pub order: String,
}

/// A document's ECS projection, updated by committed ops without rebuilding entities.
pub struct EditWorld {
    pub(crate) world: World,
    pub(crate) registry: SimRegistry,
    pub(crate) ids: BTreeMap<String, ComponentId>,
    pub(crate) workspace: Workspace,
    pub(crate) scene: AssetId,
    pub(crate) mapping: BTreeMap<String, Entity>,
    pub(crate) resolved: Resolved,
}

impl EditWorld {
    /// Projects a scene without external prefab dependencies.
    pub fn new(registry: SimRegistry, scene: Document) -> Result<Self> {
        let id = scene.id.clone();
        let mut workspace = Workspace::default();
        workspace.insert("scene.scene", scene);
        Self::from_workspace(registry, workspace, id)
    }

    /// Resolves loaded prefab dependencies and projects a complete scene.
    pub fn from_workspace(
        registry: SimRegistry,
        workspace: Workspace,
        scene: AssetId,
    ) -> Result<Self> {
        if workspace.doc(scene.as_str()).is_none() {
            return Err(SimError("scene is not loaded".into()));
        }
        let mut world = World::new();
        let ids = registry.install(&mut world);
        let mut this = Self {
            world,
            registry,
            ids,
            workspace,
            scene,
            mapping: BTreeMap::new(),
            resolved: Resolved::new(),
        };
        let rows = this.resolve()?;
        this.sync(rows)?;
        Ok(this)
    }

    /// Read-only ECS access; ordinary authoring edits still go through typed commands.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Runtime entity for a document address, stable across set/move operations.
    pub fn entity(&self, address: &str) -> Option<Entity> {
        self.mapping
            .get(address)
            .copied()
            .filter(|e| self.world.get_entity(*e).is_ok())
    }

    /// Reads the current typed or dynamic component as reflected data.
    pub fn component(&self, address: &str, name: &str) -> Result<Option<Value>> {
        let entity = self
            .entity(address)
            .ok_or_else(|| SimError(format!("unknown entity {address}")))?;
        self.registry.read(&self.world, &self.ids, entity, name)
    }

    /// Direct children in order-key / stable-ID order, independent of ECS iteration.
    pub fn children(&self, parent: Option<&str>) -> Vec<&str> {
        let parent = parent.and_then(|id| self.entity(id));
        let mut rows: Vec<_> = self
            .mapping
            .iter()
            .filter_map(|(id, &entity)| {
                let actual = self.world.get::<ChildOf>(entity).map(ChildOf::parent);
                let meta = self.world.get::<SceneMetadata>(entity)?;
                (actual == parent).then_some((meta.order.as_str(), id.as_str()))
            })
            .collect();
        rows.sort();
        rows.into_iter().map(|(_, id)| id).collect()
    }

    /// Applies a committed ara-ops transaction (including changes to loaded prefabs).
    /// Validation occurs before ECS mutation; unchanged entity handles survive.
    pub fn apply_transaction(&mut self, transaction: &ara_ops::Transaction) -> Result<()> {
        self.apply_ops(&transaction.ops)
    }

    /// Applies the committed op stream. This mirrors ops; it does not author new edits.
    pub fn apply_ops(&mut self, ops: &[Op]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut applied = Vec::new();
        for op in ops {
            let Some(doc) = self.workspace.doc_mut(op.doc().as_str()) else {
                continue;
            };
            match doc.apply(op.to_edit(), &self.registry.schema) {
                Ok(actual) => {
                    if actual != *op {
                        doc.revert(&actual, &self.registry.schema);
                        self.rollback(&applied);
                        return Err(SimError(
                            "op stream does not match the projection baseline".into(),
                        ));
                    }
                    applied.push(actual);
                }
                Err(d) => {
                    self.rollback(&applied);
                    return Err(SimError(d.message));
                }
            }
        }
        let rows = match self.resolve() {
            Ok(rows) => rows,
            Err(e) => {
                self.rollback(&applied);
                return Err(e);
            }
        };
        // Preflight Rust deserialization as well as schema validation before touching ECS.
        if let Err(e) = self.sync(rows) {
            self.rollback(&applied);
            return Err(e);
        }
        Ok(())
    }

    /// Extracts a plain scene for canonical writing. Prefab addresses require an
    /// explicit authoring remap before they can become plain document record IDs.
    /// This is a new snapshot document, never an authoring mutation of the source.
    pub fn to_document(&self) -> Result<Document> {
        let source = self
            .workspace
            .doc(self.scene.as_str())
            .ok_or_else(|| SimError("scene missing".into()))?;
        let mut doc = Document::new(source.kind.clone(), source.id.clone());
        for (address, &entity) in &self.mapping {
            let id = RecordId::parse(address).map_err(|_| {
                SimError("flattened prefab addresses require an explicit authoring remap".into())
            })?;
            let mut record = Map::new();
            if let Some(meta) = self.world.get::<SceneMetadata>(entity) {
                record.insert("name".into(), meta.name.clone().into());
                record.insert("order".into(), meta.order.clone().into());
            }
            if let Some(parent) = self.world.get::<ChildOf>(entity) {
                let id = self
                    .world
                    .get::<Identity>(parent.parent())
                    .ok_or_else(|| SimError("parent has no stable identity".into()))?;
                record.insert("parent".into(), id.0.clone().into());
            }
            for name in self.ids.keys() {
                if let Some(value) = self.registry.read(&self.world, &self.ids, entity, name)? {
                    record.insert(name.clone(), value);
                }
            }
            doc.insert_unchecked(id, record);
        }
        Ok(doc)
    }

    fn rollback(&mut self, ops: &[Op]) {
        for op in ops.iter().rev() {
            if let Some(doc) = self.workspace.doc_mut(op.doc().as_str()) {
                doc.revert(op, &self.registry.schema);
            }
        }
    }

    fn resolve(&self) -> Result<Resolved> {
        let mut diags = self
            .workspace
            .check(&self.scene, None, &self.registry.schema);
        let rows = self
            .workspace
            .resolve(&self.scene, &self.registry.schema, &mut diags);
        if let Some(d) = diags.into_iter().find(|d| d.is_error()) {
            return Err(SimError(d.message));
        }
        Ok(rows)
    }

    fn sync(&mut self, rows: Resolved) -> Result<()> {
        self.mapping
            .retain(|_, entity| self.world.get_entity(*entity).is_ok());
        let changed: BTreeSet<_> = rows
            .keys()
            .chain(self.resolved.keys())
            .filter(|id| rows.get(*id) != self.resolved.get(*id))
            .cloned()
            .collect();
        for id in &changed {
            if let Some(row) = rows.get(id) {
                for (name, value) in &row.record {
                    if ara_doc::is_component_key(name) {
                        self.registry.validate(name, value)?;
                        let complete = crate::dynamic::materialize(
                            &self.registry.schema,
                            &ara_schema::ValueType::Named(name.clone()),
                            value,
                        );
                        self.registry.validate(name, &complete)?;
                        if let Some(codec) = self.registry.typed.get(name) {
                            (codec.check)(complete)?;
                        }
                    }
                }
            }
        }
        for id in &changed {
            if rows.contains_key(id) && !self.mapping.contains_key(id) {
                let entity = self
                    .world
                    .spawn((Identity(id.clone()), SceneMetadata::default()))
                    .id();
                self.mapping.insert(id.clone(), entity);
            }
        }
        // Detach before deletion: Bevy's Children relationship otherwise cascades.
        for id in &changed {
            if let Some(&entity) = self.mapping.get(id) {
                self.world.entity_mut(entity).remove::<ChildOf>();
            }
        }
        for id in &changed {
            let Some(row) = rows.get(id) else { continue };
            let entity = self.mapping[id];
            let previous = self.resolved.get(id);
            for (name, &component_id) in &self.ids {
                let old = previous.and_then(|row| row.record.get(name));
                let new = row.record.get(name);
                if old == new {
                    continue;
                }
                if let Some(value) = new {
                    self.registry
                        .write(&mut self.world, &self.ids, entity, name, value)?;
                } else {
                    self.world.entity_mut(entity).remove_by_id(component_id);
                }
            }
            self.world.entity_mut(entity).insert(metadata(row));
            if let Some(parent) = &row.parent {
                let parent = *self
                    .mapping
                    .get(parent)
                    .ok_or_else(|| SimError(format!("missing parent {parent}")))?;
                self.world.entity_mut(entity).insert(ChildOf(parent));
            }
        }
        for id in &changed {
            if !rows.contains_key(id)
                && let Some(entity) = self.mapping.remove(id)
            {
                self.world.despawn(entity);
            }
        }
        self.resolved = rows;
        Ok(())
    }
}

fn metadata(row: &ResolvedEntity) -> SceneMetadata {
    SceneMetadata {
        name: row
            .record
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        order: row.order.clone().unwrap_or_default(),
    }
}
