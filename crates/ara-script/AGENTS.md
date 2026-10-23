# ara-script

The C# host (D11, A13): C# gameplay code (components, systems, behaviours, commands)
on the Rust engine, with hot reload, NativeAOT for shipping and the `ara_run` agent
worker. The C# side lives in `dotnet/` (see `dotnet/AGENTS.md`). Depends on ara-schema,
ara-diag, ara-components, bevy_ecs 0.20, netcorehost, libloading, windows-sys.

## Public API

- `abi`: the one `#[repr(C)]` ABI, declared once with descriptor macros:
  `HostInit` (+ `HostApi`, Rust → C#) and `ScriptApi` (C# → Rust), `Tick`, `Frame`,
  `SystemCall`, `BehaviourCall`, `CommandCall`, `ReloadReport`, the `ABI_CONSTS`, and
  `layout_hash(engine_layouts)`. Entry point: `ara_script_init(*const HostInit, *mut ScriptApi)`.
- `csgen`: Rust → C# (`csharp(db, engine)`, `write`, `stale`, `blittable_components`):
  `Abi.g.cs`, `Components.g.cs` (engine components from the Schema DB with units,
  ranges, replication, defaults and docs as attributes/XML docs), `Units.g.cs`, `Codes.g.cs`.
- `ScriptHost` (per process; one .NET runtime):
  - `start(ScriptConfig, SchemaDb, &EngineBindings, &mut World)`;
    `ScriptConfig::jit(ara_host_dir)` (+ `compile_server`) or `::native_aot(lib)`.
  - `load_modules(world, &[ModuleSpec])`; `ModuleSpec::from_dir(dir)` reads `ara.module.toml`.
  - Per tick: `tick_boundary(world)` (applies staged reloads) then `run_tick(world, Tick)`
    (C# systems in parallel, then behaviours). `run_frame(world, Frame)` for `OnFrame`.
  - `execute_command(world, name, &params, caller) -> CommandRecord`, `undo(world, &record)`,
    `describe(&record)`, `command_catalog()` (ara-cmd `Info` shape, `scope: "play"`).
  - `schema()` (Rust + C# types: `reflection.json`), `read_component` / `insert_component`
    (JSON via canonical layouts), `take_diagnostics`, `take_cues`, `modules` (heat),
    `set_heat`, `diag(DIAG_*)`, `shutdown() -> leaked generations`.
- `EngineBindings` + `bind_component!(b, T as "Name" { fields })`: Rust `#[repr(C)]`
  components viewed by C#; checked against the schema's canonical layout at start.
- `run`: `RunPool` / `RunWorker` (spawn `Ara.RunWorker`, `run(code, RunLimits, &mut dyn CommandSink)`).
- `value`: JSON ↔ bytes for blittable schema structs; entities as `"12v1"`.
- `codes`: host diagnostics `ARA0900`–`ARA0999` (generated into C# `AraCodes`).
- `register_tick_thread()`: call from your task pool's `on_thread_spawn` if you build it yourself.

## How it fits together

- C# modules register back at every load/L3 through `HostApi::register_module`: their
  `types.json`-format types go into the Schema DB as dynamic types (module = module name),
  C# layouts are checked against `SchemaDb::struct_layout`, and bevy components are
  registered with `ComponentDescriptor::new_with_layout` (reused when the layout is
  unchanged; a changed layout gets a new id and the column is migrated field by field).
- C# systems: `ecs::system::ScriptQuery`, a custom `SystemParam` built from the declared
  columns/resources. It registers that access with bevy's scheduler, walks matched tables
  for column base pointers, marks written rows changed, splits tables into chunks and runs
  one `run_system` call per chunk on the compute task pool. Conflicting systems are
  ordered by `(Order, name)`. Structural ops go to per-chunk `CmdBuf`s applied by
  `SystemParam::apply` at bevy sync points.
- Behaviours and commands run with exclusive world access, published to C# as a `Scope`
  (`host/ctx.rs`); `World.Get<T>()` is only valid there. Commands journal first-seen bytes
  of every component they open for writing; rejection or a fault restores them.
- Rust never holds pointers into module code: systems/behaviours/commands are ids
  (`slot << 16 | index`) into C# tables swapped at tick boundaries.

## Testing

- `cargo test -p ara-script` builds `dotnet/Ara.slnx` (Release) once per test binary.
  - `generated`: C# generator snapshot + checked-in bindings fresh (`ARA_UPDATE_GENERATED=1`
    rewrites `dotnet/src/Ara.Runtime/Generated`; review the diff).
  - `host`: registration round trip, systems over chunks, behaviours, commands + undo,
    fault containment (prebuilt modules). `reload`: L1/L3 through the compile server.
  - `aot`: publishes `Outpost.Sample.Aot` with NativeAOT and runs it. `run_worker`: limits.
- Each CLR test file is its own process (one .NET runtime per process); tests in a file
  share it through `support::clr_lock()`.
- Latency: `cargo run -p ara-script --release --example reload_bench [-- --verbose]`.
  Hot-reload demo: `cargo run -p ara-script --example outpost`, then edit
  `dotnet/samples/Outpost.Sample/*.cs`.

## Gotchas

- One .NET runtime per process. `DOTNET_MODIFIABLE_ASSEMBLIES=debug` (EnC) is set before
  it starts; on non-Windows, start the host before other threads read the environment.
- bevy stores entity indexes complemented (`NonMaxU32`): raw bits 0 are never valid, so
  C# `default(Entity)` is null. Never interpret the bits beyond that.
- bevy 0.20 resources are entities: a dynamic resource needs an `IsResource` marker on its
  entity (see `registry::insert_resource_bytes`).
- Engine binding order is ABI: bind components in the order the C# was generated with
  (`csgen::blittable_components`, by name) or the layout hash rejects the runtime.
- Windows AMSI/Defender scans every new in-memory image: 60–200 ms per L3 load depending
  on load (S2 finding 4). A Defender exclusion for the editor removes it; it is the user's call.
- NativeAOT publish on this machine needs `vswhere.exe` on PATH (VS 2026's `vcvarsall`);
  `tests/support::publish_aot` adds the VS Installer directory.
- Per-frame code must not allocate or lock: chunk scratch (`Scratch`, `PhaseScratch`) is
  reused; the only mutexes are on diagnostic/report queues.

## Measurements (`reload_bench`, 7800X3D, 5.2k-line module, 20k entities)

Machine shared with other agents' builds (CPU 65–99%), so these are upper bounds:
L1 save→visible p50 26 ms, p95 61 ms; L3 p50 434 ms, p95 542 ms (≈200 ms emit +
≈150–200 ms AMSI on load). Tick p99 3.8 ms, max 8.5 ms during reloads.
