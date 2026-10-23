//! Opt-in save-game projection. Complete replay checkpoints remain in `snapshot`.
use crate::{Identity, Result, Session, SimContext, SimError, StateHash, dynamic::materialize};
use ara_schema::ValueType;
use bevy_ecs::{prelude::*, resource::IsResource};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Reflected component/resource names explicitly selected by a persistence policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistenceSelection {
    /// Whole components declared persistent by the save schema.
    pub components: BTreeSet<String>,
    /// Whole service resources declared persistent; gameplay stays in components.
    pub resources: BTreeSet<String>,
}

/// Stable identity/hierarchy plus selected component values, without presentation metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistentEntity {
    /// Stable parent identity. Ancestor anchors are included even with no selected fields.
    pub parent: Option<String>,
    /// Schema-named component values, in deterministic name order.
    pub components: BTreeMap<String, Value>,
}

/// Persistent engine clock/RNG cursor, structurally separate from active input state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistentCursor {
    /// Completed simulation tick.
    pub tick: u64,
    /// Fixed simulation step in seconds.
    pub dt: f64,
    /// Initial deterministic session seed.
    pub seed: u64,
    /// Per-system/per-identity deterministic RNG stream states.
    pub streams: BTreeMap<String, BTreeMap<String, u64>>,
    /// Next deterministic runtime identity counter.
    pub next_identity: u64,
}

/// Save-game state at a tick boundary. Input holds, pending work, network sequence
/// numbers, presentation phase and editor documents are deliberately not saved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentState {
    /// Persistent entities and the ancestors needed to reconstruct their hierarchy.
    pub entities: BTreeMap<String, PersistentEntity>,
    /// Selected infrastructure resource values; explicit None preserves absence.
    pub resources: BTreeMap<String, Option<Value>>,
    /// Tick, fixed dt, deterministic RNG streams and identity allocator.
    /// External adapters must resynchronize after load; active inputs are excluded.
    pub context: PersistentCursor,
}

impl PersistentState {
    /// Fills schema defaults and normalizes scalar storage precision after migration.
    /// Validate values first; capture already has this form from the runtime codecs.
    pub fn materialize(&mut self, schema: &ara_schema::SchemaDb) -> Result<()> {
        for entity in self.entities.values_mut() {
            for (name, value) in &mut entity.components {
                *value = persistent_value(schema, &ValueType::Named(name.clone()), value)?;
            }
        }
        for (name, value) in &mut self.resources {
            if let Some(value) = value {
                *value = persistent_value(schema, &ValueType::Named(name.clone()), value)?;
            }
        }
        Ok(())
    }
    /// Hash of the persistent projection, independent of ECS handles and map order.
    pub fn state_hash(&self) -> Result<StateHash> {
        Ok(StateHash::of_value(&serde_json::to_value(self)?))
    }
}

fn persistent_value(schema: &ara_schema::SchemaDb, ty: &ValueType, value: &Value) -> Result<Value> {
    let mut value = materialize(schema, ty, value);
    match (ty, &mut value) {
        (ValueType::F64, value) => {
            if let Some(number) = value.as_f64() {
                *value = Value::from(number);
            }
        }
        (ValueType::F32, value) => {
            if let Some(number) = value.as_f64() {
                if !(number as f32).is_finite() {
                    return Err(SimError("persistent f32 overflow".into()));
                }
                *value = Value::from(f64::from(number as f32));
            }
        }
        (ty, Value::Array(items)) if ty.vector_len().is_some() => {
            for item in items {
                *item = persistent_value(schema, &ValueType::F32, item)?;
            }
        }
        (ValueType::Named(name), Value::Object(object)) => {
            if let Some(desc) = schema.get(name) {
                for field in desc.fields() {
                    if let Some(value) = object.get_mut(&field.name) {
                        *value = persistent_value(schema, &field.ty, value)?;
                    }
                }
            }
        }
        (ValueType::Option(inner), value) if !value.is_null() => {
            *value = persistent_value(schema, inner, value)?;
        }
        (ValueType::List(inner) | ValueType::Keyed(inner), Value::Array(items)) => {
            for value in items {
                *value = persistent_value(schema, inner, value)?;
            }
        }
        (ValueType::Map(inner), Value::Object(items)) => {
            for value in items.values_mut() {
                *value = persistent_value(schema, inner, value)?;
            }
        }
        _ => {}
    }
    Ok(value)
}

