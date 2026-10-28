//! `register_modules`: the reverse direction of the generator. C# sends the descriptors
//! of one atomic reload (a module, or a module and its dependents in dependency order);
//! each registers its types in the Schema DB as dynamic types, has its C# layouts checked
//! against the canonical layouts, and registers (or reuses, or migrates) the bevy
//! components that hold its data.
//!
//! Registration is all or nothing: every descriptor is validated against the view the
//! earlier ones in the batch produce ([`Registry::prepare`]) before any of them touches
//! the world ([`Registry::commit`]). On error nothing changes and every module keeps its
//! last good generation.

use std::alloc::Layout;
use std::collections::BTreeSet;

use ara_diag::Diagnostic;
use ara_schema::{SchemaDb, StructLayout, type_from_json};
use bevy_ecs::component::{ComponentCloneBehavior, ComponentDescriptor, ComponentId, StorageType};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::Deserialize;
use serde_json::Value;

use super::registry::{
    BehaviourDecl, CommandDecl, ModuleRecord, Owner, Registry, ScriptComponent, ScriptData,
    SystemDecl, convert, insert_resource_bytes, resource_bytes,
};
use crate::codes;
use crate::ecs::cmdbuf;
use crate::value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    module: String,
    slot: u32,
    // Removed from the shape before parsing (see `prepare`); set from the removed value.
    #[serde(default)]
    generation: u32,
    hot: bool,
    types: Value,
    components: Vec<LayoutDecl>,
    resources: Vec<LayoutDecl>,
    cues: Vec<LayoutDecl>,
    systems: Vec<SystemDesc>,
    behaviours: Vec<BehaviourDesc>,
    commands: Vec<CommandDesc>,
}

#[derive(Deserialize)]
struct LayoutDecl {
    name: String,
    size: usize,
    offsets: Vec<usize>,
}

#[derive(Deserialize)]
struct Access {
    name: String,
    write: bool,
}

#[derive(Deserialize)]
struct SystemDesc {
    name: String,
    order: i32,
    columns: Vec<Access>,
    resources: Vec<Access>,
    with: Vec<String>,
    without: Vec<String>,
    #[serde(default)]
    structural: bool,
}

#[derive(Deserialize)]
struct BehaviourDesc {
    name: String,
    data: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandDesc {
    name: String,
    summary: String,
    params: String,
    params_layout: LayoutDecl,
    reads: Vec<String>,
    writes: Vec<String>,
    #[serde(default)]
    expose: Option<Value>,
    #[serde(default)]
    examples: Vec<Value>,
}

/// One validated descriptor, ready to commit.
enum Prepared {
    /// Same declarations as the running generation (an L3 that only changed code).
    Same {
        slot: u32,
        generation: u32,
        hot: bool,
    },
    /// New declarations, checked against the batch's schema and the components before it.
    Changed(Box<Checked>),
}

struct Checked {
    shape: Value,
    d: Descriptor,
    type_names: Vec<String>,
    components: Vec<(String, StructLayout)>,
    resources: Vec<(String, StructLayout)>,
    cues: Vec<(String, StructLayout)>,
    command_layouts: Vec<StructLayout>,
}

/// Names declared by earlier descriptors of the batch: (name, owner slot).
#[derive(Default)]
struct Pending {
    components: Vec<(String, u32)>,
    resources: Vec<(String, u32)>,
}

fn error(code: ara_diag::Code, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, msg)
}

