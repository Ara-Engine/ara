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
//! - Entities C# spawns get a stable identity (systems and behaviours `e_cs<tick>x<n>`,
//!   commands `e_cs<recorded tick>c<n>`, numbered in content order), so snapshots,
//!   state hashes and replays include them like authored entities.
//! - [`ScriptProvider`] captures C# components (by entity identity) and resources into
//!   snapshots and state hashes, and restores them.
//! - [`execute`], [`catalog`] and [`request_compile`] serve play commands and agent
//!   edits between ticks. Executed commands are recorded with the next tick
//!   ([`COMMAND`] boundary commands), so recordings replay them. Reload reports,
//!   diagnostics and L4 restarts arrive on the [`ScriptEvent`] channel.
use ara_diag::Diagnostic;
use ara_runtime::{
    Result, RuntimeError,
    plugin::{RuntimePlugin, Stage},
};
use ara_schema::Schema;
use ara_schema::SchemaDb;
use ara_script::{
    EditOrigin, ModuleSpec, ScriptConfig, ScriptHost,
    abi::{ReloadReport, Tick},
};
use ara_sim::{
    BoundaryCommand, ExternalWrites, Identity, Session, SimContext, SimError, SimRegistry,
    StateProvider,
};
use bevy_ecs::{component::ComponentId, prelude::*, resource::IsResource};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    any::TypeId,
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, channel},
};

/// Where the C# host and the game's modules are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSettings {
    /// `Ara.Host` build output for the reloadable JIT host
    /// (`dotnet/artifacts/bin/Ara.Host/release`).
    pub host_dir: PathBuf,
    /// `Ara.CompileServer.dll` for hot reload; `None` loads prebuilt assemblies only.
    pub compile_server: Option<PathBuf>,
    /// Private module input directories from the shared project inventory.
    pub modules: Vec<PathBuf>,
}

/// Selects development compilation or a precompiled shipping image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptHostConfig {
    /// Reloadable editor/Play host (the default configuration).
    Development(ScriptSettings),
    /// Portable NativeAOT metadata adjacent to the linked gameplay image.
    NativeAot {
        /// Export-local `scripts.json` path.
        manifest: PathBuf,
    },
}

impl From<ScriptSettings> for ScriptHostConfig {
    fn from(settings: ScriptSettings) -> Self {
        Self::Development(settings)
    }
}

/// ABI/layout identity for the engine components exposed by this runtime plugin.
pub fn shipping_layout(schema: &SchemaDb) -> Result<u64> {
    let bindings = ara_script::engine_bindings();
    let layouts = ara_script::csgen::engine_layouts(schema, &bindings.names())
        .map_err(|d| failed(vec![d]))?;
    Ok(ara_script::abi::layout_hash(&layouts))
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
    config: ScriptHostConfig,
    events: Sender<ScriptEvent>,
    /// The host between `prepare` and `install`, only reached through `&mut self`
    /// (`Mutex::get_mut`, never locked): `ScriptHost` is `Send` but not `Sync`.
    host: std::sync::Mutex<Option<ScriptHost>>,
    components: Vec<String>,
    prepared_world: Option<World>,
}

impl ScriptPlugin {
    /// A plugin for the selected host and the receiving end of its event channel.
    pub fn new(config: impl Into<ScriptHostConfig>) -> (Self, Receiver<ScriptEvent>) {
        let (events, receiver) = channel();
        (
            Self {
                config: config.into(),
                events,
                host: std::sync::Mutex::new(None),
                components: Vec::new(),
                prepared_world: None,
            },
            receiver,
        )
    }

    /// A shipping image described by an export-local `scripts.json`. Linked modules
    /// use the real gameplay ABI; no CoreCLR, compiler, watcher or reload is started.
    pub fn shipping(manifest: impl Into<PathBuf>) -> (Self, Receiver<ScriptEvent>) {
        Self::new(ScriptHostConfig::NativeAot {
            manifest: manifest.into(),
        })
    }

