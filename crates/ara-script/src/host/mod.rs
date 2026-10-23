//! [`ScriptHost`]: the C# runtime attached to a bevy `World`.
//!
//! Per tick the owner (the simulation) calls, in order:
//! 1. [`ScriptHost::tick_boundary`]: applies staged reloads (no C# code is running);
//! 2. [`ScriptHost::run_tick`]: C# systems in parallel on the compute task pool (their
//!    command buffers applied at bevy sync points), then the behaviour phase.
//!
//! [`ScriptHost::run_frame`] runs `OnFrame` per presented frame, and
//! [`ScriptHost::execute_command`] runs one gameplay command as an undoable step.

mod bindings;
mod command;
mod ctx;
mod registry;

use std::mem::{ManuallyDrop, MaybeUninit};
use std::path::PathBuf;

use ara_diag::Diagnostic;
use ara_schema::SchemaDb;
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::{IntoScheduleConfigs, Mut, Res, Resource};
use bevy_ecs::schedule::{MultiThreadedExecutor, Schedule, SystemSet};
use bevy_ecs::system::{ParamBuilder, SystemParamBuilder, SystemState};
use bevy_ecs::world::World;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use serde_json::{Value, json};

pub use bindings::EngineBindings;
pub use command::{CommandRecord, ComponentWrite, Cue};
pub use ctx::LogSink;

use crate::abi::{self, Frame, HostInit, PHASE_FRAME, PHASE_TICK, ReloadReport, ScriptApi, Tick};
use crate::clr::{self, Loaded, Runtime};
use crate::codes;
use crate::ecs::behaviour::{self, BehaviourPlan};
use crate::ecs::cmdbuf::{self, CmdBuf, RawCue, ScriptCatalog, ScriptEvents};
use crate::ecs::system::{ScriptQuery, ScriptQueryBuilder, SystemPlan};
use crate::module::ModuleSpec;
use crate::value;
use ctx::{HostCtx, Scope};
use registry::{Owner, Registry, ScriptComponent};

/// How to start the C# host.
pub struct ScriptConfig {
    /// The runtime: JIT (dev, reloadable) or a NativeAOT image.
    pub runtime: Runtime,
    /// `Ara.CompileServer.dll` for hot reload (JIT only); `None` loads modules without reload.
    pub compile_server: Option<PathBuf>,
    /// Raise tick-thread priority while the compile server compiles (A13).
    pub boost_ticks: bool,
    /// Where C# log lines go; defaults to stderr for warnings and errors.
    pub log: Option<LogSink>,
}

impl ScriptConfig {
    /// A JIT host from an `Ara.Host` build directory, EnC on, no compile server.
    pub fn jit(dir: impl Into<PathBuf>) -> Self {
        ScriptConfig {
            runtime: Runtime::Jit {
                dir: dir.into(),
                enc: true,
            },
            compile_server: None,
            boost_ticks: true,
            log: None,
        }
    }

    /// A NativeAOT host from a module image exporting `ara_script_init`.
    pub fn native_aot(library: impl Into<PathBuf>) -> Self {
        ScriptConfig {
            runtime: Runtime::NativeAot {
                library: library.into(),
            },
            compile_server: None,
            boost_ticks: false,
            log: None,
        }
    }
}

/// The current tick, read by every C# system.
#[derive(Resource, Clone, Copy)]
pub(crate) struct ScriptTick(pub Tick);

/// System set holding one C# system (for ordering edges).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ScriptSet(u32);

/// What a tick boundary did.
#[derive(Debug, Clone, Default)]
pub struct Boundary {
    /// Code changed (an L1 delta or an L3 swap was applied).
    pub reloaded: bool,
    /// Every reload report received since the last boundary (also errors).
    pub reports: Vec<ReloadReport>,
}

/// A module's state, for status bars and the profiler's heat badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleStatus {
    /// Module slot.
    pub slot: u32,
    /// Module name.
    pub name: String,
    /// L3 generations so far.
    pub generation: u32,
    /// True if Debug IL with EnC (hot: 4–24× slower loops, R10).
    pub hot: bool,
    /// Registered systems, behaviours and commands.
    pub counts: (usize, usize, usize),
}

/// Pointer scratch for the behaviour phase (reused across frames).
struct PhaseScratch(Vec<(*const u64, *mut u8, usize)>);

// SAFETY: the pointers are only valid inside one behaviour phase on the calling thread
// and are cleared before it ends; between phases the vector holds no live pointers.
unsafe impl Send for PhaseScratch {}