/// Checks a C# layout against the canonical one. Empty (tag) structs are 1 byte in C#.
fn check_layout(db: &SchemaDb, d: &LayoutDecl) -> Result<StructLayout, Diagnostic> {
    let canonical = db
        .struct_layout(&d.name)
        .map_err(|e| error(codes::NOT_BLITTABLE, e).record(&d.name))?;
    let offsets: Vec<usize> = canonical.fields.iter().map(|f| f.offset).collect();
    let tag = canonical.size == 0 && d.size == 1 && d.offsets.is_empty();
    if !tag && (d.size != canonical.size || d.offsets != offsets) {
        return Err(error(
            codes::LAYOUT_MISMATCH,
            format!(
                "`{}`: C# layout (size {}, offsets {:?}) differs from the canonical layout (size {}, offsets {:?}); \
                 declare fields in schema order with blittable types",
                d.name, d.size, d.offsets, canonical.size, offsets
            ),
        )
        .record(&d.name));
    }
    Layout::from_size_align(canonical.size, canonical.align)
        .map_err(|e| error(codes::LAYOUT_MISMATCH, format!("`{}`: {e}", d.name)))?;
    Ok(canonical)
}

/// Registers a plain-data component (no drop) with the canonical layout.
fn register_bevy_component(world: &mut World, name: &str, layout: &StructLayout) -> ComponentId {
    // `check_layout` already validated size and alignment; the fallback is unreachable.
    let l = Layout::from_size_align(layout.size, layout.align).unwrap_or(Layout::new::<u8>());
    // SAFETY: script components are plain data (blittable fields only, no drop glue),
    // so a layout with no drop function describes them fully.
    let desc = unsafe {
        ComponentDescriptor::new_with_layout(
            format!("script::{name}"),
            StorageType::Table,
            l,
            None,
            true,
            false,
            ComponentCloneBehavior::Default,
            None,
        )
    };
    world.register_component_with_descriptor(desc)
}

/// Moves every entity's `old` component into the new component `id` (new layout).
fn migrate(
    world: &mut World,
    schema: &SchemaDb,
    old: &ScriptComponent,
    id: ComponentId,
    layout: &StructLayout,
) {
    // Every archetype with the old component, including disabled entities (a query would
    // apply the default `Disabled` filter).
    let entities: Vec<Entity> = world
        .archetypes()
        .iter()
        .filter(|a| a.contains(old.id))
        .flat_map(|a| a.entities().iter().map(|e| e.id()))
        .collect();
    for e in entities {
        let Some(bytes) = cmdbuf::component_bytes(world, e, old.id, old.layout.size) else {
            continue;
        };
        let next = convert(schema, &old.layout, layout, &bytes);
        cmdbuf::insert_bytes(world, e, id, &next);
        if let Ok(mut em) = world.get_entity_mut(e) {
            em.remove_by_id(old.id);
        }
    }
}

impl Registry {
    /// Registers one atomic reload: a JSON array of module descriptors in dependency
    /// order (a single module is a batch of one). On error nothing changes.
    pub fn register_modules(
        &mut self,
        world: &mut World,
        json: &[u8],
    ) -> Result<Value, Vec<Diagnostic>> {
        let batch: Vec<Value> = serde_json::from_slice(json).map_err(|e| {
            vec![error(
                codes::BAD_DESCRIPTOR,
                format!("module descriptors: {e}"),
            )]
        })?;
        let mut schema = self.schema.clone();
        let mut pending = Pending::default();
        let mut prepared = Vec::with_capacity(batch.len());
        for shape in batch {
            prepared.push(self.prepare(&mut schema, &mut pending, shape)?);
        }
        for p in prepared {
            self.commit(world, &schema, p);
        }
        self.schema = schema;
        Ok(self.response())
    }