impl Session {
    /// Captures only schema-selected state. Call on a tick boundary, then encode/write
    /// the owned result on a worker. No full snapshot or authoring document copy.
    pub fn persistent_state(&self, selection: &PersistenceSelection) -> Result<PersistentState> {
        self.check_persistence_selection(selection)?;
        if self.fault.is_some() {
            return Err(SimError("cannot save a faulted session".into()));
        }
        let identities = self.persistent_identities()?;
        let mut entities = BTreeMap::new();
        for (id, &entity) in &identities {
            let mut components = BTreeMap::new();
            for name in &selection.components {
                if let Some(value) =
                    self.edit
                        .registry
                        .read(self.world(), &self.edit.ids, entity, name)?
                {
                    self.edit.registry.validate(name, &value)?;
                    components.insert(name.clone(), value);
                }
            }
            if !components.is_empty() {
                entities.insert(
                    id.clone(),
                    PersistentEntity {
                        parent: self.persistent_parent(entity)?,
                        components,
                    },
                );
            }
        }
        // Include hierarchy anchors without copying nonpersistent components.
        let mut parents: Vec<_> = entities.values().filter_map(|e| e.parent.clone()).collect();
        while let Some(id) = parents.pop() {
            if entities.contains_key(&id) {
                continue;
            }
            let entity = identities
                .get(&id)
                .ok_or_else(|| SimError("persistent ancestor has no identity".into()))?;
            let parent = self.persistent_parent(*entity)?;
            if let Some(parent) = &parent {
                parents.push(parent.clone());
            }
            entities.insert(
                id,
                PersistentEntity {
                    parent,
                    components: BTreeMap::new(),
                },
            );
        }
        let mut resources = BTreeMap::new();
        for name in &selection.resources {
            let value = (self.edit.registry.resources[name].read)(self.world())?;
            if let Some(value) = &value {
                self.edit.registry.validate(name, value)?;
            }
            resources.insert(name.clone(), value);
        }
        let context = self.context().persistent_cursor();
        let mut state = PersistentState {
            entities,
            resources,
            context,
        };
        state.materialize(&self.edit.registry.schema)?;
        Ok(state)
    }

