//! Worker validation and explicit tick-boundary cell projection.
use crate::{EditWorld, Identity, Result, SceneMetadata, SimError, SimRegistry};
use ara_doc::Record;
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Portable structural input recorded at the boundary where it took effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamChange {
    /// Spawn a validated authored row under its project-wide stable identity.
    Spawn {
        /// Stable identity, independent of cell path and Bevy handles.
        identity: String,
        /// Materialized component data plus optional name/order/parent metadata.
        record: Record,
    },
    /// Unload a row. Gameplay state must be saved by the host before removal.
    Despawn {
        /// Stable identity to remove.
        identity: String,
    },
}

/// Fully validated row prepared on a worker using the session's immutable registry.
/// Contains owned values; publication moves them into ECS without schema traversal.
pub struct PreparedStreamEntity {
    identity: String,
    record: Record,
    signature: [u8; 32],
    staged_bytes: usize,
}

impl PreparedStreamEntity {
    /// Stable lookup key; moving a row between cells does not change it.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Conservative owned staging footprint, including materialized schema defaults.
    /// Hosts add ECS/mapping and source-buffer reservations before admitting a cell.
    pub fn staged_bytes(&self) -> usize {
        self.staged_bytes
    }
}

impl SimRegistry {
    /// Validates and materializes a cooked row off the main thread. The registry
    /// must match the receiving session; changing schemas requires recooking cells.
    pub fn prepare_stream_entity(
        &self,
        identity: String,
        record: Record,
    ) -> Result<PreparedStreamEntity> {
        ara_doc::RecordId::parse(&identity).map_err(SimError)?;
        let mut complete = Record::new();
        for (name, value) in record {
            if ara_doc::is_component_key(&name) {
                self.validate(&name, &value)?;
                let value = crate::dynamic::materialize(
                    &self.schema,
                    &ara_schema::ValueType::Named(name.clone()),
                    &value,
                );
                self.validate(&name, &value)?;
                if let Some(codec) = self.typed.get(&name) {
                    (codec.check)(value.clone())?;
                }
                complete.insert(name, value);
            } else if matches!(name.as_str(), "name" | "order" | "parent") && value.is_string() {
                complete.insert(name, value);
            } else {
                return Err(SimError(format!(
                    "unsupported streamed field {name}; expand prefabs before cooking"
                )));
            }
        }
        let staged_bytes = std::mem::size_of::<PreparedStreamEntity>()
            + identity.capacity()
            + complete
                .iter()
                .map(|(key, value)| 96 + key.capacity() + value_bytes(value))
                .sum::<usize>();
        Ok(PreparedStreamEntity {
            identity,
            record: complete,
            signature: self.stream_signature,
            staged_bytes,
        })
    }
}

/// Exclusive access supplied only by `Session::step_with_streaming`, before systems.
/// Time/range decisions belong to the host; this hook uses no wall clock or I/O.
pub struct StreamingBoundary<'a> {
    pub(crate) edit: &'a mut EditWorld,
    pub(crate) changes: &'a mut Vec<StreamChange>,
    pub(crate) recording: bool,
}

impl StreamingBoundary<'_> {
    /// Whether an identity is already resident, without scanning the ECS world.
    pub fn contains(&self, identity: &str) -> bool {
        self.edit.entity(identity).is_some()
    }

    /// Inserts one prepared row. Parents must arrive first; cells cook topologically.
    pub fn spawn(&mut self, entity: PreparedStreamEntity) -> Result<()> {
        if entity.signature != self.edit.registry.stream_signature {
            return Err(SimError(
                "stream schema differs from receiving registry".into(),
            ));
        }
        if self.contains(&entity.identity) {
            return Err(SimError(format!(
                "duplicate streamed identity {}",
                entity.identity
            )));
        }
        let parent = entity
            .record
            .get("parent")
            .and_then(Value::as_str)
            .map(|id| {
                self.edit
                    .entity(id)
                    .ok_or_else(|| SimError(format!("stream parent {id} is not resident")))
            })
            .transpose()?;
        if self.recording {
            self.changes.push(StreamChange::Spawn {
                identity: entity.identity.clone(),
                record: entity.record.clone(),
            });
        }
        let PreparedStreamEntity {
            identity,
            mut record,
            signature: _,
            staged_bytes: _,
        } = entity;
        let metadata = SceneMetadata {
            name: record
                .remove("name")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            order: record
                .remove("order")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
        };
        record.remove("parent");
        let handle = self
            .edit
            .world
            .spawn((Identity(identity.to_owned()), metadata))
            .id();
        for (name, value) in record {
            if let Some(codec) = self.edit.registry.typed.get(&name) {
                (codec.write)(&mut self.edit.world, handle, value)?;
            } else if let Some(layout) = self.edit.registry.dynamic.get(&name) {
                layout.insert(&mut self.edit.world, self.edit.ids[&name], handle, &value)?;
            } else {
                return Err(SimError(
                    "stream schema differs from worker registry".into(),
                ));
            }
        }
        if let Some(parent) = parent {
            self.edit.world.entity_mut(handle).insert(ChildOf(parent));
        }
        self.edit.mapping.insert(identity, handle);
        Ok(())
    }

    /// Removes one row without recursive cascading. Unload children before parents.
    pub fn despawn(&mut self, identity: &str) -> Result<()> {
        let Some(handle) = self.edit.entity(identity) else {
            return Ok(());
        };
        if self
            .edit
            .world
            .get::<Children>(handle)
            .is_some_and(|children| !children.is_empty())
        {
            return Err(SimError(
                "stream unload must remove children before their parent".into(),
            ));
        }
        self.edit.mapping.remove(identity);
        self.edit.world.despawn(handle);
        if self.recording {
            self.changes.push(StreamChange::Despawn {
                identity: identity.into(),
            });
        }
        Ok(())
    }

    /// Applies a recorded arrival during deterministic replay; no loading occurs.
    pub fn replay(&mut self, change: StreamChange) -> Result<()> {
        match change {
            StreamChange::Spawn { identity, record } => {
                let prepared = self.edit.registry.prepare_stream_entity(identity, record)?;
                self.spawn(prepared)
            }
            StreamChange::Despawn { identity } => self.despawn(&identity),
        }
    }
}

fn value_bytes(value: &Value) -> usize {
    std::mem::size_of::<Value>()
        + match value {
            Value::String(text) => text.capacity(),
            Value::Array(values) => {
                values.capacity() * std::mem::size_of::<Value>()
                    + values.iter().map(value_bytes).sum::<usize>()
            }
            Value::Object(fields) => fields
                .iter()
                .map(|(key, value)| 96 + key.capacity() + value_bytes(value))
                .sum(),
            _ => 0,
        }
}
