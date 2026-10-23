//! C# gameplay in the runtime: [`ScriptPlugin`] runs ara-script modules on the ara-sim
//! world.
//!
//! - `prepare`: starts the .NET host on the empty simulation world, loads the modules
//!   and adds their types to the schema, so scene documents may use C# components.
//! - `register`: C# components become external ara-sim components; the projection
//!   queues their document values and the `scripts` system applies them.
//! - Each tick: `before_tick` applies staged reloads (`ScriptHost::tick_boundary`, the
//!   only place code changes); the `scripts` system runs C# systems and behaviours at
//!   `Stage::Gameplay`, in the host's deterministic `(Order, name)` order.
//! - [`ScriptProvider`] captures C# components (by entity identity) and resources into
//!   snapshots and state hashes, and restores them.
//! - [`execute`], [`catalog`] and [`request_compile`] serve play commands and agent
//!   edits between ticks. Reload reports, diagnostics and L4 restarts arrive on the
//!   [`ScriptEvent`] channel.
use ara_diag::Diagnostic;
use ara_runtime::{
    Result, RuntimeError,
    plugin::{RuntimePlugin, Stage},
};
use ara_schema::SchemaDb;
use ara_script::{
    EditOrigin, EngineBindings, ModuleSpec, ScriptConfig, ScriptHost,
    abi::{ReloadReport, Tick},
    bind_component,
};
use ara_sim::{
    ExternalWrites, Identity, Session, SimContext, SimError, SimRegistry, StateProvider,
};
use bevy_ecs::{component::ComponentId, prelude::*};
use serde_json::{Map, Value, json};
use std::{
    any::TypeId,
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, channel},
};

/// Where the C# host and the game's modules are.
#[derive(Debug, Clone)]
pub struct ScriptSettings {
    /// `Ara.Host` build output for the reloadable JIT host
    /// (`dotnet/artifacts/bin/Ara.Host/release`).
    pub host_dir: PathBuf,
    /// `Ara.CompileServer.dll` for hot reload; `None` loads prebuilt assemblies only.
    pub compile_server: Option<PathBuf>,
    /// Module folders, each with an `ara.module.toml`.
    pub modules: Vec<PathBuf>,
}

/// What the C# host reported, drained by the play process between ticks.
#[derive(Debug, Clone)]
pub enum ScriptEvent {
    /// A reload was applied (or failed) at a tick boundary.
    Reload(ReloadReport),
    /// A host or module diagnostic.
    Diagnostic(Diagnostic),
    /// Engine bindings changed (L4): the last good code keeps running until the play
    /// process restarts from a snapshot.
    RestartRequired,
    /// The modules' reflected types (after load and after each reload), as project
    /// `*.types.json` documents by path (`<module>/<Name>.types.json`).
    Types(BTreeMap<PathBuf, Value>),
}

/// The C# runtime plugin. Create with [`ScriptPlugin::new`] and pass it to
/// `Runtime::new`; keep the receiver to forward [`ScriptEvent`]s.
pub struct ScriptPlugin {
    settings: ScriptSettings,
    events: Sender<ScriptEvent>,
    /// The host between `prepare` and `install`, only reached through `&mut self`
    /// (`Mutex::get_mut`, never locked): `ScriptHost` is `Send` but not `Sync`.
    host: std::sync::Mutex<Option<ScriptHost>>,
    components: Vec<String>,
}

impl ScriptPlugin {
    /// A plugin for `settings` and the receiving end of its event channel.
    pub fn new(settings: ScriptSettings) -> (Self, Receiver<ScriptEvent>) {
        let (events, receiver) = channel();
        (
            Self {
                settings,
                events,
                host: std::sync::Mutex::new(None),
                components: Vec::new(),
            },
            receiver,
        )
    }
}

/// The host inside the simulation world, owned by [`ScriptProvider`].
#[derive(Resource)]
struct Scripts {
    host: ScriptHost,
    events: Sender<ScriptEvent>,
}

// SAFETY: `Scripts` is private to this crate and no system declares `Res<Scripts>`:
// it is reached only through exclusive world access (the `scripts` system, tick
// boundaries, provider restore, commands) or `&World` reads on the thread that owns
// the session between ticks (provider capture, catalog, compile claims). No two
// threads ever hold it at once; `ScriptHost` itself is `Send`.
unsafe impl Sync for Scripts {}

