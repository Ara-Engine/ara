//! Schema-owned codecs. Unregistered state is rejected instead of silently omitted.

use crate::{BoundaryCommand, DynamicLayout, Result, SimError, dynamic::materialize};
use ara_schema::{Schema, SchemaDb, Validator, ValueType};
use bevy_ecs::{component::ComponentId, prelude::*};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy)]
pub(crate) struct Codec {
    pub check: fn(Value) -> Result<()>,
    pub install: fn(&mut World) -> ComponentId,
    pub read: fn(&World, Entity) -> Result<Option<Value>>,
    pub write: fn(&mut World, Entity, Value) -> Result<()>,
}

#[derive(Clone, Copy)]
pub(crate) struct ResourceCodec {
    pub type_id: fn() -> Option<std::any::TypeId>,
    pub check: fn(Value) -> Result<()>,
    pub install: fn(&mut World) -> ComponentId,
    pub read: fn(&World) -> Result<Option<Value>>,
    pub write: fn(&mut World, Option<Value>) -> Result<()>,
}

type Handler = Arc<dyn Fn(&mut World, &BoundaryCommand) -> Result<()> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct CommandCodec {
    pub direct: Option<std::any::TypeId>,
    pub params: String,
    pub check: fn(Value) -> Result<()>,
    pub run: Handler,
}

/// Startup registry shared by independent worlds; contains no mutable gameplay state.
#[derive(Clone)]
pub struct SimRegistry {
    pub(crate) schema: SchemaDb,
    pub(crate) typed: BTreeMap<String, Codec>,
    pub(crate) resources: BTreeMap<String, ResourceCodec>,
    pub(crate) dynamic: BTreeMap<String, DynamicLayout>,
    pub(crate) external: BTreeSet<String>,
    pub(crate) handlers: BTreeMap<String, CommandCodec>,
    pub(crate) stream_signature: [u8; 32],
}

impl SimRegistry {
    /// Builds runtime layouts from a verified Schema DB and installs the built-in bridge.
    pub fn new(mut schema: SchemaDb) -> Result<Self> {
        schema.register::<ara_components::Transform>();
        let problems = schema.verify();
        if let Some(d) = problems.iter().find(|d| d.is_error()) {
            return Err(SimError(d.message.clone()));
        }
        let dynamic = schema
            .components()
            .map(|t| Ok((t.name.clone(), DynamicLayout::new(t)?)))
            .collect::<Result<_>>()?;
        let mut registry = Self {
            schema,
            typed: BTreeMap::new(),
            resources: BTreeMap::new(),
            dynamic,
            external: BTreeSet::new(),
            handlers: BTreeMap::new(),
            stream_signature: [0; 32],
        };
        registry.register_component::<ara_components::Transform>()?;
        registry.refresh_layouts()?;
        Ok(registry)
    }

    /// Immutable authoring/runtime reflection, also used to compute replay compatibility.
    pub fn schema(&self) -> &SchemaDb {
        &self.schema
    }

    /// Registers a Rust component bridge. Its Serde names must match its Schema names.
    /// Saved references must be stable schema entity references, never Bevy Entity values.
    pub fn register_component<T: Component + Schema + Serialize + DeserializeOwned>(
        &mut self,
    ) -> Result<()> {
        // A schema that already holds `T` (engine schemas do) needs no re-verification.
        let known = self.schema.registers::<T>();
        self.schema.register::<T>();
        let desc = T::describe();
        if !desc.is_component() {
            return Err(SimError(format!("{} is not a component schema", desc.name)));
        }
        if !known {
            self.check_schema()?;
        }
        self.dynamic.remove(&desc.name);
        self.typed.insert(
            desc.name,
            Codec {
                check: |value| {
                    serde_json::from_value::<T>(value)?;
                    Ok(())
                },
                install: World::register_component::<T>,
                read: |world, entity| {
                    world
                        .get::<T>(entity)
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(Into::into)
                },
                write: |world, entity, value| {
                    let component: T = serde_json::from_value(value)?;
                    world
                        .get_entity_mut(entity)
                        .map_err(|e| SimError(e.to_string()))?
                        .insert(component);
                    Ok(())
                },
            },
        );
        if known {
            return Ok(());
        }
        self.refresh_layouts()
    }

