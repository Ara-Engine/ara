//! Gameplay commands declared in C#: the catalog agents and tools see, execution as
//! one undoable step, and undo.
//!
//! A command runs with exclusive world access. Every component and resource it opens
//! for writing is journaled (first-seen bytes); on success the writes and the
//! structural ops it buffered become a [`CommandRecord`] with inverses, on rejection or
//! fault everything it wrote is restored, so a rejected command leaves the world
//! unchanged.

use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use ara_schema::StructLayout;
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::Mut;
use bevy_ecs::world::World;
use serde_json::{Map, Value, json};

use super::ScriptHost;
use super::ctx::Journal;
use super::registry::{self, ScriptComponent, ScriptData};
use crate::abi::{CommandCall, RUN_BAD_ID, RUN_DISABLED, RUN_OK, RUN_REJECTED};
use crate::codes;
use crate::ecs::cmdbuf::{self, Applied, CmdBuf, ScriptCatalog};
use crate::value;

/// One component value a command changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentWrite {
    /// Entity bits.
    pub entity: u64,
    /// Script component index.
    pub component: u32,
    /// Bytes before the command.
    pub before: Vec<u8>,
    /// Bytes after it.
    pub after: Vec<u8>,
}

/// One resource value a command changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceWrite {
    /// Script resource index.
    pub resource: u32,
    /// Bytes before the command.
    pub before: Vec<u8>,
    /// Bytes after it.
    pub after: Vec<u8>,
}

/// A presentation cue, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// Cue type name (`MedkitUsed`).
    pub name: String,
    /// Its fields.
    pub value: Value,
}

/// An applied command: what it changed (with inverses) and the cues it emitted.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandRecord {
    /// Command name.
    pub command: String,
    /// Params as called.
    pub params: Value,
    /// Component values changed in place.
    pub writes: Vec<ComponentWrite>,
    /// Resource values changed in place.
    pub resources: Vec<ResourceWrite>,
    /// Structural changes, in order.
    pub structural: Vec<Applied>,
    /// Cues emitted.
    pub cues: Vec<Cue>,
    /// Name and layout of every component and resource the record holds bytes for, as
    /// they were when the command ran: undo converts the bytes if a reload migrated a type.
    layouts: Layouts,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Layouts {
    components: BTreeMap<u32, (String, StructLayout)>,
    resources: BTreeMap<u32, (String, StructLayout)>,
}

/// One validated undo step, with bytes already in the current layout.
enum Step {
    /// Undo a spawn.
    Despawn(u64),
    /// Undo a despawn: a new entity with these components; later steps naming the old
    /// entity go to the new one.
    Respawn(u64, Vec<(u32, Vec<u8>)>),
    Set(u64, u32, Vec<u8>),
    Remove(u64, ComponentId),
    Resource(ComponentId, Vec<u8>),
}

impl ScriptHost {
    /// The C# commands in the shape of the command registry's catalog (`reflection.json`
    /// `commands`): summary, params type, scope, examples, plus access and exposure.
    pub fn command_catalog(&self) -> Value {
        let mut out = Map::new();
        for m in self.registry.modules.values() {
            for c in &m.commands {
                out.insert(
                    c.name.clone(),
                    json!({
                        "kind": "command",
                        "summary": c.summary,
                        "params": c.params,
                        "hints": { "idempotent": false, "destructive": false },
                        "scope": "play",
                        "examples": c.examples,
                        "module": m.name,
                        "reads": c.reads,
                        "writes": c.writes,
                        "expose": c.expose,
                    }),
                );
            }
        }
        Value::Object(out)
    }

