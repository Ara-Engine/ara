//! What C# modules declared, as the host sees it: script components, resources and
//! cues (by script index), and each module's systems, behaviours and commands.
//!
//! `register_module` is the reverse direction of the generator: a module's descriptor
//! (sent by C# at load and at every L3 swap) registers its types in the Schema DB as
//! dynamic types, checks every C# layout against the canonical layout, and registers
//! (or reuses, or migrates) the bevy components that hold the data.

use std::alloc::Layout;
use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use ara_schema::{SchemaDb, StructLayout, type_from_json};
use bevy_ecs::component::{ComponentCloneBehavior, ComponentDescriptor, ComponentId, StorageType};
use bevy_ecs::entity::Entity;
use bevy_ecs::query::QueryBuilder;
use bevy_ecs::resource::IsResource;
use bevy_ecs::world::World;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::codes;
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
    pub name: String,
    pub generation: u32,
    pub hot: bool,
    pub types: Vec<String>,
    pub systems: Vec<SystemDecl>,
    pub behaviours: Vec<BehaviourDecl>,
    pub commands: Vec<CommandDecl>,
}

/// Everything registered by the engine and the C# modules.
pub(crate) struct Registry {
    pub schema: SchemaDb,
    pub components: Vec<ScriptComponent>,
    pub resources: Vec<ScriptData>,
    pub cues: Vec<ScriptData>,
    pub modules: BTreeMap<u32, ModuleRecord>,
    /// Systems, behaviours or commands changed since the schedule was built.
    pub dirty: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    module: String,
    slot: u32,
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
    Ok(canonical)
}

/// Registers a plain-data component (no drop) with the canonical layout.
fn register_bevy_component(world: &mut World, name: &str, layout: &StructLayout) -> Result<ComponentId, Diagnostic> {
    let l = Layout::from_size_align(layout.size, layout.align)
        .map_err(|e| error(codes::LAYOUT_MISMATCH, format!("`{name}`: {e}")))?;
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
    Ok(world.register_component_with_descriptor(desc))
}