    /// Registers restorable infrastructure state. Authoritative gameplay stays in components.
    /// Resource state is hashed too; absence and presence are restored exactly.
    pub fn register_resource<T: Resource + Schema + Serialize + DeserializeOwned>(
        &mut self,
    ) -> Result<()> {
        self.schema.register::<T>();
        self.check_schema()?;
        let name = T::describe().name;
        self.resources.insert(
            name,
            ResourceCodec {
                type_id: || Some(std::any::TypeId::of::<T>()),
                check: |value| {
                    serde_json::from_value::<T>(value)?;
                    Ok(())
                },
                install: World::register_component::<T>,
                read: |world| {
                    world
                        .get_resource::<T>()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(Into::into)
                },
                write: |world, value| {
                    if let Some(value) = value {
                        world.insert_resource(serde_json::from_value::<T>(value)?);
                    } else {
                        world.remove_resource::<T>();
                    }
                    Ok(())
                },
            },
        );
        self.refresh_layouts()
    }

    /// Registers a typed tick-boundary command, including asynchronous result payloads.
    /// The session adapter authenticates player/target permissions before submission.
    pub fn register_command<T: Schema + DeserializeOwned>(
        &mut self,
        name: &str,
        run: fn(&mut World, &BoundaryCommand, T) -> Result<()>,
    ) -> Result<()> {
        if name.is_empty() || self.handlers.contains_key(name) {
            return Err(SimError("empty or duplicate boundary command name".into()));
        }
        self.schema.register::<T>();
        self.check_schema()?;
        self.handlers.insert(
            name.into(),
            CommandCodec {
                direct: None,
                params: T::describe().name,
                check: |value| {
                    serde_json::from_value::<T>(value)?;
                    Ok(())
                },
                run: Arc::new(move |world, command| {
                    let payload = serde_json::from_value::<T>(command.payload.clone())?;
                    run(world, command, payload)
                }),
            },
        );
        self.refresh_layouts()
    }

    /// Registers the same typed feature for direct Local dispatch and encoded replay.
    pub fn register_sim_command<C: crate::SimCommand>(&mut self) -> Result<()> {
        self.register_command::<C>(C::NAME, |world, envelope, value| {
            let context = crate::CommandContext {
                player: envelope.player,
                sequence: envelope.sequence,
                target: envelope.target.clone(),
            };
            value.validate(&context)?;
            value.execute(world, &context)
        })?;
        self.handlers
            .get_mut(C::NAME)
            .expect("the command was just registered")
            .direct = Some(std::any::TypeId::of::<C>());
        Ok(())
    }

    /// Hands a schema component's storage to another runtime (C# modules own their
    /// columns). Documents still validate against its schema type; the projection
    /// queues its values in [`crate::ExternalWrites`] instead of writing an ara-sim
    /// column, and the owner's `StateProvider` captures and restores it.
    pub fn register_external(&mut self, name: &str) -> Result<()> {
        if !self.schema.get(name).is_some_and(|t| t.is_component()) {
            return Err(SimError(format!("{name} is not a component schema")));
        }
        if self.typed.contains_key(name) {
            return Err(SimError(format!("{name} is a typed Rust component")));
        }
        self.dynamic.remove(name);
        self.external.insert(name.to_owned());
        Ok(())
    }

    /// Whether another runtime owns `name`'s storage ([`SimRegistry::register_external`]).
    pub fn is_external(&self, name: &str) -> bool {
        self.external.contains(name)
    }

    /// Layout of a data-only component, absent for typed Rust components.
    pub fn dynamic_layout(&self, name: &str) -> Option<&DynamicLayout> {
        self.dynamic.get(name)
    }

    /// Installs a reflected component's actual ECS storage for a scripting host.
    pub fn install_component(&self, world: &mut World, name: &str) -> Result<ComponentId> {
        if let Some(codec) = self.typed.get(name) {
            return Ok((codec.install)(world));
        }
        self.dynamic
            .get(name)
            .map(|layout| layout.register(world))
            .ok_or_else(|| SimError(format!("unknown component {name}")))
    }

    /// Reads typed or schema-owned ECS storage without retaining a world borrow.
    pub fn read_component(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
        id: ComponentId,
    ) -> Result<Option<Value>> {
        if let Some(codec) = self.typed.get(name) {
            return (codec.read)(world, entity);
        }
        self.dynamic
            .get(name)
            .ok_or_else(|| SimError(format!("unknown component {name}")))?
            .read(world, id, entity)
    }