    /// Runs command `name` with JSON `params` (validated against its params type) as
    /// one step. `caller` is the calling entity, if any. Rejections (`ctx.Reject`) and
    /// faults return an error and leave the world unchanged.
    pub fn execute_command(
        &mut self,
        world: &mut World,
        name: &str,
        params: &Value,
        caller: Option<Entity>,
    ) -> Result<CommandRecord, Diagnostic> {
        let decl = self
            .registry
            .modules
            .values()
            .flat_map(|m| m.commands.iter())
            .find(|c| c.name == name)
            .cloned()
            .ok_or_else(|| {
                let names: Vec<&str> = self
                    .registry
                    .modules
                    .values()
                    .flat_map(|m| m.commands.iter().map(|c| c.name.as_str()))
                    .collect();
                let mut msg = format!("no C# command `{name}`");
                if let Some(s) = ara_diag::did_you_mean(name, names) {
                    msg.push_str(&format!("; did you mean `{s}`?"));
                }
                Diagnostic::error(codes::UNKNOWN_COMMAND, msg)
            })?;
        let mut bytes = value::encode(&self.registry.schema, &decl.params_layout, params, "params")
            .map_err(|d| Diagnostic {
                code: codes::INVALID_PARAMS,
                ..d
            })?;
        if bytes.is_empty() {
            bytes.push(0); // C# reads at least one byte (empty structs are 1 byte)
        }
        let mut journal = Journal::default();
        let mut buf = CmdBuf::default();
        let tick = self.tick;
        let mut call = CommandCall {
            command: decl.id,
            reason_len: 0,
            params: bytes.as_ptr(),
            tick: &tick,
            sequence: self.sequence,
            caller: caller.map_or(value::NULL_ENTITY, Entity::to_bits),
            commands: std::ptr::null_mut(), // set once the buffer is bound to the published world
            reason: [0; 64],
        };
        self.sequence += 1;
        let reported = self.ctx.diagnostic_count();
        let handle = self.ctx.handle();
        let rc = self.exclusive(world, false, Some(&mut journal), |api, cell| {
            // The allocator comes from the published cell, which stays valid for the call.
            buf.reset(cell.entity_allocator());
            call.commands = std::ptr::from_mut(&mut buf).cast();
            // SAFETY: `call` and everything it points at live for the call; world access is published.
            unsafe { (api.run_command)(handle, &mut call) }
        });
        if rc != RUN_OK {
            self.restore(world, &journal);
            cmdbuf::discard(world, &buf.ops);
            let reason =
                String::from_utf8_lossy(&call.reason[..(call.reason_len as usize).min(64)])
                    .into_owned();
            return Err(match rc {
                RUN_REJECTED => Diagnostic::error(
                    codes::COMMAND_REJECTED,
                    format!("`{name}` rejected the call: {reason}"),
                ),
                RUN_BAD_ID => {
                    Diagnostic::error(codes::UNKNOWN_COMMAND, format!("`{name}` is not loaded"))
                }
                RUN_DISABLED => Diagnostic::error(
                    codes::COMMAND_FAULT,
                    format!(
                        "`{name}` is disabled after an earlier fault; it runs again after the next reload"
                    ),
                ),
                _ => {
                    // Only this call's fault; other diagnostics stay queued for their owner.
                    let detail = self
                        .ctx
                        .take_since(reported, codes::COMMAND_FAULT)
                        .into_iter()
                        .map(|d| d.message)
                        .collect::<Vec<_>>()
                        .join("; ");
                    Diagnostic::error(codes::COMMAND_FAULT, format!("`{name}` failed: {detail}"))
                }
            });
        }
        let writes = self.collect_writes(world, &journal);
        let resources = self.collect_resource_writes(world, &journal);
        let mut structural = Vec::new();
        let mut cues = Vec::new();
        let mut problems = Vec::new();
        world.resource_scope(|world, catalog: Mut<ScriptCatalog>| {
            cmdbuf::apply(
                world,
                &catalog,
                &buf.ops,
                &mut cues,
                &mut problems,
                Some(&mut structural),
            );
        });
        for p in problems {
            self.ctx.report(p);
        }
        let layouts = self.layouts(&writes, &resources, &structural);
        Ok(CommandRecord {
            command: name.to_owned(),
            params: params.clone(),
            writes,
            resources,
            structural,
            cues: cues.iter().filter_map(|c| self.decode_cue(c)).collect(),
            layouts,
        })
    }

    fn collect_writes(&self, world: &World, journal: &Journal) -> Vec<ComponentWrite> {
        journal
            .before
            .iter()
            .filter_map(|(entity, component, before)| {
                self.registry.components.get(*component as usize)?;
                let after = world.resource::<ScriptCatalog>().read(
                    world,
                    Entity::try_from_bits(*entity)?,
                    *component,
                )?;
                (after != *before).then(|| ComponentWrite {
                    entity: *entity,
                    component: *component,
                    before: before.clone(),
                    after,
                })
            })
            .collect()
    }

    fn collect_resource_writes(&self, world: &World, journal: &Journal) -> Vec<ResourceWrite> {
        journal
            .resources
            .iter()
            .filter_map(|(resource, before)| {
                let r = self.registry.resources.get(*resource as usize)?;
                let after = registry::resource_bytes(world, r.id?, r.layout.size)?;
                (after != *before).then(|| ResourceWrite {
                    resource: *resource,
                    before: before.clone(),
                    after,
                })
            })
            .collect()
    }