/// The C# runtime attached to a world. One per process (one .NET runtime).
pub struct ScriptHost {
    ctx: Box<HostCtx>,
    api: ScriptApi,
    registry: Registry,
    /// Never dropped: a .NET runtime cannot be unloaded from a process.
    _runtime: ManuallyDrop<Loaded>,
    compile_server: Option<PathBuf>,
    schedule: Option<Schedule>,
    behaviours: Vec<BehaviourPlan>,
    phase_buf: CmdBuf,
    phase_scratch: PhaseScratch,
    sequence: u64,
    tick: Tick,
    shut_down: bool,
}

fn default_log(level: u32, msg: &str) {
    if level > 0 {
        let tag = if level == 1 { "warn" } else { "error" };
        eprintln!("[ara-script {tag}] {msg}");
    }
}

impl ScriptHost {
    /// Starts the C# runtime and binds the engine components.
    ///
    /// `schema` must contain the engine components in `engine`; C# modules add their
    /// types to the host's copy ([`ScriptHost::schema`]). On non-Windows platforms call
    /// this before other threads read the environment: EnC needs an environment
    /// variable set before .NET starts.
    pub fn start(
        config: ScriptConfig,
        schema: SchemaDb,
        engine: &EngineBindings,
        world: &mut World,
    ) -> Result<Self, Vec<Diagnostic>> {
        ComputeTaskPool::get_or_init(|| {
            TaskPoolBuilder::new()
                .on_thread_spawn(crate::boost::register_tick_thread)
                .build()
        });
        crate::boost::register_tick_thread();
        let mut registry = Registry::new(schema);
        let resolved = engine
            .resolve(&registry.schema, world)
            .map_err(|d| vec![d])?;
        let layouts: Vec<_> = resolved.iter().map(|(_, _, l)| l.clone()).collect();
        for (name, id, layout) in resolved {
            registry.components.push(ScriptComponent {
                name,
                id,
                layout,
                owner: Owner::Engine,
            });
        }
        let ctx = Box::new(HostCtx::new(
            config.log.unwrap_or_else(|| Box::new(default_log)),
        ));
        if !config.boost_ticks {
            ctx.boost.disable();
        }
        let (init, loaded) = clr::start(&config.runtime).map_err(|d| vec![d])?;
        let enc = matches!(config.runtime, Runtime::Jit { enc: true, .. });
        let host = HostInit {
            abi_version: abi::ABI_VERSION,
            flags: if enc { abi::INIT_ENC } else { 0 },
            layout_hash: abi::layout_hash(&layouts),
            ctx: std::ptr::from_ref::<HostCtx>(&ctx).cast_mut().cast(),
            api: HostCtx::api(),
        };
        let mut api = MaybeUninit::<ScriptApi>::uninit();
        // SAFETY: `init` is `ara_script_init`; `host` and `api` are valid for the call, and
        // `ctx` (boxed, owned by the returned host) outlives the runtime's use of it.
        let rc = unsafe { init(&host, api.as_mut_ptr()) };
        if rc != 0 {
            let mut problems = std::mem::take(
                &mut *ctx
                    .diagnostics
                    .lock()
                    .map_err(|_| Vec::<Diagnostic>::new())?,
            );
            problems.push(Diagnostic::error(
                codes::RUNTIME_START,
                format!("ara_script_init failed with {rc}"),
            ));
            return Err(problems);
        }
        // SAFETY: `ara_script_init` returned 0, which means it filled every entry.
        let api = unsafe { api.assume_init() };
        world.insert_resource(registry.catalog());
        world.insert_resource(ScriptEvents::default());
        world.insert_resource(ScriptTick(Tick {
            frame: 0,
            seed: 0,
            elapsed: 0.0,
            dt: 0.0,
            reserved: 0,
        }));
        Ok(ScriptHost {
            ctx,
            api,
            registry,
            _runtime: ManuallyDrop::new(loaded),
            compile_server: config.compile_server,
            schedule: None,
            behaviours: Vec::new(),
            phase_buf: CmdBuf::default(),
            phase_scratch: PhaseScratch(Vec::new()),
            sequence: 0,
            tick: Tick {
                frame: 0,
                seed: 0,
                elapsed: 0.0,
                dt: 0.0,
                reserved: 0,
            },
            shut_down: false,
        })
    }