    /// Writes through the same materialization and codec as scene projection.
    pub fn write_component(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
        id: ComponentId,
        value: &Value,
    ) -> Result<()> {
        self.validate(name, value)?;
        let value = materialize(&self.schema, &ValueType::Named(name.to_owned()), value);
        self.validate(name, &value)?;
        if let Some(codec) = self.typed.get(name) {
            return (codec.write)(world, entity, value);
        }
        self.dynamic
            .get(name)
            .ok_or_else(|| SimError(format!("unknown component {name}")))?
            .insert(world, id, entity, &value)
    }

    pub(crate) fn prepare_command(&self, command: &mut BoundaryCommand) -> Result<()> {
        let codec = self
            .handlers
            .get(&command.kind)
            .ok_or_else(|| SimError(format!("unknown boundary command {}", command.kind)))?;
        self.validate(&codec.params, &command.payload)?;
        let value = materialize(
            &self.schema,
            &ValueType::Named(codec.params.clone()),
            &command.payload,
        );
        (codec.check)(value.clone())?;
        command.payload = value;
        Ok(())
    }

    fn check_schema(&self) -> Result<()> {
        if let Some(d) = self.schema.verify().into_iter().find(|d| d.is_error()) {
            return Err(SimError(d.message));
        }
        Ok(())
    }

    fn refresh_layouts(&mut self) -> Result<()> {
        for desc in self.schema.components() {
            if !self.typed.contains_key(&desc.name)
                && !self.dynamic.contains_key(&desc.name)
                && !self.external.contains(&desc.name)
            {
                self.dynamic
                    .insert(desc.name.clone(), DynamicLayout::new(desc)?);
            }
        }
        self.stream_signature = *blake3::hash(
            ara_json::to_canonical_string(&ara_json::sort_keys(&self.schema.reflection()))
                .as_bytes(),
        )
        .as_bytes();
        Ok(())
    }

    pub(crate) fn install(&self, world: &mut World) -> BTreeMap<String, ComponentId> {
        let mut ids = BTreeMap::new();
        for (name, codec) in &self.typed {
            ids.insert(name.clone(), (codec.install)(world));
        }
        for (name, layout) in &self.dynamic {
            ids.insert(name.clone(), layout.register(world));
        }
        for codec in self.resources.values() {
            (codec.install)(world);
        }
        ids
    }

    pub(crate) fn read(
        &self,
        world: &World,
        ids: &BTreeMap<String, ComponentId>,
        entity: Entity,
        name: &str,
    ) -> Result<Option<Value>> {
        if let Some(codec) = self.typed.get(name) {
            return (codec.read)(world, entity);
        }
        if self.external.contains(name) {
            // Another runtime owns the column: read it through that owner.
            return Ok(None);
        }
        let Some(layout) = self.dynamic.get(name) else {
            return Err(SimError(format!("unknown component {name}")));
        };
        layout.read(world, ids[name], entity)
    }

    pub(crate) fn write(
        &self,
        world: &mut World,
        ids: &BTreeMap<String, ComponentId>,
        entity: Entity,
        name: &str,
        value: &Value,
    ) -> Result<()> {
        self.validate(name, value)?;
        let value = materialize(&self.schema, &ValueType::Named(name.into()), value);
        self.validate(name, &value)?;
        if let Some(codec) = self.typed.get(name) {
            return (codec.write)(world, entity, value);
        }
        let layout = self
            .dynamic
            .get(name)
            .ok_or_else(|| SimError(format!("unknown component {name}")))?;
        layout.insert(world, ids[name], entity, &value)
    }

    pub(crate) fn validate(&self, name: &str, value: &Value) -> Result<()> {
        let mut problems = Vec::new();
        Validator::new(&self.schema, &mut problems).value(
            &ValueType::Named(name.into()),
            None,
            value,
            name,
        );
        if let Some(d) = problems.into_iter().find(|d| d.is_error()) {
            return Err(SimError(d.message));
        }
        let mut overflow = false;
        self.schema.visit(
            &ValueType::Named(name.into()),
            value,
            name,
            &mut |ty, v, _| {
                if *ty == ValueType::F32 {
                    overflow |= v.as_f64().is_some_and(|v| !(v as f32).is_finite());
                }
                if ty.vector_len().is_some()
                    && let Some(values) = v.as_array()
                {
                    overflow |= values
                        .iter()
                        .any(|v| v.as_f64().is_some_and(|v| !(v as f32).is_finite()));
                }
            },
        );
        if overflow {
            return Err(SimError(format!(
                "{name} contains a value outside finite f32 storage"
            )));
        }
        Ok(())
    }
}