/// Field-by-field conversion: fields with the same name and type keep their bytes;
/// the rest take their new defaults.
fn convert(db: &SchemaDb, old: &StructLayout, new: &StructLayout, bytes: &[u8]) -> Vec<u8> {
    let mut out = value::default_bytes(db, new);
    for nf in &new.fields {
        if let Some(of) = old.field(&nf.name).filter(|of| of.ty == nf.ty && of.layout == nf.layout)
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
            schema,
            components: Vec::new(),
            resources: Vec::new(),
            cues: Vec::new(),
            modules: BTreeMap::new(),
            dirty: true,
        }
    }

    /// Index of a component by schema name.
    pub fn component(&self, name: &str) -> Option<u32> {
        self.components.iter().position(|c| c.name == name).and_then(|i| u32::try_from(i).ok())
    }

    fn resource(&self, name: &str) -> Option<u32> {
        self.resources.iter().position(|c| c.name == name).and_then(|i| u32::try_from(i).ok())
    }

    /// The catalog the command-buffer code reads from the world.
    pub fn catalog(&self) -> ScriptCatalog {
        ScriptCatalog {
            components: self.components.iter().map(|c| (c.id, c.layout.size)).collect(),
        }
    }

    /// Registers one module generation. On error nothing changes and the old
    /// generation keeps running.
    pub fn register_module(&mut self, world: &mut World, json: &[u8]) -> Result<Value, Vec<Diagnostic>> {
        let d: Descriptor = serde_json::from_slice(json)
            .map_err(|e| vec![error(codes::BAD_DESCRIPTOR, format!("module descriptor: {e}"))])?;
        let owner = Owner::Module(d.slot);
        let mut problems = Vec::new();

        // 1. Schema: drop this module's previous types, add the new ones, verify.
        let mut schema = self.schema.clone();
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

        // 2. Layouts.
        let checked = |decls: &[LayoutDecl], problems: &mut Vec<Diagnostic>| -> Vec<(String, StructLayout)> {
            decls
                .iter()
                .filter_map(|l| match check_layout(&schema, l) {
                    Ok(c) => Some((l.name.clone(), c)),
                    Err(e) => {
                        problems.push(e);
                        None
                    }
                })
                .collect()
        };
        let comps = checked(&d.components, &mut problems);
        let ress = checked(&d.resources, &mut problems);
        let cues = checked(&d.cues, &mut problems);
        for (name, _) in &ress {
            if let Some(r) = self.resources.iter().find(|r| &r.name == name && r.owner != owner) {
                problems.push(error(
                    codes::BAD_DESCRIPTOR,
                    format!("resource `{name}` is already declared by {:?}", r.owner),
                ));
            }
        }
        for (name, _) in &comps {
            if let Some(c) = self.components.iter().find(|c| &c.name == name && c.owner != owner) {
                problems.push(error(
                    codes::BAD_DESCRIPTOR,
                    format!("component `{name}` is already declared by {:?}", c.owner),
                ));
            }
        }
        let command_layouts: Vec<Option<StructLayout>> = d
            .commands
            .iter()
            .map(|c| match check_layout(&schema, &c.params_layout) {
                Ok(l) => Some(l),
                Err(e) => {
                    problems.push(e);
                    None
                }
            })
            .collect();
        if !problems.is_empty() {
            return Err(problems);
        }

        // 3. Resolve names: existing components plus this module's.
        let known_component = |name: &str| self.component(name).is_some() || comps.iter().any(|(n, _)| n == name);
        let known_resource = |name: &str| self.resource(name).is_some() || ress.iter().any(|(n, _)| n == name);
        for s in &d.systems {
            for a in &s.columns {
                if !known_component(&a.name) {
                    problems.push(error(codes::UNKNOWN_COMPONENT, format!("system `{}` uses unknown component `{}`", s.name, a.name)));
                }
            }
            for a in &s.resources {
                if !known_resource(&a.name) {
                    problems.push(error(codes::UNKNOWN_COMPONENT, format!("system `{}` uses unknown resource `{}`", s.name, a.name)));
                }
            }
            for n in s.with.iter().chain(&s.without) {
                if !known_component(n) {
                    problems.push(error(codes::UNKNOWN_COMPONENT, format!("system `{}` filters on unknown component `{n}`", s.name)));
                }
            }
            let mut seen = std::collections::BTreeSet::new();
            if s.columns.iter().any(|a| !seen.insert(&a.name)) {
                problems.push(error(codes::BAD_DESCRIPTOR, format!("system `{}` takes the same component twice", s.name)));
            }
        }
        for b in &d.behaviours {
            if !known_component(&b.data) {
                problems.push(error(codes::UNKNOWN_COMPONENT, format!("behaviour `{}` has no data component `{}`", b.name, b.data)));
            }
        }
        for c in &d.commands {
            for n in c.reads.iter().chain(&c.writes) {
                if !known_component(n) {
                    problems.push(error(codes::UNKNOWN_COMPONENT, format!("command `{}` uses unknown component `{n}`", c.name)));
                }
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }

        // 4. Apply: components (reuse, migrate or add), resources, cues.
        for (name, layout) in comps {
            match self.component(&name) {
                Some(i) if self.components[i as usize].layout == layout => {}
                Some(i) => {
                    let id = register_bevy_component(world, &name, &layout).map_err(|e| vec![e])?;
                    let old = self.components[i as usize].clone();
                    migrate(world, &schema, &old, id, &layout);
                    self.components[i as usize] = ScriptComponent { name, id, layout, owner };
                }
                None => {
                    let id = register_bevy_component(world, &name, &layout).map_err(|e| vec![e])?;
                    self.components.push(ScriptComponent { name, id, layout, owner });
                }
            }
        }
        for (name, layout) in ress {
            let existing = self.resource(&name);
            let keep = existing.is_some_and(|i| self.resources[i as usize].layout == layout);
            if keep {
                continue;
            }
            let id = register_bevy_component(world, &name, &layout).map_err(|e| vec![e])?;
            let bytes = match existing.and_then(|i| {
                let r = &self.resources[i as usize];
                r.id.and_then(|rid| resource_bytes(world, rid, r.layout.size)).map(|b| (r.layout.clone(), b))
            }) {
                Some((old, b)) => convert(&schema, &old, &layout, &b),
                None => value::default_bytes(&schema, &layout),
            };
            insert_resource_bytes(world, id, &bytes);
            let rec = ScriptData { name, id: Some(id), layout, owner };
            match existing {
                Some(i) => self.resources[i as usize] = rec,
                None => self.resources.push(rec),
            }
        }
        for (name, layout) in cues {
            match self.cues.iter().position(|c| c.name == name) {
                Some(i) => self.cues[i].layout = layout,
                None => self.cues.push(ScriptData { name, id: None, layout, owner }),
            }
        }

        // 5. The module record (ids are `slot << 16 | index`).
        let resolve = |n: &str| self.component(n).unwrap_or(u32::MAX);
        let id = |i: usize| (d.slot << 16) | u32::try_from(i).unwrap_or(0xFFFF);
        let record = ModuleRecord {
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
                    columns: s.columns.iter().map(|a| (resolve(&a.name), a.write)).collect(),
                    resources: s.resources.iter().map(|a| (self.resource(&a.name).unwrap_or(u32::MAX), a.write)).collect(),
                    with: s.with.iter().map(|n| resolve(n)).collect(),
                    without: s.without.iter().map(|n| resolve(n)).collect(),
                })
                .collect(),
            behaviours: d
                .behaviours
                .iter()
                .enumerate()
                .map(|(i, b)| BehaviourDecl { id: id(i), name: b.name.clone(), data: resolve(&b.data) })
                .collect(),
            commands: d
                .commands
                .iter()
                .zip(command_layouts)
                .enumerate()
                .filter_map(|(i, (c, layout))| {
                    Some(CommandDecl {
                        id: id(i),
                        name: c.name.clone(),
                        summary: c.summary.clone(),
                        params: c.params.clone(),
                        params_layout: layout?,
                        reads: c.reads.clone(),
                        writes: c.writes.clone(),
                        expose: c.expose.clone(),
                        examples: c.examples.clone(),
                    })
                })
                .collect(),
        };
        self.modules.insert(d.slot, record);
        self.schema = schema;
        self.dirty = true;

        let index = |items: Vec<(String, usize)>| -> Value {
            Value::Object(items.into_iter().map(|(n, i)| (n, json!(i))).collect::<Map<_, _>>())
        };
        Ok(json!({
            "ok": true,
            "components": index(self.components.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect()),
            "resources": index(self.resources.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect()),
            "cues": index(self.cues.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect()),
        }))
    }
}