    /// Validates all schema/typed codecs before changing ECS rows. Apply only at a
    /// host-approved tick boundary after authoritative save-schema migration.
    /// Surviving identities retain their nonpersistent components; new entities get
    /// only saved state. Network adapters must rebuild baselines after loading.
    pub fn restore_persistent(
        &mut self,
        state: &PersistentState,
        selection: &PersistenceSelection,
    ) -> Result<()> {
        self.check_persistence_selection(selection)?;
        if state.context.dt != self.context().dt || !state.context.dt.is_finite() {
            return Err(SimError("save fixed tick differs from session".into()));
        }
        let mut current = self.persistent_identities()?;
        for (id, row) in &state.entities {
            if row.components.is_empty()
                && !state
                    .entities
                    .values()
                    .any(|e| e.parent.as_deref() == Some(id))
            {
                return Err(SimError(
                    "empty persistent entity is not a hierarchy anchor".into(),
                ));
            }
            if !ara_schema::grammar::is_entity_address(id) {
                return Err(SimError("invalid persistent identity".into()));
            }
            let mut seen = BTreeSet::new();
            let mut ancestor = row.parent.as_deref();
            while let Some(parent) = ancestor {
                if parent == id || !seen.insert(parent) {
                    return Err(SimError("persistent hierarchy cycle".into()));
                }
                ancestor = state
                    .entities
                    .get(parent)
                    .ok_or_else(|| SimError("missing persistent parent".into()))?
                    .parent
                    .as_deref();
            }
            for (name, value) in &row.components {
                if !selection.components.contains(name) {
                    return Err(SimError("save contains an unselected component".into()));
                }
                self.edit.registry.validate(name, value)?;
                if let Some(codec) = self.edit.registry.typed.get(name) {
                    (codec.check)(materialize(
                        &self.edit.registry.schema,
                        &ValueType::Named(name.clone()),
                        value,
                    ))?;
                }
            }
        }
        if state.resources.keys().collect::<BTreeSet<_>>() != selection.resources.iter().collect() {
            return Err(SimError("persistent resource selection differs".into()));
        }
        let mut resources = BTreeMap::new();
        for (name, value) in &state.resources {
            let value = value.as_ref().map(|v| {
                materialize(
                    &self.edit.registry.schema,
                    &ValueType::Named(name.clone()),
                    v,
                )
            });
            if let Some(value) = &value {
                self.edit.registry.validate(name, value)?;
                (self.edit.registry.resources[name].check)(value.clone())?;
            }
            resources.insert(name, value);
        }
        let mut removed = BTreeSet::new();
        for (id, &entity) in &current {
            if !state.entities.contains_key(id)
                && selection.components.iter().any(|n| {
                    self.world()
                        .get_entity(entity)
                        .is_ok_and(|e| e.contains_id(self.edit.ids[n]))
                })
            {
                removed.insert(entity);
            }
        }
        // Detach before despawn so Bevy's linked hierarchy cannot delete excluded children.
        let detach: Vec<_> = self
            .world()
            .iter_entities()
            .filter(|e| {
                e.get::<ChildOf>()
                    .is_some_and(|p| removed.contains(&p.parent()))
                    || state
                        .entities
                        .contains_key(e.get::<Identity>().map_or("", |i| &i.0))
            })
            .map(|e| e.id())
            .collect();
        for entity in detach {
            self.edit.world.entity_mut(entity).remove::<ChildOf>();
        }
        for &entity in &removed {
            self.edit.world.despawn(entity);
        }
        current.retain(|_, entity| !removed.contains(entity));
        for id in state.entities.keys() {
            current
                .entry(id.clone())
                .or_insert_with(|| self.edit.world.spawn(Identity(id.clone())).id());
        }
        for (id, row) in &state.entities {
            let entity = current[id];
            for name in &selection.components {
                if let Some(value) = row.components.get(name) {
                    self.edit.registry.write(
                        &mut self.edit.world,
                        &self.edit.ids,
                        entity,
                        name,
                        value,
                    )?;
                } else {
                    self.edit
                        .world
                        .entity_mut(entity)
                        .remove_by_id(self.edit.ids[name]);
                }
            }
            if let Some(parent) = &row.parent {
                self.edit
                    .world
                    .entity_mut(entity)
                    .insert(ChildOf(current[parent]));
            }
        }
        for (name, value) in resources {
            (self.edit.registry.resources[name].write)(&mut self.edit.world, value)?;
        }
        let context = SimContext::from_persistent(&state.context);
        self.edit.world.insert_resource(context);
        self.fixed.rebase_clock();
        self.pending_inputs.clear();
        self.pending_commands.clear();
        self.pending_typed.clear();
        self.pending_ops.clear();
        self.applied.clear();
        self.sequences.clear();
        self.fault = None;
        self.edit.mapping = current
            .into_iter()
            .filter(|(id, _)| self.edit.resolved.contains_key(id))
            .collect();
        Ok(())
    }

    fn check_persistence_selection(&self, selection: &PersistenceSelection) -> Result<()> {
        if selection
            .components
            .iter()
            .any(|n| !self.edit.ids.contains_key(n))
            || selection
                .resources
                .iter()
                .any(|n| !self.edit.registry.resources.contains_key(n))
        {
            return Err(SimError(
                "unknown persistent component/resource codec".into(),
            ));
        }
        Ok(())
    }
    fn persistent_identities(&self) -> Result<BTreeMap<String, Entity>> {
        let mut identities = BTreeMap::new();
        for entity in self
            .world()
            .iter_entities()
            .filter(|e| !e.contains::<IsResource>())
        {
            if let Some(id) = entity.get::<Identity>()
                && identities.insert(id.0.clone(), entity.id()).is_some()
            {
                return Err(SimError("duplicate stable identity".into()));
            }
            if entity.get::<Identity>().is_none()
                && self.edit.ids.values().any(|id| entity.contains_id(*id))
            {
                return Err(SimError("schema component has no stable identity".into()));
            }
        }
        Ok(identities)
    }
    fn persistent_parent(&self, entity: Entity) -> Result<Option<String>> {
        self.world()
            .get::<ChildOf>(entity)
            .map(|p| {
                self.world()
                    .get::<Identity>(p.parent())
                    .map(|id| id.0.clone())
                    .ok_or_else(|| SimError("parent has no stable identity".into()))
            })
            .transpose()
    }
}