    fn layouts(
        &self,
        writes: &[ComponentWrite],
        resources: &[ResourceWrite],
        structural: &[Applied],
    ) -> Layouts {
        let mut out = Layouts::default();
        let mut component = |i: u32| {
            if let Some(c) = self.registry.components.get(i as usize) {
                out.components
                    .entry(i)
                    .or_insert_with(|| (c.name.clone(), c.layout.clone()));
            }
        };
        writes.iter().for_each(|w| component(w.component));
        for a in structural {
            match a {
                Applied::Spawn { .. } => {}
                Applied::Despawn { components, .. } => {
                    components.iter().for_each(|(i, _)| component(*i));
                }
                Applied::Insert { component: i, .. } | Applied::Remove { component: i, .. } => {
                    component(*i);
                }
            }
        }
        for w in resources {
            if let Some(r) = self.registry.resources.get(w.resource as usize) {
                out.resources
                    .insert(w.resource, (r.name.clone(), r.layout.clone()));
            }
        }
        out
    }

    fn restore(&self, world: &mut World, journal: &Journal) {
        for (entity, component, before) in journal.before.iter().rev() {
            if let (Some(e), Some(_)) = (
                Entity::try_from_bits(*entity),
                self.registry.components.get(*component as usize),
            ) {
                world.resource_scope(|world, catalog: Mut<ScriptCatalog>| {
                    catalog.insert(world, e, *component, before);
                });
            }
        }
        for (resource, before) in &journal.resources {
            if let Some(id) = self
                .registry
                .resources
                .get(*resource as usize)
                .and_then(|r| r.id)
            {
                registry::insert_resource_bytes(world, id, before);
            }
        }
    }

    /// Undoes a command: structural changes in reverse, then component and resource
    /// values. A despawned entity comes back as a new entity with its saved components
    /// (later steps naming it follow it there). Every step is checked before any is
    /// applied, so a failed undo leaves the world unchanged.
    pub fn undo(&self, world: &mut World, record: &CommandRecord) -> Result<(), Diagnostic> {
        let steps = self.undo_steps(world, record).map_err(|what| {
            Diagnostic::error(
                codes::UNDO_FAILED,
                format!("undo `{}`: {what}", record.command),
            )
        })?;
        let mut remap: BTreeMap<u64, Entity> = BTreeMap::new();
        let entity = |bits: u64, remap: &BTreeMap<u64, Entity>| {
            remap
                .get(&bits)
                .copied()
                .or_else(|| Entity::try_from_bits(bits))
        };
        for step in steps {
            match step {
                Step::Despawn(bits) => {
                    if let Some(e) = entity(bits, &remap) {
                        world.despawn(e);
                    }
                }
                Step::Respawn(bits, components) => {
                    let e = world.spawn_empty().id();
                    for (i, bytes) in components {
                        world.resource_scope(|world, catalog: Mut<ScriptCatalog>| {
                            catalog.insert(world, e, i, &bytes);
                        });
                    }
                    remap.insert(bits, e);
                }
                Step::Set(bits, i, bytes) => {
                    if let Some(e) = entity(bits, &remap) {
                        world.resource_scope(|world, catalog: Mut<ScriptCatalog>| {
                            catalog.insert(world, e, i, &bytes);
                        });
                    }
                }
                Step::Remove(bits, id) => {
                    if let Some(mut em) =
                        entity(bits, &remap).and_then(|e| world.get_entity_mut(e).ok())
                    {
                        em.remove_by_id(id);
                    }
                }
                Step::Resource(id, bytes) => registry::insert_resource_bytes(world, id, &bytes),
            }
        }
        Ok(())
    }