fn failed(diagnostics: Vec<Diagnostic>) -> RuntimeError {
    RuntimeError(
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

impl RuntimePlugin for ScriptPlugin {
    fn name(&self) -> &'static str {
        "scripts"
    }

    fn prepare(&mut self, world: &mut World, schema: &mut SchemaDb) -> Result<()> {
        let engine = bind_component!(
            EngineBindings::new(),
            ara_components::Transform as "Transform" { position, rotation, scale }
        );
        let mut config = ScriptConfig::jit(&self.settings.host_dir);
        config.compile_server = self.settings.compile_server.clone();
        let mut host = ScriptHost::start(config, schema.clone(), &engine, world).map_err(failed)?;
        let specs = self
            .settings
            .modules
            .iter()
            .map(|dir| ModuleSpec::from_dir(dir))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|d| failed(vec![d]))?;
        host.load_modules(world, &specs).map_err(failed)?;
        let _ = self
            .events
            .send(ScriptEvent::Types(types_files(&specs, &host)));
        for ty in host.schema().types() {
            if schema.get(&ty.name).is_none() {
                schema.add(ty.clone()).map_err(|d| failed(vec![d]))?;
            }
        }
        self.components = host
            .module_components()
            .into_iter()
            .map(str::to_owned)
            .collect();
        self.host = std::sync::Mutex::new(Some(host));
        Ok(())
    }

    fn register(&mut self, registry: &mut SimRegistry) -> Result<()> {
        for name in &self.components {
            registry.register_external(name)?;
        }
        Ok(())
    }

    fn install(&mut self, stage: Stage, session: &mut Session) -> Result<()> {
        if stage != Stage::Gameplay {
            return Ok(());
        }
        let host = self
            .host
            .get_mut()
            .ok()
            .and_then(Option::take)
            .ok_or_else(|| RuntimeError("the C# host was not prepared".into()))?;
        let world = session.world_mut();
        world.insert_resource(Scripts {
            host,
            events: self.events.clone(),
        });
        // Document values projected during setup exist from tick 0.
        world.resource_scope::<Scripts, _>(|world, scripts| apply_writes(&scripts, world));
        session.add_provider(ScriptProvider)?;
        session.add_system("scripts", run_scripts)?;
        Ok(())
    }

    fn before_tick(&mut self, session: &mut Session) -> Result<()> {
        let world = session.world_mut();
        if !world.contains_resource::<Scripts>() {
            return Ok(());
        }
        let specs: Vec<ModuleSpec> = self
            .settings
            .modules
            .iter()
            .filter_map(|dir| ModuleSpec::from_dir(dir).ok())
            .collect();
        world.resource_scope::<Scripts, _>(|world, mut scripts| {
            let boundary = scripts.host.tick_boundary(world);
            if boundary.reloaded {
                let _ = scripts
                    .events
                    .send(ScriptEvent::Types(types_files(&specs, &scripts.host)));
            }
            let diagnostics = scripts.host.take_diagnostics();
            for report in boundary.reports {
                if report.kind == ara_script::abi::RELOAD_L4 {
                    let _ = scripts.events.send(ScriptEvent::RestartRequired);
                }
                let _ = scripts.events.send(ScriptEvent::Reload(report));
            }
            for d in diagnostics {
                let _ = scripts.events.send(ScriptEvent::Diagnostic(d));
            }
        });
        Ok(())
    }
}

/// `<module folder>/<Name>.types.json` → the module's reflected types.
fn types_files(specs: &[ModuleSpec], host: &ScriptHost) -> BTreeMap<PathBuf, Value> {
    let types = host.module_types();
    specs
        .iter()
        .filter_map(|spec| {
            let doc = types.get(&spec.name)?;
            Some((
                spec.dir.join(format!("{}.types.json", spec.name)),
                doc.clone(),
            ))
        })
        .collect()
}

/// Applies queued document values of C# components through the host.
fn apply_writes(scripts: &Scripts, world: &mut World) {
    let writes = std::mem::take(&mut world.resource_mut::<ExternalWrites>().0);
    for write in writes {
        let result = match &write.value {
            Some(value) => {
                scripts
                    .host
                    .insert_component(world, write.entity, &write.component, value)
            }
            None => {
                if let Some(id) = scripts.host.component_id(&write.component)
                    && let Ok(mut entity) = world.get_entity_mut(write.entity)
                {
                    entity.remove_by_id(id);
                }
                Ok(())
            }
        };
        if let Err(d) = result {
            let _ = scripts.events.send(ScriptEvent::Diagnostic(d));
        }
    }
}

/// The `scripts` system: applies projected values, then runs C# systems and behaviours.
fn run_scripts(world: &mut World) {
    let context = world.resource::<SimContext>();
    let tick = Tick {
        frame: context.tick(),
        seed: context.seed(),
        elapsed: context.tick() as f64 * context.dt(),
        dt: context.dt() as f32,
        reserved: 0,
    };
    world.resource_scope::<Scripts, _>(|world, mut scripts| {
        apply_writes(&scripts, world);
        scripts.host.run_tick(world, tick);
    });
}

/// Captures C# components (by entity identity) and C# resources into snapshots and
/// state hashes; restores them through the host. Entities without an identity (spawned
/// by C# at runtime) are not captured yet.
pub struct ScriptProvider;