/// Moves every entity's `old` component into the new component `id` (new layout).
fn migrate(world: &mut World, schema: &SchemaDb, old: &ScriptComponent, id: ComponentId, layout: &StructLayout) {
    let mut q = QueryBuilder::<Entity>::new(world).with_id(old.id).build();
    let entities: Vec<Entity> = q.iter(world).collect();
    for e in entities {
        let Some(bytes) = cmdbuf::component_bytes(world, e, old.id, old.layout.size) else { continue };
        let next = convert(schema, &old.layout, layout, &bytes);
        cmdbuf::insert_bytes(world, e, id, &next);
        if let Ok(mut em) = world.get_entity_mut(e) {
            em.remove_by_id(old.id);
        }
    }
}

fn resource_bytes(world: &World, id: ComponentId, size: usize) -> Option<Vec<u8>> {
    let ptr = world.get_resource_by_id(id)?;
    // SAFETY: the resource's storage holds `size` initialized bytes (its layout size).
    Some(unsafe { std::slice::from_raw_parts(ptr.as_ptr(), size) }.to_vec())
}

/// Inserts (or replaces) a dynamic resource's value. Resources are entities in bevy
/// 0.20: the `IsResource` marker's hook makes the entity findable by resource id.
fn insert_resource_bytes(world: &mut World, id: ComponentId, bytes: &[u8]) {
    let entity = match world.resource_entities().get(id) {
        Some(e) => e,
        None => world.spawn(IsResource::new(id)).id(),
    };
    cmdbuf::insert_bytes(world, entity, id, bytes);
}
