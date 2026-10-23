use crate::{
    BoundaryCommand, EditWorld, FixedTick, Identity, Result, SceneMetadata, Session, SessionMode,
    SimContext, SimError, TickRecord,
};
use ara_doc::{AssetId, Workspace};
use ara_input::{ActionMap, Injection, PlayerId};
use bevy_ecs::{entity_disabling::DefaultQueryFilters, prelude::*, resource::IsResource};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// BLAKE3 digest over canonical schema values with exact IEEE float bits, independent
/// of Bevy handles/table order. Map keys are sorted before binary encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateHash(
    /// Digest bytes, displayed as lowercase hexadecimal.
    pub [u8; 32],
);

impl StateHash {
    /// Hashes content bytes, such as a build manifest.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub(crate) fn of_value(value: &Value) -> Self {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&ara_json::sort_keys(value), &mut bytes)
            .expect("a JSON value can always serialize to an in-memory CBOR buffer");
        Self::of_bytes(&bytes)
    }
}

impl std::fmt::Display for StateHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EntityState {
    pub id: String,
    pub metadata: Option<SceneMetadata>,
    pub parent: Option<String>,
    pub components: BTreeMap<String, Value>,
}

type ResourceStates = BTreeMap<String, Option<Value>>;

/// Complete registered simulation state plus pending boundary work and host accumulator.
/// Capturing rejects unregistered components/resources and entities without identities.
/// Restore preserves the World itself so Bevy query/system caches remain valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub(crate) format: u32,
    pub(crate) schema_hash: StateHash,
    pub(crate) build_hash: StateHash,
    pub(crate) scene_hash: StateHash,
    pub(crate) scene: AssetId,
    pub(crate) documents: BTreeMap<String, crate::source::SourceDocument>,
    pub(crate) entities: Vec<EntityState>,
    pub(crate) resources: BTreeMap<String, Option<Value>>,
    pub(crate) context: SimContext,
    pub(crate) fixed: FixedTick,
    pub(crate) actions: ActionMap,
    pub(crate) inputs: BTreeMap<u64, Vec<Injection>>,
    pub(crate) commands: BTreeMap<u64, Vec<BoundaryCommand>>,
    pub(crate) ops: BTreeMap<u64, Vec<ara_doc::Op>>,
    pub(crate) sequences: BTreeMap<PlayerId, u64>,
    pub(crate) mode: SessionMode,
    pub(crate) systems: Vec<String>,
}

impl Snapshot {
    /// Encodes a binary checkpoint, preserving signed zero and finite float bits.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        encode(self)
    }
    /// Decodes a binary checkpoint; restore validates build/schema/state contracts.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        decode(bytes)
    }
    /// The tick represented by this checkpoint.
    pub fn tick(&self) -> u64 {
        self.context.tick
    }
    /// Compatibility hash of all reflected types.
    pub fn schema_hash(&self) -> StateHash {
        self.schema_hash
    }
}

/// Portable replay artifact. Systems are supplied by the matching engine build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Replay {
    /// Replay format version.
    pub format: u32,
    /// Canonical initial scene/dependency digest.
    pub scene_hash: StateHash,
    /// Initial session seed.
    pub seed: u64,
    /// Caller-supplied binary/module build manifest digest.
    pub build_hash: StateHash,
    /// Reflected schema compatibility digest.
    pub schema_hash: StateHash,
    /// Complete initial simulation checkpoint, including RNG and input holds.
    pub initial: Snapshot,
    /// Applied input/command/authoring log with hashes after every tick.
    pub ticks: Vec<TickRecord>,
}

impl Replay {
    /// Encodes a replay with bit-preserving binary checkpoints and input values.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        encode(self)
    }
    /// Decodes a replay; PlayControl::replay verifies compatibility and every tick.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        decode(bytes)
    }
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).map_err(|e| SimError(e.to_string()))?;
    Ok(bytes)
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut cursor = std::io::Cursor::new(bytes);
    let result = ciborium::de::from_reader(&mut cursor).map_err(|e| SimError(e.to_string()))?;
    if cursor.position() != bytes.len() as u64 {
        return Err(SimError("trailing checkpoint/replay data".into()));
    }
    Ok(result)
}