    /// Prepares the selected host before a pooled player is offered for Play.
    /// Development warms CoreCLR/Roslyn; shipping loads only its linked image.
    /// The empty world moves with its host so component IDs never cross worlds.
    pub fn prewarm(config: impl Into<ScriptHostConfig>) -> Result<(Self, Receiver<ScriptEvent>)> {
        let (mut plugin, events) = Self::new(config);
        let mut world = World::new();
        let mut schema = ara_runtime::schema(&[])?;
        plugin.prepare(&mut world, &mut schema)?;
        plugin.prepared_world = Some(world);
        Ok((plugin, events))
    }

    /// Whether this prepared host belongs to the requested module configuration.
    pub fn matches(&self, settings: &ScriptSettings) -> bool {
        matches!(&self.config, ScriptHostConfig::Development(prepared) if prepared == settings)
    }
}

/// The host inside the simulation world, owned by [`ScriptProvider`].
#[derive(Resource)]
struct Scripts {
    host: ScriptHost,
    events: Sender<ScriptEvent>,
    recovery: Option<u64>,
    recovery_specs: Vec<ModuleSpec>,
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
        if let Some(prepared) = self.prepared_world.take() {
            *world = prepared;
            let host = self
                .host
                .get_mut()
                .ok()
                .and_then(|host| host.as_ref())
                .ok_or_else(|| RuntimeError("prewarmed script host missing".into()))?;
            for ty in host.schema().types() {
                if schema.get(&ty.name).is_none() {
                    schema.add(ty.clone()).map_err(|d| failed(vec![d]))?;
                }
            }
            return Ok(());
        }
        let engine = ara_script::engine_bindings();
        let (config, specs, manifest) = match &self.config {
            ScriptHostConfig::Development(settings) => {
                let mut config = ScriptConfig::jit(&settings.host_dir);
                config.compile_server = settings.compile_server.clone();
                let specs = settings
                    .modules
                    .iter()
                    .map(|dir| ModuleSpec::from_dir(dir))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|d| failed(vec![d]))?;
                (config, specs, None)
            }
            ScriptHostConfig::NativeAot { manifest: path } => {
                let manifest = ara_script::shipping::ScriptManifest::read(path)
                    .map_err(|d| failed(vec![d]))?;
                manifest
                    .validate(shipping_layout(schema)?)
                    .map_err(|d| failed(vec![d]))?;
                let config = ScriptConfig::native_aot(
                    manifest.library_path(path).map_err(|d| failed(vec![d]))?,
                );
                (config, Vec::new(), Some(manifest))
            }
        };
        let mut host = ScriptHost::start(config, schema.clone(), &engine, world).map_err(|mut problems| {
            if manifest.is_some() {
                for diagnostic in &mut problems {
                    if diagnostic.code == ara_script::codes::STALE_BINDINGS {
                        diagnostic.message.push_str("; rebuild shipping scripts and the player with the same Ara SDK and export again");
                    }
                }
            }
            failed(problems)
        })?;
        host.load_modules(world, &specs).map_err(failed)?;
        if let Some(manifest) = manifest {
            let mut actual: Vec<_> = host.modules().into_iter().map(|m| m.name).collect();
            let mut expected = manifest.modules;
            actual.sort();
            expected.sort();
            if actual != expected {
                return Err(failed(vec![ara_diag::Diagnostic::error(
                    ara_script::codes::LOAD_FAILED,
                    format!(
                        "shipping module inventory mismatch: expected {expected:?}, loaded {actual:?}; export again"
                    ),
                )]));
            }
        }
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
        registry.register_command::<ScriptCommand>(COMMAND, replay_command)?;
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
            recovery: None,
            recovery_specs: match &self.config {
                ScriptHostConfig::Development(settings) => settings
                    .modules
                    .iter()
                    .map(|dir| ModuleSpec::from_dir(dir))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|diagnostic| failed(vec![diagnostic]))?,
                ScriptHostConfig::NativeAot { .. } => Vec::new(),
            },
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
        let specs: Vec<ModuleSpec> = match &self.config {
            ScriptHostConfig::Development(settings) => settings
                .modules
                .iter()
                .filter_map(|dir| ModuleSpec::from_dir(dir).ok())
                .collect(),
            ScriptHostConfig::NativeAot { .. } => Vec::new(),
        };
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
                    let _ = scripts
                        .events
                        .send(restart_event(scripts.recovery.is_some()));
                }
                let _ = scripts.events.send(ScriptEvent::Reload(report));
            }
            for d in diagnostics {
                if recovery_completed(&d, scripts.recovery) {
                    scripts.recovery = None;
                }
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

/// The `scripts` system: applies projected values, then runs C# systems and behaviours
/// and names the entities they spawned.
fn run_scripts(world: &mut World) {
    let context = world.resource::<SimContext>();
    let actions = context
        .actions()
        .map(|(player, name, value)| {
            let axes = match value {
                ara_input::ActionValue::Button(value) => [f32::from(u8::from(value)), 0.0],
                ara_input::ActionValue::Axis(value) => [value, 0.0],
                ara_input::ActionValue::Axis2(value) => value,
            };
            ((player.0, name.to_owned()), axes)
        })
        .collect();
    let tick = Tick {
        frame: context.tick(),
        seed: context.seed(),
        elapsed: context.tick() as f64 * context.dt(),
        dt: context.dt() as f32,
        reserved: 0,
    };
    let players = world
        .query::<(Entity, &ara_runtime::components::PlayerControl)>()
        .iter(world)
        .map(|(e, c)| (e.to_bits(), c.player))
        .collect();
    world.insert_resource(ara_script::ScriptInputs { actions, players });
    world.init_resource::<ara_script::ScriptNavigation>();
    world.init_resource::<ara_script::ScriptControls>();
    world.resource_scope::<Scripts, _>(|world, mut scripts| {
        apply_writes(&scripts, world);
        scripts.host.run_tick(world, tick);
        identify_spawns(&scripts, world, &format!("e_cs{}x", tick.frame));
    });
    let requests = std::mem::take(&mut world.resource_mut::<ara_script::ScriptNavigation>().0);
    for (entity, target) in requests {
        if let Some(entity) = Entity::try_from_bits(entity)
            && let Some(mut state) = world.get_mut::<ara_nav::NavAgentState>(entity)
            && state.target != target
        {
            state.set_target(target);
        }
    }
    let controls = std::mem::take(&mut *world.resource_mut::<ara_script::ScriptControls>());
    for handle in controls.stopped {
        if let Some(entity) = Entity::try_from_bits(handle)
            && let Some(mut input) = world.get_mut::<ara_physics::CharacterInput>(entity)
        {
            input.desired_velocity = [0.; 3];
            input.jump_speed = 0.;
        }
    }
    for (handle, name, value) in controls.animation {
        if let Some(entity) = Entity::try_from_bits(handle)
            && let Some(mut slot) = world.get_mut::<ara_runtime::animation::AnimatorSlot>(entity)
            && let Some(parameter) = slot.program.controller.parameter(&name)
            && let Err(error) = slot
                .animator
                .set(parameter, ara_anim::graph::ParameterValue::Bool(value))
        {
            slot.fault = Some(error.0);
        }
    }
}

/// Gives every entity C# spawned (no `Identity` yet) a stable one, `<prefix><n>`,
/// numbered in content order (Transform plus C# component values) after any earlier
/// names with that prefix: spawn handles depend on allocator history, content does
/// not, and equal entities are interchangeable. Snapshots, state hashes and replays
/// then include them.
fn identify_spawns(scripts: &Scripts, world: &mut World, prefix: &str) {
    let fresh: Vec<Entity> = world
        .query_filtered::<Entity, (Without<Identity>, Without<IsResource>)>()
        .iter(world)
        .collect();
    if fresh.is_empty() {
        return;
    }
    let first = world
        .query::<&Identity>()
        .iter(world)
        .filter(|id| id.0.starts_with(prefix))
        .count();
    let mut names = scripts.host.module_components();
    names.sort_unstable();
    let key = |entity: Entity| {
        let transform = world
            .get::<ara_components::Transform>(entity)
            .map(|t| json!([t.position, t.rotation, t.scale]));
        let values: Vec<Option<Value>> = names
            .iter()
            .map(|n| scripts.host.read_component(world, entity, n))
            .collect();
        json!([transform, values]).to_string()
    };
    let mut keyed: Vec<(String, Entity)> = fresh.into_iter().map(|e| (key(e), e)).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    for (n, (_, entity)) in keyed.into_iter().enumerate() {
        world
            .entity_mut(entity)
            .insert(Identity(format!("{prefix}{}", first + n)));
    }
}

/// Boundary command name of a C# `[Command]` run between ticks ([`execute`]).
pub const COMMAND: &str = "script.command";

/// A C# `[Command]` invocation recorded for replay.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "scripts")]
#[serde(default, rename_all = "camelCase")]
pub struct ScriptCommand {
    /// Command name (`game.UseMedkit`).
    pub name: String,
    /// Parameters with entities as document addresses (`e_fox`).
    pub params: Value,
}

/// Replays a recorded C# command at its boundary (the tick that records it).
fn replay_command(
    world: &mut World,
    _: &BoundaryCommand,
    command: ScriptCommand,
) -> std::result::Result<(), SimError> {
    let tick = world.resource::<SimContext>().tick();
    run_command(world, &command.name, &command.params, tick)
        .map(drop)
        .map_err(|e| SimError(e.0))
}

/// Runs a command with resolved entity parameters and names what it spawned after
/// `tick`, the tick whose record carries the command (live and replayed alike).
fn run_command(world: &mut World, name: &str, params: &Value, tick: u64) -> Result<Value> {
    if !world.contains_resource::<Scripts>() {
        return Err(RuntimeError("this world runs no C# modules".into()));
    }
    let mut params = params.clone();
    resolve_entities(world, &mut params);
    let prefix = format!("e_cs{tick}c");
    world.resource_scope::<Scripts, _>(|world, mut scripts| {
        let record = scripts
            .host
            .execute_command(world, name, &params, None)
            .map_err(|diagnostic| {
                let message = diagnostic.to_string();
                // A command consumes its synchronous fault row from the managed host;
                // publish that original row too, before the request becomes a text error.
                let _ = scripts.events.send(ScriptEvent::Diagnostic(diagnostic));
                RuntimeError(message)
            })?;
        identify_spawns(&scripts, world, &prefix);
        Ok(scripts.host.describe(&record))
    })
}

/// Captures C# components (by entity identity) and C# resources into snapshots and
/// state hashes; restores them through the host (C#-spawned entities included: they
/// are named when spawned).
pub struct ScriptProvider;

impl StateProvider for ScriptProvider {
    fn name(&self) -> &'static str {
        "scripts"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![
            TypeId::of::<Scripts>(),
            TypeId::of::<ara_script::ScriptInputs>(),
            TypeId::of::<ara_script::ScriptNavigation>(),
            TypeId::of::<ara_script::ScriptControls>(),
        ]
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
/// one undoable step, and records it with the next tick (agent player 0) so
/// recordings replay it. Entity params may use document addresses. Returns the
/// command's description (`ScriptHost::describe`: writes, cues, outcome).
pub fn execute(session: &mut Session, name: &str, params: &Value) -> Result<Value> {
    let tick = session.tick() + 1;
    let described = run_command(session.world_mut(), name, params, tick)?;
    session.record_applied(BoundaryCommand {
        kind: COMMAND.into(),
        player: ara_input::PlayerId(0),
        sequence: 0,
        target: None,
        payload: serde_json::to_value(ScriptCommand {
            name: name.to_owned(),
            params: params.clone(),
        })?,
    })?;
    Ok(described)
}

/// A C# component's value on `entity` (inspection, `play.read`), when it has one.
pub fn read(world: &World, entity: Entity, name: &str) -> Option<Value> {
    world
        .get_resource::<Scripts>()?
        .host
        .read_component(world, entity, name)
}

/// Script component names for policy-filtered network presentation.
pub fn component_names(world: &World) -> Vec<String> {
    world
        .get_resource::<Scripts>()
        .map(|s| {
            s.host
                .module_components()
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Applies a received partial component projection in a never-stepped client
/// mirror. Fields absent from the projection retain their local values.
pub fn apply_replica(
    world: &mut World,
    entity: Entity,
    name: &str,
    projection: &Value,
) -> Result<()> {
    fn merge(current: &mut Value, incoming: &Value) {
        if let (Some(current), Some(incoming)) = (current.as_object_mut(), incoming.as_object()) {
            for (key, value) in incoming {
                merge(current.entry(key.clone()).or_insert(Value::Null), value);
            }
        } else {
            *current = incoming.clone();
        }
    }
    if !world.contains_resource::<Scripts>() {
        return Ok(());
    }
    world.resource_scope::<Scripts, _>(|world, scripts| {
        let mut value = scripts
            .host
            .read_component(world, entity, name)
            .unwrap_or_else(|| json!({}));
        merge(&mut value, projection);
        scripts
            .host
            .insert_component(world, entity, name, &value)
            .map_err(|d| failed(vec![d]))
    })
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

/// Queue an engine-private recovery without advancing simulation or waiting for compilation.
/// False means no reloadable host accepted the request; the caller reports the failure.
pub fn request_recovery(world: &mut World, ticket: u64) -> bool {
    let Some(mut scripts) = world.get_resource_mut::<Scripts>() else {
        return false;
    };
    if scripts.recovery.is_some() {
        return false;
    }
    if !scripts.host.request_recovery(ticket) {
        return false;
    }
    scripts.recovery = Some(ticket);
    true
}

/// Applies prepared recovery work at a boundary even while Play is paused. Never steps
/// simulation, performs discovery or waits for the compiler/CLR preparation worker.
/// Queued diagnostics also flow while paused, without applying an ordinary reload.
pub fn poll_recovery(world: &mut World) {
    let Some(scripts) = world.get_resource::<Scripts>() else {
        return;
    };
    if scripts.recovery.is_none() {
        if scripts.host.has_diagnostics() {
            for diagnostic in scripts.host.take_diagnostics() {
                let _ = scripts.events.send(ScriptEvent::Diagnostic(diagnostic));
            }
        }
        return;
    }
    world.resource_scope::<Scripts, _>(|world, mut scripts| {
        let boundary = scripts.host.tick_boundary(world);
        if boundary.reloaded {
            let _ = scripts.events.send(ScriptEvent::Types(types_files(
                &scripts.recovery_specs,
                &scripts.host,
            )));
        }
        for report in boundary.reports {
            if report.kind == ara_script::abi::RELOAD_L4 {
                let _ = scripts
                    .events
                    .send(restart_event(scripts.recovery.is_some()));
            }
            let _ = scripts.events.send(ScriptEvent::Reload(report));
        }
        for diagnostic in scripts.host.take_diagnostics() {
            if recovery_completed(&diagnostic, scripts.recovery) {
                scripts.recovery = None;
            }
            let _ = scripts.events.send(ScriptEvent::Diagnostic(diagnostic));
        }
    });
}

fn recovery_completed(diagnostic: &Diagnostic, ticket: Option<u64>) -> bool {
    diagnostic.code == ara_script::codes::RECOVERY_RESULT
        && ticket.is_some_and(|ticket| {
            diagnostic
                .data
                .as_ref()
                .and_then(|data| data["recovery"]["ticket"].as_u64())
                == Some(ticket)
        })
}

fn restart_event(recovering: bool) -> ScriptEvent {
    if recovering {
        ScriptEvent::Diagnostic(Diagnostic::error(
            ara_script::codes::RESTART_REQUIRED,
            "Explicit script recovery cannot replace changed engine/module bindings; stop Play and reopen explicitly. No restart was performed.",
        ))
    } else {
        ScriptEvent::RestartRequired
    }
}

/// The module whose folder contains `path` (`(name, folder)`), among `settings.modules`.
pub fn module_of(settings: &ScriptSettings, path: &std::path::Path) -> Option<(String, PathBuf)> {
    settings
        .modules
        .iter()
        .filter_map(|dir| {
            let module = ModuleSpec::from_dir(dir).ok()?;
            module
                .owns_source(path)
                .ok()?
                .then_some((module.name, dir.clone()))
        })
        .max_by_key(|(_, dir)| dir.components().count())
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn explicit_recovery_reports_stale_bindings_without_requesting_a_play_restart() {
        let ScriptEvent::Diagnostic(diagnostic) = restart_event(true) else {
            panic!("recovery must not request restart");
        };
        assert_eq!(diagnostic.code, ara_script::codes::RESTART_REQUIRED);
        assert!(diagnostic.message.contains("No restart was performed"));
        assert!(matches!(restart_event(false), ScriptEvent::RestartRequired));
    }
}