impl StateProvider for ScriptProvider {
    fn name(&self) -> &'static str {
        "scripts"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![TypeId::of::<Scripts>()]
    }
    fn owned_components(&self, world: &World) -> Vec<ComponentId> {
        world
            .get_resource::<Scripts>()
            .map(|s| s.host.owned_ids(world))
            .unwrap_or_default()
    }
    fn capture(&self, world: &World) -> std::result::Result<Value, SimError> {
        let Some(scripts) = world.get_resource::<Scripts>() else {
            return Ok(Value::Null);
        };
        let host = &scripts.host;
        let mut names = host.module_components();
        names.sort_unstable();
        let mut entities = BTreeMap::new();
        for entity in world.iter_entities() {
            let Some(id) = entity.get::<Identity>() else {
                continue;
            };
            let values: Map<String, Value> = names
                .iter()
                .filter_map(|n| Some((n.to_string(), host.read_component(world, entity.id(), n)?)))
                .collect();
            if !values.is_empty() {
                entities.insert(id.0.clone(), values);
            }
        }
        let resources: BTreeMap<&str, Value> = host
            .module_resources()
            .into_iter()
            .filter_map(|n| Some((n, host.read_resource(world, n)?)))
            .collect();
        Ok(json!({"entities": entities, "resources": resources}))
    }
    fn restore(&self, world: &mut World, state: &Value) -> std::result::Result<(), SimError> {
        if !world.contains_resource::<Scripts>() || state.is_null() {
            return Ok(());
        }
        world.resource_scope::<Scripts, _>(|world, scripts| {
            let host = &scripts.host;
            let names: Vec<String> = host
                .module_components()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut targets = Vec::new();
            let mut query = world.query::<(Entity, &Identity)>();
            for (entity, id) in query.iter(world) {
                targets.push((entity, id.0.clone()));
            }
            for (entity, id) in targets {
                let saved = &state["entities"][&id];
                for name in &names {
                    match saved.get(name) {
                        Some(value) => host
                            .insert_component(world, entity, name, value)
                            .map_err(|d| SimError(d.to_string()))?,
                        None => {
                            if let Some(component) = host.component_id(name) {
                                world.entity_mut(entity).remove_by_id(component);
                            }
                        }
                    }
                }
            }
            if let Some(resources) = state["resources"].as_object() {
                for (name, value) in resources {
                    host.write_resource(world, name, value)
                        .map_err(|d| SimError(d.to_string()))?;
                }
            }
            Ok(())
        })
    }
}

/// Document addresses in `params` (`"e_fox"`) become the C# entity form (`"12v3"`).
fn resolve_entities(world: &mut World, params: &mut Value) {
    let mut ids = BTreeMap::new();
    let mut query = world.query::<(Entity, &Identity)>();
    for (entity, id) in query.iter(world) {
        ids.insert(id.0.clone(), entity);
    }
    fn walk(value: &mut Value, ids: &BTreeMap<String, Entity>) {
        match value {
            Value::String(s) => {
                if let Some(entity) = ids.get(s.as_str()) {
                    *value = ara_script::value::entity_json(entity.to_bits());
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| walk(v, ids)),
            Value::Object(map) => map.values_mut().for_each(|v| walk(v, ids)),
            _ => {}
        }
    }
    walk(params, &ids);
}

/// Runs a C# `[Command]` (`game.UseMedkit`) against the play world between ticks as
/// one undoable step. Entity params may use document addresses. Returns the command's
/// description (`ScriptHost::describe`: writes, cues, outcome).
pub fn execute(world: &mut World, name: &str, params: &Value) -> Result<Value> {
    if !world.contains_resource::<Scripts>() {
        return Err(RuntimeError("this world runs no C# modules".into()));
    }
    let mut params = params.clone();
    resolve_entities(world, &mut params);
    world.resource_scope::<Scripts, _>(|world, mut scripts| {
        let record = scripts
            .host
            .execute_command(world, name, &params, None)
            .map_err(|d| RuntimeError(d.to_string()))?;
        Ok(scripts.host.describe(&record))
    })
}

/// A C# component's value on `entity` (inspection, `play.read`), when it has one.
pub fn read(world: &World, entity: Entity, name: &str) -> Option<Value> {
    world
        .get_resource::<Scripts>()?
        .host
        .read_component(world, entity, name)
}

/// The C# command catalog (ara-cmd `Info` shape, `scope: "play"`), or `null`.
pub fn catalog(world: &World) -> Value {
    world
        .get_resource::<Scripts>()
        .map_or(Value::Null, |s| s.host.command_catalog())
}

/// Claims `paths` of `module` as an agent edit before the agent writes them (D20:
/// coalesced, compiled behind human edits). False when hot reload is off.
pub fn request_compile(world: &World, module: &str, paths: &[PathBuf]) -> bool {
    world
        .get_resource::<Scripts>()
        .is_some_and(|s| s.host.request_compile(module, paths, EditOrigin::Agent))
}

/// The module whose folder contains `path` (`(name, folder)`), among `settings.modules`.
pub fn module_of(settings: &ScriptSettings, path: &std::path::Path) -> Option<(String, PathBuf)> {
    settings
        .modules
        .iter()
        .filter(|dir| path.starts_with(dir))
        .max_by_key(|dir| dir.components().count())
        .and_then(|dir| Some((ModuleSpec::from_dir(dir).ok()?.name, dir.clone())))
}