impl Session {
    /// Hashes schema values, hierarchy, explicit resources, RNG and active inputs.
    /// Pending external arrivals and host interpolation are excluded from a tick hash.
    pub fn state_hash(&self) -> Result<StateHash> {
        if let Some(error) = &self.fault {
            return Err(SimError(format!("cannot hash a faulted session: {error}")));
        }
        let (entities, resources) = self.capture_world()?;
        Ok(StateHash::of_value(&serde_json::json!({
            "entities": entities, "resources": resources, "context": self.context(), "sequences": self.sequences, "systems": self.systems,
        })))
    }

    /// Captures a complete checkpoint. `build_hash` includes all simulation modules.
    pub fn snapshot(&self, build_hash: StateHash) -> Result<Snapshot> {
        if let Some(error) = &self.fault {
            return Err(SimError(format!(
                "cannot snapshot a faulted session: {error}"
            )));
        }
        let (entities, resources) = self.capture_world()?;
        let documents: BTreeMap<_, _> = self
            .edit
            .workspace
            .docs()
            .map(|(path, doc)| (path.into(), crate::source::SourceDocument::capture(doc)))
            .collect();
        let scene_hash =
            crate::source::scene_hash(&self.edit.workspace, &self.edit.registry.schema)?;
        let mut commands = self.pending_commands.clone();
        for (&tick, queued) in &self.pending_typed {
            for command in queued {
                let mut record = command.record()?;
                self.edit.registry.prepare_command(&mut record)?;
                commands.entry(tick).or_default().push(record);
            }
        }
        Ok(Snapshot {
            format: 1,
            schema_hash: self.schema_hash(),
            build_hash,
            scene_hash,
            scene: self.edit.scene.clone(),
            documents,
            entities,
            resources,
            context: self.context().clone(),
            fixed: self.fixed.clone(),
            actions: self.actions.clone(),
            inputs: self.pending_inputs.clone(),
            commands,
            ops: self.pending_ops.clone(),
            sequences: self.sequences.clone(),
            mode: self.mode,
            systems: self.systems.clone(),
        })
    }