    /// Runs `f` with exclusive world access published to C#.
    fn exclusive<R>(
        &mut self,
        world: &mut World,
        register: bool,
        journal: Option<&mut ctx::Journal>,
        f: impl FnOnce(&ScriptApi, UnsafeWorldCell<'_>) -> R,
    ) -> R {
        let cell = world.as_unsafe_world_cell();
        let mut scope = Scope {
            world: cell,
            registry: &raw mut self.registry,
            journal: journal.map(std::ptr::from_mut),
            thread: std::thread::current().id(),
            register,
        };
        let api = &self.api;
        self.ctx.with_scope(&mut scope, || f(api, cell))
    }

    /// Loads modules (compiled by the compile server when configured, otherwise from
    /// their prebuilt `assembly`) and registers them. Errors leave earlier modules running.
    pub fn load_modules(
        &mut self,
        world: &mut World,
        modules: &[ModuleSpec],
    ) -> Result<(), Vec<Diagnostic>> {
        let config = json!({
            "compileServer": self.compile_server.as_ref().map(|p| p.display().to_string()),
            "modules": modules.iter().map(|m| json!({
                "name": m.name,
                "dir": m.dir.display().to_string(),
                "assembly": m.assembly.as_ref().map(|p| p.display().to_string()),
                "watch": m.watch && self.compile_server.is_some(),
                "hot": m.hot,
            })).collect::<Vec<_>>(),
        })
        .to_string();
        let rc = self.exclusive(world, true, None, |api, _| {
            // SAFETY: the JSON span is valid for the call; world access is published.
            unsafe {
                (api.load_modules)(
                    config.as_ptr(),
                    u32::try_from(config.len()).unwrap_or(u32::MAX),
                )
            }
        });
        self.rebuild(world);
        let problems: Vec<Diagnostic> = self
            .take_diagnostics()
            .into_iter()
            .filter(Diagnostic::is_error)
            .collect();
        if rc != 0 || !problems.is_empty() {
            return Err(if problems.is_empty() {
                vec![Diagnostic::error(
                    codes::LOAD_FAILED,
                    format!("load_modules failed with {rc}"),
                )]
            } else {
                problems
            });
        }
        Ok(())
    }

    /// Applies staged reloads (call between ticks). Cheap when nothing is pending.
    pub fn tick_boundary(&mut self, world: &mut World) -> Boundary {
        // SAFETY: plain call into the runtime.
        let pending = unsafe { (self.api.has_pending)() } != 0;
        let mut out = Boundary::default();
        if pending {
            let tick = self.tick;
            let flags = self.exclusive(world, true, None, |api, _| {
                // SAFETY: no C# code is running; world access is published for registration.
                unsafe { (api.tick_boundary)(&tick) }
            });
            out.reloaded = flags & abi::BOUNDARY_RELOADED != 0;
            self.rebuild(world);
        }
        if let Ok(mut r) = self.ctx.reloads.lock() {
            out.reports = std::mem::take(&mut *r);
        }
        out
    }

    /// Rebuilds the system schedule and behaviour plans after the registry changed.
    fn rebuild(&mut self, world: &mut World) {
        if self.registry.catalog_dirty {
            self.registry.catalog_dirty = false;
            world.insert_resource(self.registry.catalog());
        }
        if !self.registry.dirty {
            return;
        }
        self.registry.dirty = false;
        let reg = &self.registry;
        let mut systems: Vec<_> = reg
            .modules
            .values()
            .flat_map(|m| m.systems.iter())
            .collect();
        systems.sort_by(|a, b| (a.order, &a.name, a.id).cmp(&(b.order, &b.name, b.id)));
        let mut schedule = Schedule::default();
        schedule.set_executor(MultiThreadedExecutor::new());
        let comp = |i: u32| reg.components.get(i as usize);
        for (k, s) in systems.iter().enumerate() {
            let columns: Option<Vec<_>> = s
                .columns
                .iter()
                .map(|&(i, w)| comp(i).map(|c| (c.id, w, c.layout.size)))
                .collect();
            let resources: Option<Vec<_>> = s
                .resources
                .iter()
                .map(|&(i, w)| {
                    reg.resources
                        .get(i as usize)
                        .and_then(|r| r.id)
                        .map(|id| (id, w))
                })
                .collect();
            let with: Option<Vec<ComponentId>> =
                s.with.iter().map(|&i| comp(i).map(|c| c.id)).collect();
            let without: Option<Vec<ComponentId>> =
                s.without.iter().map(|&i| comp(i).map(|c| c.id)).collect();
            let (Some(columns), Some(resources), Some(with), Some(without)) =
                (columns, resources, with, without)
            else {
                self.ctx.report(Diagnostic::error(
                    codes::UNKNOWN_COMPONENT,
                    format!(
                        "system `{}` refers to an unregistered type; not scheduled",
                        s.name
                    ),
                ));
                continue;
            };
            let plan = SystemPlan {
                id: s.id,
                columns,
                resources,
                with,
                without,
                api: self.api,
            };
            let state: SystemState<(ScriptQuery<'static, 'static>, Res<'static, ScriptTick>)> =
                (ScriptQueryBuilder(plan), ParamBuilder).build_state(world);
            let system =
                state.build_system(|mut q: ScriptQuery<'_, '_>, tick: Res<'_, ScriptTick>| {
                    q.run(&tick.0)
                });
            let conflicts = |a: &registry::SystemDecl, b: &registry::SystemDecl| {
                let clash = |x: &[(u32, bool)], y: &[(u32, bool)]| {
                    x.iter()
                        .any(|(i, w)| y.iter().any(|(j, v)| i == j && (*w || *v)))
                };
                clash(&a.columns, &b.columns) || clash(&a.resources, &b.resources)
            };
            let mut config = system.in_set(ScriptSet(s.id)).into_configs();
            for earlier in systems[..k].iter().filter(|e| conflicts(e, s)) {
                config = config.after(ScriptSet(earlier.id));
            }
            schedule.add_systems(config);
        }
        self.schedule = Some(schedule);
        self.behaviours = reg
            .modules
            .values()
            .flat_map(|m| m.behaviours.iter())
            .filter_map(|b| {
                let found = comp(b.data).map(|c| (b.id, c.id));
                if found.is_none() {
                    self.ctx.report(Diagnostic::error(
                        codes::UNKNOWN_COMPONENT,
                        format!("behaviour `{}` has no registered data component", b.name),
                    ));
                }
                found
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(id, data)| BehaviourPlan::new(world, id, data))
            .collect();
    }

    /// Runs one fixed tick: C# systems in parallel, then the behaviour phase.
    pub fn run_tick(&mut self, world: &mut World, tick: Tick) {
        self.tick = tick;
        world.resource_mut::<ScriptTick>().0 = tick;
        if let Some(schedule) = self.schedule.as_mut() {
            schedule.run(world);
        }
        self.drain_events(world);
        self.behaviour_phase(world, PHASE_TICK, None);
    }

    /// Runs `OnFrame` on every behaviour (presentation; never change simulation state there).
    pub fn run_frame(&mut self, world: &mut World, frame: Frame) {
        self.behaviour_phase(world, PHASE_FRAME, Some(&frame));
    }

    fn behaviour_phase(&mut self, world: &mut World, phase: u32, frame: Option<&Frame>) {
        if self.behaviours.is_empty() {
            return;
        }
        let this_run = world.change_tick();
        let tick = self.tick;
        let cell = world.as_unsafe_world_cell();
        self.phase_buf.reset(cell.entity_allocator());
        let mut scope = Scope {
            world: cell,
            registry: &raw mut self.registry,
            journal: None,
            thread: std::thread::current().id(),
            register: false,
        };
        let (plans, api, buf, scratch) = (
            &mut self.behaviours,
            &self.api,
            &mut self.phase_buf,
            &mut self.phase_scratch.0,
        );
        self.ctx.with_scope(&mut scope, || {
            let p = behaviour::Phase {
                api,
                phase,
                tick: &tick,
                frame,
                this_run,
            };
            behaviour::run_phase(cell, plans, &p, buf, scratch);
        });
        if self.phase_buf.ops.is_empty() {
            return;
        }
        let ops = std::mem::take(&mut self.phase_buf.ops);
        let mut cues = Vec::new();
        let mut problems = Vec::new();
        world.resource_scope(|world, catalog: Mut<ScriptCatalog>| {
            cmdbuf::apply(world, &catalog, &ops, &mut cues, &mut problems, None);
        });
        self.phase_buf.ops = ops;
        self.phase_buf.ops.clear();
        self.push_events(world, cues, problems);
    }

    fn drain_events(&mut self, world: &mut World) {
        if let Some(mut ev) = world.get_resource_mut::<ScriptEvents>()
            && (!ev.cues.is_empty() || !ev.problems.is_empty())
        {
            let cues = std::mem::take(&mut ev.cues);
            let problems = std::mem::take(&mut ev.problems);
            self.push_events(world, cues, problems);
        }
    }

    fn push_events(&mut self, world: &mut World, cues: Vec<RawCue>, problems: Vec<Diagnostic>) {
        for p in problems {
            self.ctx.report(p);
        }
        if !cues.is_empty()
            && let Some(mut ev) = world.get_resource_mut::<ScriptEvents>()
        {
            ev.cues.extend(cues);
        }
    }

    /// Cues emitted since the last call, decoded to JSON.
    pub fn take_cues(&mut self, world: &mut World) -> Vec<Cue> {
        let raw = world
            .get_resource_mut::<ScriptEvents>()
            .map(|mut e| std::mem::take(&mut e.cues))
            .unwrap_or_default();
        raw.into_iter()
            .filter_map(|c| self.decode_cue(&c))
            .collect()
    }

    fn decode_cue(&self, c: &RawCue) -> Option<Cue> {
        let cue = self.registry.cues.get(c.cue as usize)?;
        Some(Cue {
            name: cue.name.clone(),
            value: value::decode(&self.registry.schema, &cue.layout, &c.data),
        })
    }

    /// Diagnostics reported since the last call (faults, compile errors, registration).
    pub fn take_diagnostics(&self) -> Vec<Diagnostic> {
        self.ctx
            .diagnostics
            .lock()
            .map(|mut d| std::mem::take(&mut *d))
            .unwrap_or_default()
    }

    /// The Schema DB with every C# module's types registered (`reflection.json` source).
    pub fn schema(&self) -> &SchemaDb {
        &self.registry.schema
    }

    /// The bevy component holding schema component `name`.
    pub fn component_id(&self, name: &str) -> Option<ComponentId> {
        self.registry
            .component(name)
            .map(|i| self.registry.components[i as usize].id)
    }

    /// A component's value as JSON (agent inspection).
    pub fn read_component(&self, world: &World, entity: Entity, name: &str) -> Option<Value> {
        let c = &self.registry.components[self.registry.component(name)? as usize];
        let bytes = cmdbuf::component_bytes(world, entity, c.id, c.layout.size)?;
        Some(value::decode(&self.registry.schema, &c.layout, &bytes))
    }

    /// Inserts (or replaces) a component from JSON; missing fields take their defaults.
    pub fn insert_component(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
        v: &Value,
    ) -> Result<(), Diagnostic> {
        let i = self.registry.component(name).ok_or_else(|| {
            Diagnostic::error(
                codes::UNKNOWN_COMPONENT,
                format!("no script component `{name}`"),
            )
        })?;
        let c = &self.registry.components[i as usize];
        let bytes = value::encode(&self.registry.schema, &c.layout, v, name)?;
        if cmdbuf::insert_bytes(world, entity, c.id, &bytes) {
            Ok(())
        } else {
            Err(Diagnostic::error(
                codes::UNDO_FAILED,
                format!("entity {entity} does not exist"),
            ))
        }
    }

    /// The modules and their heat.
    pub fn modules(&self) -> Vec<ModuleStatus> {
        self.registry
            .modules
            .iter()
            .map(|(slot, m)| ModuleStatus {
                slot: *slot,
                name: m.name.clone(),
                generation: m.generation,
                hot: m.hot,
                counts: (m.systems.len(), m.behaviours.len(), m.commands.len()),
            })
            .collect()
    }

    /// Recompiles a module hot (Debug IL, EnC) or cold (optimized), at L3 cost.
    pub fn set_heat(&self, slot: u32, hot: bool) -> bool {
        // SAFETY: plain call into the runtime.
        unsafe { (self.api.set_heat)(slot, u32::from(hot)) == 0 }
    }

    /// A runtime counter (`abi::DIAG_*`).
    pub fn diag(&self, key: u32) -> u64 {
        // SAFETY: plain call into the runtime.
        unsafe { (self.api.diag)(key) }
    }

    /// True while the compile server is compiling (tick threads are boosted).
    pub fn compiling(&self) -> bool {
        self.ctx.boost.active()
    }

    /// Stops the compile client, unloads every module and returns the number of
    /// generations that failed to unload (leaks).
    pub fn shutdown(mut self) -> usize {
        self.stop()
    }

    fn stop(&mut self) -> usize {
        if self.shut_down {
            return 0;
        }
        self.shut_down = true;
        // SAFETY: plain call into the runtime; it stops every C# thread that could call back.
        let leaked = unsafe { (self.api.shutdown)() };
        usize::try_from(leaked).unwrap_or(0)
    }
}

impl Drop for ScriptHost {
    fn drop(&mut self) {
        self.stop();
        // A .NET runtime cannot be unloaded from a process (JIT or NativeAOT), so the
        // library stays mapped; C# no longer holds a live pointer to `ctx` after `stop`.
    }
}
