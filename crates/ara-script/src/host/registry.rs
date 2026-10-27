//! What C# modules declared, as the host sees it: script components, resources and
//! cues (by script index), and each module's systems, behaviours and commands.
//!
//! Registration (C# descriptors in, script indexes out) lives in `register.rs`.

use std::collections::BTreeMap;

use ara_schema::{SchemaDb, StructLayout};
use bevy_ecs::component::ComponentId;
use bevy_ecs::resource::IsResource;
use bevy_ecs::world::World;
use serde_json::{Map, Value, json};

use crate::ecs::cmdbuf::{self, ScriptCatalog};
use crate::value;

/// Who declared a script type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A Rust engine component bound at startup.
    Engine,
    /// A C# module (slot).
    Module(u32),
}

/// A component C# can view in place.
#[derive(Debug, Clone)]
pub(crate) struct ScriptComponent {
    pub name: String,
    pub id: ComponentId,
    pub layout: StructLayout,
    pub owner: Owner,
    pub json: bool,
}

/// A resource or cue declared by C#.
#[derive(Debug, Clone)]
pub(crate) struct ScriptData {
    pub name: String,
    /// The resource's component id (cues have none).
    pub id: Option<ComponentId>,
    pub layout: StructLayout,
    pub owner: Owner,
}

/// One C# system, by script indexes.
#[derive(Debug, Clone)]
pub(crate) struct SystemDecl {
    pub id: u32,
    pub name: String,
    pub order: i32,
    pub columns: Vec<(u32, bool)>,
    pub resources: Vec<(u32, bool)>,
    pub with: Vec<u32>,
    pub without: Vec<u32>,
    /// The system takes `ref Commands` (structural changes).
    pub structural: bool,
}

/// One behaviour type.
#[derive(Debug, Clone)]
pub(crate) struct BehaviourDecl {
    pub id: u32,
    pub name: String,
    pub data: u32,
}

/// One command.
#[derive(Debug, Clone)]
pub(crate) struct CommandDecl {
    pub id: u32,
    pub name: String,
    pub summary: String,
    pub params: String,
    pub params_layout: StructLayout,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub expose: Option<Value>,
    pub examples: Vec<Value>,
}

/// One module's current generation.
#[derive(Debug, Clone)]
pub(crate) struct ModuleRecord {
    /// The descriptor without its generation: equal shapes need no re-registration.
    pub shape: Value,
    pub name: String,
    pub generation: u32,
    pub hot: bool,
    pub types: Vec<String>,
    pub systems: Vec<SystemDecl>,
    pub behaviours: Vec<BehaviourDecl>,
    pub commands: Vec<CommandDecl>,
}

impl ModuleRecord {
    /// True if both generations schedule the same systems and behaviours with the same access.
    pub(crate) fn same_shape(&self, other: &ModuleRecord) -> bool {
        let systems = |m: &ModuleRecord| -> Vec<_> {
            m.systems
                .iter()
                .map(|s| {
                    (
                        s.id,
                        s.name.clone(),
                        s.order,
                        s.columns.clone(),
                        s.resources.clone(),
                        s.with.clone(),
                        s.without.clone(),
                        s.structural,
                    )
                })
                .collect()
        };
        let behaviours =
            |m: &ModuleRecord| -> Vec<_> { m.behaviours.iter().map(|b| (b.id, b.data)).collect() };
        systems(self) == systems(other) && behaviours(self) == behaviours(other)
    }
}

/// Everything registered by the engine and the C# modules.
pub(crate) struct Registry {
    pub json: Option<std::sync::Arc<ara_sim::SimRegistry>>,
    pub schema: SchemaDb,
    pub components: Vec<ScriptComponent>,
    pub resources: Vec<ScriptData>,
    pub cues: Vec<ScriptData>,
    pub modules: BTreeMap<u32, ModuleRecord>,
    /// Systems, behaviours or the components they use changed since the schedule was built.
    pub dirty: bool,
    /// Components were added or moved to new ids since the world's catalog was written.
    pub catalog_dirty: bool,
}

/// Re-lays `bytes` from `old` to `new`: fields with the same name and type keep their
/// value, the rest take defaults.
pub(crate) fn convert(
    db: &SchemaDb,
    old: &StructLayout,
    new: &StructLayout,
    bytes: &[u8],
) -> Vec<u8> {
    let mut out = value::default_bytes(db, new);
    for nf in &new.fields {
        if let Some(of) = old
            .field(&nf.name)
            .filter(|of| of.ty == nf.ty && of.layout == nf.layout)
            && let Some(src) = bytes.get(of.offset..of.offset + of.layout.size)
        {
            out[nf.offset..nf.offset + nf.layout.size].copy_from_slice(src);
        }
    }
    out
}

impl Registry {
    pub fn new(schema: SchemaDb) -> Self {
        Registry {
            json: None,
            schema,
            components: Vec::new(),
            resources: Vec::new(),
            cues: Vec::new(),
            modules: BTreeMap::new(),
            dirty: true,
            catalog_dirty: true,
        }
    }

    /// Index of a component by schema name.
    pub fn component(&self, name: &str) -> Option<u32> {
        self.components
            .iter()
            .position(|c| c.name == name)
            .and_then(|i| u32::try_from(i).ok())
    }

    pub(crate) fn resource(&self, name: &str) -> Option<u32> {
        self.resources
            .iter()
            .position(|c| c.name == name)
            .and_then(|i| u32::try_from(i).ok())
    }

    /// The catalog the command-buffer code reads from the world.
    pub fn catalog(&self) -> ScriptCatalog {
        ScriptCatalog {
            json: self.json.clone(),
            names: self
                .components
                .iter()
                .map(|c| c.json.then(|| c.name.clone()))
                .collect(),
            components: self
                .components
                .iter()
                .map(|c| (c.id, c.layout.size))
                .collect(),
        }
    }
    pub(crate) fn response(&self) -> Value {
        let index = |items: Vec<(String, usize)>| -> Value {
            Value::Object(
                items
                    .into_iter()
                    .map(|(n, i)| (n, json!(i)))
                    .collect::<Map<_, _>>(),
            )
        };
        json!({
            "ok": true,
            "components": index(self.components.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect()),
            "resources": index(self.resources.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect()),
            "cues": index(self.cues.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect()),
        })
    }
}

/// The bytes of a dynamic resource, if present.
pub(crate) fn resource_bytes(world: &World, id: ComponentId, size: usize) -> Option<Vec<u8>> {
    let ptr = world.get_resource_by_id(id)?;
    // SAFETY: the resource's storage holds `size` initialized bytes (its layout size).
    Some(unsafe { std::slice::from_raw_parts(ptr.as_ptr(), size) }.to_vec())
}

/// Inserts (or replaces) a dynamic resource's value. Resources are entities in bevy
/// 0.20: the `IsResource` marker's hook makes the entity findable by resource id.
pub(crate) fn insert_resource_bytes(world: &mut World, id: ComponentId, bytes: &[u8]) {
    let entity = match world.resource_entities().get(id) {
        Some(e) => e,
        None => world.spawn(IsResource::new(id)).id(),
    };
    cmdbuf::insert_bytes(world, entity, id, bytes);
}