    /// Restores complete registered state. World handles for surviving identities remain
    /// usable; recreated identities must be resolved again. No raw Entity fields are saved.
    pub fn restore(&mut self, snapshot: &Snapshot, build_hash: StateHash) -> Result<()> {
        if snapshot.format != 1
            || snapshot.schema_hash != self.schema_hash()
            || snapshot.build_hash != build_hash
            || snapshot.systems != self.systems
        {
            return Err(SimError("snapshot format/build/schema mismatch".into()));
        }
        if snapshot.context.dt != 1.0 / f64::from(snapshot.fixed.config().hz) {
            return Err(SimError("snapshot tick configuration mismatch".into()));
        }
        snapshot.fixed.validate()?;
        for (&tick, inputs) in &snapshot.inputs {
            if tick <= snapshot.tick() {
                return Err(SimError("snapshot contains stale queued input".into()));
            }
            for input in inputs {
                snapshot.actions.validate(&input.action, input.value)?;
                if input.hold_ticks == 0 || tick.checked_add(u64::from(input.hold_ticks)).is_none()
                {
                    return Err(SimError("invalid queued input duration".into()));
                }
            }
        }
        for (&tick, commands) in &snapshot.commands {
            if tick <= snapshot.tick() {
                return Err(SimError("snapshot contains stale queued commands".into()));
            }
            for command in commands {
                self.edit.registry.prepare_command(&mut command.clone())?;
            }
        }
        if snapshot.ops.keys().any(|&tick| tick <= snapshot.tick()) {
            return Err(SimError("snapshot contains stale queued ops".into()));
        }
        self.validate_storage()?;
        let mut workspace = Workspace::default();
        for (path, source) in &snapshot.documents {
            workspace.insert(path, source.restore());
        }
        if snapshot.scene_hash != crate::source::scene_hash(&workspace, &self.edit.registry.schema)?
        {
            return Err(SimError("snapshot scene hash mismatch".into()));
        }
        // Validate all rows and typed codecs before mutating the live world.
        let projected = EditWorld::from_workspace(
            self.edit.registry.clone(),
            workspace,
            snapshot.scene.clone(),
        )?;
        let mut wanted = BTreeSet::new();
        let by_id: BTreeMap<_, _> = snapshot
            .entities
            .iter()
            .map(|row| (row.id.as_str(), row))
            .collect();
        for row in &snapshot.entities {
            if !wanted.insert(row.id.as_str()) {
                return Err(SimError("duplicate snapshot entity identity".into()));
            }
            for (name, value) in &row.components {
                self.edit.registry.validate(name, value)?;
                if let Some(codec) = self.edit.registry.typed.get(name) {
                    (codec.check)(crate::dynamic::materialize(
                        &self.edit.registry.schema,
                        &ara_schema::ValueType::Named(name.clone()),
                        value,
                    ))?;
                }
            }
        }
        for row in &snapshot.entities {
            let mut seen = BTreeSet::new();
            let mut parent = row.parent.as_deref();
            while let Some(id) = parent {
                if id == row.id || !seen.insert(id) {
                    return Err(SimError("snapshot hierarchy cycle".into()));
                }
                parent = by_id
                    .get(id)
                    .ok_or_else(|| SimError("snapshot parent is absent".into()))?
                    .parent
                    .as_deref();
            }
        }
        if snapshot.resources.len() != self.edit.registry.resources.len() {
            return Err(SimError("snapshot resource registry mismatch".into()));
        }
        for (name, value) in &snapshot.resources {
            let codec = self
                .edit
                .registry
                .resources
                .get(name)
                .ok_or_else(|| SimError(format!("unknown resource {name}")))?;
            if let Some(v) = value {
                self.edit.registry.validate(name, v)?;
                (codec.check)(v.clone())?;
            }
        }
        let mut current = self.identity_map()?;
        for &entity in current.values() {
            self.edit.world.entity_mut(entity).remove::<ChildOf>();
        }
        for (id, &entity) in &current {
            if !wanted.contains(id.as_str()) {
                self.edit.world.despawn(entity);
            }
        }
        current.retain(|id, _| wanted.contains(id.as_str()));
        for row in &snapshot.entities {
            current
                .entry(row.id.clone())
                .or_insert_with(|| self.edit.world.spawn(Identity(row.id.clone())).id());
        }
        for row in &snapshot.entities {
            let entity = current[&row.id];
            for (name, &id) in &self.edit.ids {
                if let Some(v) = row.components.get(name) {
                    self.edit.registry.write(
                        &mut self.edit.world,
                        &self.edit.ids,
                        entity,
                        name,
                        v,
                    )?;
                } else {
                    self.edit.world.entity_mut(entity).remove_by_id(id);
                }
            }
            if let Some(meta) = &row.metadata {
                self.edit.world.entity_mut(entity).insert(meta.clone());
            } else {
                self.edit.world.entity_mut(entity).remove::<SceneMetadata>();
            }
            if let Some(parent) = &row.parent {
                self.edit
                    .world
                    .entity_mut(entity)
                    .insert(ChildOf(current[parent]));
            }
        }
        for (name, value) in &snapshot.resources {
            (self.edit.registry.resources[name].write)(&mut self.edit.world, value.clone())?;
        }
        self.edit.world.insert_resource(snapshot.context.clone());
        self.fixed = snapshot.fixed.clone();
        self.fixed.rebase_clock();
        self.actions = snapshot.actions.clone();
        self.pending_inputs = snapshot.inputs.clone();
        self.pending_commands = snapshot.commands.clone();
        self.pending_typed.clear();
        self.pending_ops = snapshot.ops.clone();
        self.sequences = snapshot.sequences.clone();
        self.mode = snapshot.mode;
        self.fault = None;
        self.edit.workspace = projected.workspace;
        self.edit.scene = projected.scene;
        self.edit.resolved = projected.resolved;
        self.edit.mapping = current
            .into_iter()
            .filter(|(id, _)| self.edit.resolved.contains_key(id))
            .collect();
        Ok(())
    }

    pub(crate) fn schema_hash(&self) -> StateHash {
        StateHash::of_value(&self.edit.registry.schema.reflection())
    }