    /// Validates one descriptor against `schema` (the batch so far) and the components
    /// earlier descriptors declare; adds its types to `schema`.
    fn prepare(
        &self,
        schema: &mut SchemaDb,
        pending: &mut Pending,
        mut shape: Value,
    ) -> Result<Prepared, Vec<Diagnostic>> {
        let generation = shape
            .as_object_mut()
            .and_then(|o| o.remove("generation"))
            .and_then(|g| g.as_u64())
            .and_then(|g| u32::try_from(g).ok());
        let slot = shape["slot"]
            .as_u64()
            .and_then(|s| u32::try_from(s).ok())
            .unwrap_or(u32::MAX);
        // Fast path: an L3 that changed only code declares exactly what the running generation did.
        if let Some(prev) = self.modules.get(&slot)
            && prev.shape == shape
        {
            let hot = shape["hot"].as_bool().unwrap_or(prev.hot);
            return Ok(Prepared::Same {
                slot,
                generation: generation.unwrap_or(prev.generation),
                hot,
            });
        }
        let mut d: Descriptor = serde_json::from_value(shape.clone()).map_err(|e| {
            vec![error(
                codes::BAD_DESCRIPTOR,
                format!("module descriptor: {e}"),
            )]
        })?;
        d.generation = generation.unwrap_or(0);
        let owner = Owner::Module(d.slot);
        let mut problems = Vec::new();

        // 1. Schema: drop this module's previous types, add the new ones, verify.
        if let Some(prev) = self.modules.get(&d.slot) {
            for t in &prev.types {
                schema.remove(t);
            }
        }
        let mut type_names = Vec::new();
        if let Some(types) = d.types.get("types").and_then(Value::as_object) {
            for (name, def) in types {
                match type_from_json(name, def) {
                    Ok(mut desc) => {
                        desc.module = d.module.clone();
                        // Initial scene validation may already hold this exact
                        // module's published schema. Adopt it when the assembly
                        // supplies the matching runtime registration and layout.
                        if !self.modules.contains_key(&d.slot)
                            && let Some(published) = schema.get(name)
                            && published.module == d.module
                            && schema.type_json(published) == schema.type_json(&desc)
                        {
                            schema.remove(name);
                        }
                        match schema.add(desc) {
                            Ok(()) => type_names.push(name.clone()),
                            Err(e) => problems.push(e.record(name)),
                        }
                    }
                    Err(e) => problems.push(e),
                }
            }
        }
        problems.extend(schema.verify().into_iter().filter(Diagnostic::is_error));
        if !problems.is_empty() {
            return Err(problems);
        }

        // 2. Layouts, and names other modules (registered or earlier in the batch) own.
        let checked =
            |decls: &[LayoutDecl], problems: &mut Vec<Diagnostic>| -> Vec<(String, StructLayout)> {
                decls
                    .iter()
                    .filter_map(|l| match check_layout(schema, l) {
                        Ok(c) => Some((l.name.clone(), c)),
                        Err(e) => {
                            problems.push(e);
                            None
                        }
                    })
                    .collect()
            };
        let components = checked(&d.components, &mut problems);
        let resources = checked(&d.resources, &mut problems);
        let cues = checked(&d.cues, &mut problems);
        let taken = |name: &str, registered: Option<Owner>, batch: &[(String, u32)]| {
            registered
                .filter(|o| *o != owner)
                .map(|o| format!("{o:?}"))
                .or_else(|| {
                    batch
                        .iter()
                        .find(|(n, s)| n == name && *s != d.slot)
                        .map(|(_, s)| format!("{:?}", Owner::Module(*s)))
                })
        };
        for (name, _) in &resources {
            let registered = self
                .resources
                .iter()
                .find(|r| &r.name == name)
                .map(|r| r.owner);
            if let Some(by) = taken(name, registered, &pending.resources) {
                problems.push(error(
                    codes::BAD_DESCRIPTOR,
                    format!("resource `{name}` is already declared by {by}"),
                ));
            }
        }
        for (name, _) in &components {
            let registered = self
                .components
                .iter()
                .find(|c| &c.name == name)
                .map(|c| c.owner);
            if let Some(by) = taken(name, registered, &pending.components) {
                problems.push(error(
                    codes::BAD_DESCRIPTOR,
                    format!("component `{name}` is already declared by {by}"),
                ));
            }
        }
        let mut command_layouts = Vec::with_capacity(d.commands.len());
        for c in &d.commands {
            match check_layout(schema, &c.params_layout) {
                Ok(l) => command_layouts.push(l),
                Err(e) => problems.push(e),
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }

        // 3. Resolve names: registered components, earlier modules of the batch, this module's.
        let known_component = |name: &str| {
            self.component(name).is_some()
                || pending.components.iter().any(|(n, _)| n == name)
                || components.iter().any(|(n, _)| n == name)
        };
        let known_resource = |name: &str| {
            self.resource(name).is_some()
                || pending.resources.iter().any(|(n, _)| n == name)
                || resources.iter().any(|(n, _)| n == name)
        };
        for s in &d.systems {
            for a in &s.columns {
                if self
                    .component(&a.name)
                    .is_some_and(|i| self.components[i as usize].json)
                {
                    problems.push(error(codes::BAD_DESCRIPTOR, format!("system `{}` requests a raw column for accessor component `{}`; use World.Get in a behaviour or command",s.name,a.name)));
                }
                if !known_component(&a.name) {
                    problems.push(error(
                        codes::UNKNOWN_COMPONENT,
                        format!("system `{}` uses unknown component `{}`", s.name, a.name),
                    ));
                }
            }
            for a in &s.resources {
                if !known_resource(&a.name) {
                    problems.push(error(
                        codes::UNKNOWN_COMPONENT,
                        format!("system `{}` uses unknown resource `{}`", s.name, a.name),
                    ));
                }
            }
            for n in s.with.iter().chain(&s.without) {
                if !known_component(n) {
                    problems.push(error(
                        codes::UNKNOWN_COMPONENT,
                        format!("system `{}` filters on unknown component `{n}`", s.name),
                    ));
                }
            }
            let mut seen = BTreeSet::new();
            if s.resources.iter().any(|a| !seen.insert(&a.name)) {
                problems.push(error(
                    codes::BAD_DESCRIPTOR,
                    format!("system `{}` takes the same resource twice", s.name),
                ));
            }
            let mut seen = BTreeSet::new();
            if s.columns.iter().any(|a| !seen.insert(&a.name)) {
                problems.push(error(
                    codes::BAD_DESCRIPTOR,
                    format!("system `{}` takes the same component twice", s.name),
                ));
            }
        }
        for b in &d.behaviours {
            if !known_component(&b.data) {
                problems.push(error(
                    codes::UNKNOWN_COMPONENT,
                    format!("behaviour `{}` has no data component `{}`", b.name, b.data),
                ));
            }
        }
        for c in &d.commands {
            for n in c.reads.iter().chain(&c.writes) {
                if !known_component(n) {
                    problems.push(error(
                        codes::UNKNOWN_COMPONENT,
                        format!("command `{}` uses unknown component `{n}`", c.name),
                    ));
                }
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        pending
            .components
            .extend(components.iter().map(|(n, _)| (n.clone(), d.slot)));
        pending
            .resources
            .extend(resources.iter().map(|(n, _)| (n.clone(), d.slot)));
        Ok(Prepared::Changed(Box::new(Checked {
            shape,
            d,
            type_names,
            components,
            resources,
            cues,
            command_layouts,
        })))
    }

    /// Applies one prepared descriptor: components (reuse, migrate or add), resources,
    /// cues and the module record. `schema` is the batch's final schema.
    fn commit(&mut self, world: &mut World, schema: &SchemaDb, p: Prepared) {
        let c = match p {
            Prepared::Same {
                slot,
                generation,
                hot,
            } => {
                if let Some(prev) = self.modules.get_mut(&slot) {
                    prev.generation = generation;
                    prev.hot = hot;
                }
                return;
            }
            Prepared::Changed(c) => *c,
        };
        let Checked {
            shape,
            d,
            type_names,
            components,
            resources,
            cues,
            command_layouts,
        } = c;
        let owner = Owner::Module(d.slot);
        let mut ids_changed = false;
        for (name, layout) in components {
            match self.component(&name) {
                Some(i) if self.components[i as usize].layout == layout => {}
                Some(i) => {
                    let id = register_bevy_component(world, &name, &layout);
                    let old = self.components[i as usize].clone();
                    migrate(world, schema, &old, id, &layout);
                    self.components[i as usize] = ScriptComponent {
                        name,
                        id,
                        layout,
                        owner,
                        json: false,
                    };
                    ids_changed = true;
                }
                None => {
                    let id = register_bevy_component(world, &name, &layout);
                    self.components.push(ScriptComponent {
                        name,
                        id,
                        layout,
                        owner,
                        json: false,
                    });
                    self.catalog_dirty = true;
                }
            }
        }
        for (name, layout) in resources {
            let existing = self.resource(&name);
            if existing.is_some_and(|i| self.resources[i as usize].layout == layout) {
                continue;
            }
            let id = register_bevy_component(world, &name, &layout);
            let bytes = match existing.and_then(|i| {
                let r = &self.resources[i as usize];
                r.id.and_then(|rid| resource_bytes(world, rid, r.layout.size))
                    .map(|b| (r.layout.clone(), b))
            }) {
                Some((old, b)) => convert(schema, &old, &layout, &b),
                None => value::default_bytes(schema, &layout),
            };
            insert_resource_bytes(world, id, &bytes);
            ids_changed = true;
            let rec = ScriptData {
                name,
                id: Some(id),
                layout,
                owner,
            };
            match existing {
                Some(i) => self.resources[i as usize] = rec,
                None => self.resources.push(rec),
            }
        }
        for (name, layout) in cues {
            match self.cues.iter().position(|c| c.name == name) {
                Some(i) => self.cues[i].layout = layout,
                None => self.cues.push(ScriptData {
                    name,
                    id: None,
                    layout,
                    owner,
                }),
            }
        }

        // The module record (ids are `slot << 16 | index`).
        let resolve = |n: &str| self.component(n).unwrap_or(u32::MAX);
        let id = |i: usize| (d.slot << 16) | u32::try_from(i).unwrap_or(0xFFFF);
        let record = ModuleRecord {
            shape,
            name: d.module.clone(),
            generation: d.generation,
            hot: d.hot,
            types: type_names,
            systems: d
                .systems
                .iter()
                .enumerate()
                .map(|(i, s)| SystemDecl {
                    id: id(i),
                    name: s.name.clone(),
                    order: s.order,
                    columns: s
                        .columns
                        .iter()
                        .map(|a| (resolve(&a.name), a.write))
                        .collect(),
                    resources: s
                        .resources
                        .iter()
                        .map(|a| (self.resource(&a.name).unwrap_or(u32::MAX), a.write))
                        .collect(),
                    with: s.with.iter().map(|n| resolve(n)).collect(),
                    without: s.without.iter().map(|n| resolve(n)).collect(),
                    structural: s.structural,
                })
                .collect(),
            behaviours: d
                .behaviours
                .iter()
                .enumerate()
                .map(|(i, b)| BehaviourDecl {
                    id: id(i),
                    name: b.name.clone(),
                    data: resolve(&b.data),
                })
                .collect(),
            commands: d
                .commands
                .iter()
                .zip(command_layouts)
                .enumerate()
                .map(|(i, (c, layout))| CommandDecl {
                    id: id(i),
                    name: c.name.clone(),
                    summary: c.summary.clone(),
                    params: c.params.clone(),
                    params_layout: layout,
                    reads: c.reads.clone(),
                    writes: c.writes.clone(),
                    expose: c.expose.clone(),
                    examples: c.examples.clone(),
                })
                .collect(),
        };
        // An L3 that only changed code keeps the schedule: systems are ids into the C# table.
        let same = self
            .modules
            .get(&d.slot)
            .is_some_and(|prev| prev.same_shape(&record));
        self.dirty |= ids_changed || !same;
        self.catalog_dirty |= ids_changed;
        self.modules.insert(d.slot, record);
    }
}
