//! Gameplay commands declared in C#: the catalog agents and tools see, execution as
//! one undoable step, and undo.
//!
//! A command runs with exclusive world access. Every component it opens for writing is
//! journaled (first-seen bytes); on success the writes and the structural ops it
//! buffered become a [`CommandRecord`] with inverses, on rejection or fault everything
//! it wrote is restored, so a rejected command leaves the world unchanged.

use ara_diag::Diagnostic;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::Mut;
use bevy_ecs::world::World;
use serde_json::{Map, Value, json};

use super::ScriptHost;
use super::ctx::Journal;
use crate::abi::{CommandCall, RUN_BAD_ID, RUN_OK, RUN_REJECTED};
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
    /// Structural changes, in order.
    pub structural: Vec<Applied>,
    /// Cues emitted.
    pub cues: Vec<Cue>,
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
        buf.reset(world.entity_allocator());
        let tick = self.tick;
        let mut call = CommandCall {
            command: decl.id,
            reason_len: 0,
            params: bytes.as_ptr(),
            tick: &tick,
            sequence: self.sequence,
            caller: caller.map_or(value::NULL_ENTITY, Entity::to_bits),
            commands: std::ptr::from_mut(&mut buf).cast(),
            reason: [0; 64],
        };
        self.sequence += 1;
        let rc = self.exclusive(world, false, Some(&mut journal), |api| {
            // SAFETY: `call` and everything it points at live for the call; world access is published.
            unsafe { (api.run_command)(&mut call) }
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
                _ => {
                    let detail = self
                        .take_diagnostics()
                        .into_iter()
                        .map(|d| d.message)
                        .collect::<Vec<_>>()
                        .join("; ");
                    Diagnostic::error(codes::COMMAND_FAULT, format!("`{name}` failed: {detail}"))
                }
            });
        }
        let writes = self.collect_writes(world, &journal);
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
        Ok(CommandRecord {
            command: name.to_owned(),
            params: params.clone(),
            writes,
            structural,
            cues: cues.iter().filter_map(|c| self.decode_cue(c)).collect(),
        })
    }

    fn collect_writes(&self, world: &World, journal: &Journal) -> Vec<ComponentWrite> {
        journal
            .before
            .iter()
            .filter_map(|(entity, component, before)| {
                let c = self.registry.components.get(*component as usize)?;
                let after = cmdbuf::component_bytes(
                    world,
                    Entity::try_from_bits(*entity)?,
                    c.id,
                    c.layout.size,
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

    fn restore(&self, world: &mut World, journal: &Journal) {
        for (entity, component, before) in journal.before.iter().rev() {
            if let (Some(e), Some(c)) = (
                Entity::try_from_bits(*entity),
                self.registry.components.get(*component as usize),
            ) {
                cmdbuf::insert_bytes(world, e, c.id, before);
            }
        }
    }

    /// Undoes a command: structural changes in reverse, then component values. A
    /// despawned entity comes back as a new entity with its saved components.
    pub fn undo(&self, world: &mut World, record: &CommandRecord) -> Result<(), Diagnostic> {
        let comp = |i: u32| self.registry.components.get(i as usize).map(|c| c.id);
        let failed = |what: String| {
            Diagnostic::error(
                codes::UNDO_FAILED,
                format!("undo `{}`: {what}", record.command),
            )
        };
        let entity =
            |bits: u64| Entity::try_from_bits(bits).ok_or_else(|| failed("invalid entity".into()));
        for a in record.structural.iter().rev() {
            match a {
                Applied::Spawn { entity: e } => {
                    if !world.despawn(entity(*e)?) {
                        return Err(failed(format!(
                            "spawned entity {} is gone",
                            value::entity_json(*e)
                        )));
                    }
                }
                Applied::Despawn { components, .. } => {
                    let e = world.spawn_empty().id();
                    for (i, bytes) in components {
                        if let Some(id) = comp(*i) {
                            cmdbuf::insert_bytes(world, e, id, bytes);
                        }
                    }
                }
                Applied::Insert {
                    entity: e,
                    component,
                    previous,
                } => {
                    let (e, id) = (
                        entity(*e)?,
                        comp(*component).ok_or_else(|| failed("unknown component".into()))?,
                    );
                    match previous {
                        Some(bytes) => {
                            cmdbuf::insert_bytes(world, e, id, bytes);
                        }
                        None => {
                            if let Ok(mut em) = world.get_entity_mut(e) {
                                em.remove_by_id(id);
                            }
                        }
                    }
                }
                Applied::Remove {
                    entity: e,
                    component,
                    bytes,
                } => {
                    let id = comp(*component).ok_or_else(|| failed("unknown component".into()))?;
                    if !cmdbuf::insert_bytes(world, entity(*e)?, id, bytes) {
                        return Err(failed(format!("entity {} is gone", value::entity_json(*e))));
                    }
                }
            }
        }
        for w in record.writes.iter().rev() {
            let id = comp(w.component).ok_or_else(|| failed("unknown component".into()))?;
            if !cmdbuf::insert_bytes(world, entity(w.entity)?, id, &w.before) {
                return Err(failed(format!(
                    "entity {} is gone",
                    value::entity_json(w.entity)
                )));
            }
        }
        Ok(())
    }

    /// A record as JSON for review: one op per change, with values decoded.
    pub fn describe(&self, record: &CommandRecord) -> Value {
        let comp = |i: u32| self.registry.components.get(i as usize);
        let decode = |i: u32, b: &[u8]| {
            comp(i).map_or(Value::Null, |c| {
                value::decode(&self.registry.schema, &c.layout, b)
            })
        };
        let name = |i: u32| comp(i).map_or(Value::Null, |c| Value::String(c.name.clone()));
        let mut ops: Vec<Value> = record
            .writes
            .iter()
            .map(|w| json!({ "op": "set", "entity": value::entity_json(w.entity), "component": name(w.component),
                "before": decode(w.component, &w.before), "after": decode(w.component, &w.after) }))
            .collect();
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