    fn identity_map(&self) -> Result<BTreeMap<String, Entity>> {
        let mut out = BTreeMap::new();
        for entity in self
            .world()
            .iter_entities()
            .filter(|entity| !entity.contains::<IsResource>())
        {
            let id = entity.get::<Identity>().ok_or_else(|| {
                SimError(format!("entity {:?} has no stable Identity", entity.id()))
            })?;
            if out.insert(id.0.clone(), entity.id()).is_some() {
                return Err(SimError(format!("duplicate stable identity {}", id.0)));
            }
        }
        Ok(out)
    }

    fn capture_world(&self) -> Result<(Vec<EntityState>, ResourceStates)> {
        self.validate_storage()?;
        let mut entities = Vec::new();
        for (id, entity) in self.identity_map()? {
            let mut components = BTreeMap::new();
            for name in self.edit.ids.keys() {
                if let Some(value) =
                    self.edit
                        .registry
                        .read(self.world(), &self.edit.ids, entity, name)?
                {
                    self.edit.registry.validate(name, &value)?;
                    components.insert(name.clone(), value);
                }
            }
            let parent = self
                .world()
                .get::<ChildOf>(entity)
                .map(|p| {
                    self.world()
                        .get::<Identity>(p.parent())
                        .map(|id| id.0.clone())
                        .ok_or_else(|| SimError("parent has no identity".into()))
                })
                .transpose()?;
            entities.push(EntityState {
                id,
                metadata: self.world().get::<SceneMetadata>(entity).cloned(),
                parent,
                components,
            });
        }
        let resources = self
            .edit
            .registry
            .resources
            .iter()
            .map(|(name, codec)| {
                let value = (codec.read)(self.world())?;
                if let Some(v) = &value {
                    self.edit.registry.validate(name, v)?;
                }
                Ok((name.clone(), value))
            })
            .collect::<Result<_>>()?;
        Ok((entities, resources))
    }

    fn validate_storage(&self) -> Result<()> {
        let mut allowed: BTreeSet<_> = self.edit.ids.values().copied().collect();
        for entity in self.world().iter_entities() {
            let resource = entity
                .get::<IsResource>()
                .map(IsResource::resource_component_id);
            for &id in entity.archetype().components() {
                let info = self
                    .world()
                    .components()
                    .get_info(id)
                    .ok_or_else(|| SimError("missing component metadata".into()))?;
                let ty = info.type_id();
                if let Some(primary) = resource {
                    if id != primary && ty != Some(std::any::TypeId::of::<IsResource>()) {
                        return Err(SimError(
                            "simulation resource entities cannot carry additional components"
                                .into(),
                        ));
                    }
                    continue;
                }
                if ty == Some(std::any::TypeId::of::<Identity>())
                    || ty == Some(std::any::TypeId::of::<SceneMetadata>())
                    || ty == Some(std::any::TypeId::of::<ChildOf>())
                    || ty == Some(std::any::TypeId::of::<Children>())
                {
                    allowed.insert(id);
                }
                if !allowed.contains(&id) {
                    return Err(SimError(format!(
                        "unregistered simulation component {}",
                        info.name()
                    )));
                }
            }
        }
        for (_, info, _) in self.world().iter_resources() {
            let ty = info.type_id();
            // Bevy initializes an empty schedule registry when building our externally
            // owned schedule. It is infrastructure, not an additional gameplay schedule.
            if ty == Some(std::any::TypeId::of::<SimContext>())
                || ty == Some(std::any::TypeId::of::<DefaultQueryFilters>())
                || ty == Some(std::any::TypeId::of::<bevy_ecs::schedule::Schedules>())
            {
                continue;
            }
            if !self.edit.registry.resources.values().any(|codec| {
                // Install happened before the world ran; never register from a shared borrow.
                (codec.type_id)() == ty
            }) {
                return Err(SimError(format!(
                    "unregistered simulation resource {} ({ty:?})",
                    info.name()
                )));
            }
        }
        for (id, info) in self.world().components().iter_registered() {
            if self.world().contains_non_send_by_id(id) {
                return Err(SimError(format!(
                    "unregistered non-send simulation state {}",
                    info.name()
                )));
            }
        }
        Ok(())
    }
}