    /// The undo steps in order, with every entity, type and layout checked against the
    /// current world (a dry run tracks which entities each step leaves alive).
    fn undo_steps(&self, world: &World, record: &CommandRecord) -> Result<Vec<Step>, String> {
        let schema = &self.registry.schema;
        let component = |i: u32, bytes: &[u8]| -> Result<(u32, Vec<u8>), String> {
            let (name, then) = record
                .layouts
                .components
                .get(&i)
                .ok_or_else(|| format!("component #{i} has no saved layout"))?;
            let now: &ScriptComponent =
                self.registry
                    .components
                    .iter()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| format!("component `{name}` no longer exists"))?;
            let index = self
                .registry
                .component(name)
                .ok_or_else(|| format!("component `{name}` disappeared"))?;
            Ok((
                index,
                if now.json {
                    bytes.to_vec()
                } else {
                    convert(schema, then, &now.layout, bytes)
                },
            ))
        };
        let resource = |i: u32, bytes: &[u8]| -> Result<(ComponentId, Vec<u8>), String> {
            let (name, then) = record
                .layouts
                .resources
                .get(&i)
                .ok_or_else(|| format!("resource #{i} has no saved layout"))?;
            let now: &ScriptData = self
                .registry
                .resources
                .iter()
                .find(|r| &r.name == name)
                .ok_or_else(|| format!("resource `{name}` no longer exists"))?;
            let id = now
                .id
                .ok_or_else(|| format!("resource `{name}` has no storage"))?;
            Ok((id, convert(schema, then, &now.layout, bytes)))
        };
        // Entities whose liveness an earlier step changed.
        let mut alive: BTreeMap<u64, bool> = BTreeMap::new();
        let live = |bits: u64, alive: &BTreeMap<u64, bool>| -> Result<(), String> {
            let ok = alive.get(&bits).copied().unwrap_or_else(|| {
                Entity::try_from_bits(bits).is_some_and(|e| world.get_entity(e).is_ok())
            });
            ok.then_some(())
                .ok_or_else(|| format!("entity {} is gone", value::entity_json(bits)))
        };
        let mut steps = Vec::new();
        for a in record.structural.iter().rev() {
            match a {
                Applied::Spawn { entity } => {
                    live(*entity, &alive)?;
                    alive.insert(*entity, false);
                    steps.push(Step::Despawn(*entity));
                }
                Applied::Despawn { entity, components } => {
                    let components = components
                        .iter()
                        .map(|(i, b)| component(*i, b))
                        .collect::<Result<_, _>>()?;
                    alive.insert(*entity, true);
                    steps.push(Step::Respawn(*entity, components));
                }
                Applied::Insert {
                    entity,
                    component: i,
                    previous,
                } => {
                    live(*entity, &alive)?;
                    steps.push(match previous {
                        Some(bytes) => {
                            let (id, bytes) = component(*i, bytes)?;
                            Step::Set(*entity, id, bytes)
                        }
                        None => Step::Remove(
                            *entity,
                            self.registry.components[component(*i, &[])?.0 as usize].id,
                        ),
                    });
                }
                Applied::Remove {
                    entity,
                    component: i,
                    bytes,
                } => {
                    live(*entity, &alive)?;
                    let (id, bytes) = component(*i, bytes)?;
                    steps.push(Step::Set(*entity, id, bytes));
                }
            }
        }
        for w in record.writes.iter().rev() {
            live(w.entity, &alive)?;
            let (id, bytes) = component(w.component, &w.before)?;
            steps.push(Step::Set(w.entity, id, bytes));
        }
        for w in &record.resources {
            let (id, bytes) = resource(w.resource, &w.before)?;
            steps.push(Step::Resource(id, bytes));
        }
        Ok(steps)
    }

    /// A record as JSON for review: one op per change, with values decoded.
    pub fn describe(&self, record: &CommandRecord) -> Value {
        let comp = |i: u32| self.registry.components.get(i as usize);
        let decode = |i: u32, b: &[u8]| {
            comp(i).map_or(Value::Null, |c| {
                if c.json {
                    serde_json::from_slice(b).unwrap_or(Value::Null)
                } else {
                    value::decode(&self.registry.schema, &c.layout, b)
                }
            })
        };
        let name = |i: u32| comp(i).map_or(Value::Null, |c| Value::String(c.name.clone()));
        let res = |i: u32| self.registry.resources.get(i as usize);
        let mut ops: Vec<Value> = record
            .writes
            .iter()
            .map(|w| json!({ "op": "set", "entity": value::entity_json(w.entity), "component": name(w.component),
                "before": decode(w.component, &w.before), "after": decode(w.component, &w.after) }))
            .collect();
        ops.extend(record.resources.iter().map(|w| {
            let decode = |b: &[u8]| {
                res(w.resource).map_or(Value::Null, |r| {
                    value::decode(&self.registry.schema, &r.layout, b)
                })
            };
            json!({ "op": "set_resource", "resource": res(w.resource).map(|r| r.name.clone()),
                "before": decode(&w.before), "after": decode(&w.after) })
        }));
        ops.extend(record.structural.iter().map(|a| match a {
            Applied::Spawn { entity } => json!({ "op": "spawn", "entity": value::entity_json(*entity) }),
            Applied::Despawn { entity, .. } => json!({ "op": "despawn", "entity": value::entity_json(*entity) }),
            Applied::Insert { entity, component, .. } => json!({ "op": "insert", "entity": value::entity_json(*entity), "component": name(*component) }),
            Applied::Remove { entity, component, .. } => json!({ "op": "remove", "entity": value::entity_json(*entity), "component": name(*component) }),
        }));
        json!({ "command": record.command, "params": record.params, "ops": ops,
            "cues": record.cues.iter().map(|c| json!({ "cue": c.name, "value": c.value })).collect::<Vec<_>>() })
    }
}

/// `bytes` (laid out as `then`) in layout `now`; a copy when they match.
fn convert(
    schema: &ara_schema::SchemaDb,
    then: &StructLayout,
    now: &StructLayout,
    bytes: &[u8],
) -> Vec<u8> {
    if then == now {
        bytes.to_vec()
    } else {
        registry::convert(schema, then, now, bytes)
    }
}
