# ara-script

The C# host (D11, A13): C# gameplay code (components, systems, behaviours, commands)
on the Rust engine, with layered hot reload (L1-L4), cross-module references, NativeAOT for
shipping and the `ara_run` agent worker. The C# side lives in `dotnet/` (see `dotnet/AGENTS.md`).
Depends on ara-schema, ara-diag, ara-components, bevy_ecs 0.20, netcorehost, libloading, windows-sys.

## Public API

- `abi` (ABI v2): `HostInit` (+ `HostApi`, Rust → C#) and `ScriptApi` (C# → Rust), `Tick`,
  `Frame`, `SystemCall`, `BehaviourCall`, `CommandCall`, `ReloadReport` (kind, escalation,
  origin, cascade size, per-phase µs), the `ABI_CONSTS` (`RELOAD_*`, `ESC_*`, `ORIGIN_*`) and
  `layout_hash(engine_layouts)`. Entry point: `ara_script_init(*const HostInit, *mut ScriptApi)`.
- `csgen`: Rust → C# (`csharp(db, engine)`, `write`, `stale`, `blittable_components`):
  `Abi.g.cs`, `Components.g.cs`, `Units.g.cs`, `Codes.g.cs`.
- `ScriptHost` (per process; one .NET runtime):
  - `start(ScriptConfig, SchemaDb, &EngineBindings, &mut World)`;
    `ScriptConfig::jit(ara_host_dir)` (+ `compile_server`) or `::native_aot(lib)`.
  - `load_modules(world, &[ModuleSpec])`: dependency order (`load_order`); cycles
    (`ARA0923`) and unknown references (`ARA0924`) are refused, the rest load.
  - Per tick: `tick_boundary(world) -> Boundary` (the only place reloads apply; `reports`)
    then `run_tick(world, Tick)`. `run_frame(world, Frame)` for `OnFrame`. `pending()`.
  - `request_compile(module, &paths, EditOrigin)`: never blocks. `EditOrigin::Agent` edits
    are coalesced and compiled behind human ones (D20); watcher events on paths an agent
    claimed this way count as agent edits.
  - `execute_command`, `undo`, `describe`, `command_catalog()` (ara-cmd `Info` shape, `scope: "play"`).
  - `schema()`, `read_component` / `insert_component` (JSON), `take_diagnostics`, `take_cues`,
    `modules` (heat), `set_heat`, `compiling()`, `diag(DIAG_*)`, `shutdown() -> leaked generations`.
- `ModuleSpec { name, dir, assembly, watch, hot, references, profile }` from `ara.module.toml`
  (`references = ["Core"]`, `profile = "gameplay" | "editor"`: the API allowlist, ARA05xx).
- `EngineBindings` + `bind_component!`, `run` (`RunPool`/`RunWorker`), `value`, `codes`
  (`ARA0900`–`ARA0999`), `register_tick_thread()`.

## Reload levels (A13, R10 D1)

- **L1** body edits of a hot module → EnC delta (no closures, state machines, constructors).
- **L2** additive edits → EnC delta: new methods/operators/non-auto properties, fields and
  auto-properties on classes (no initializers), consts, new non-generic non-partial types,
  lambdas inside new members. Gated on the runtime's EnC capabilities and an unchanged
  generator output. A delta the runtime rejects (`ApplyUpdate` throws) is rebuilt as L3
  (`ESC_RUNTIME_REJECTED`, `ARA0905`).
- **L3** anything else (signatures, struct fields, attributes, removals, Ara declarations,
  cold modules, debugger attached) → a new collectible ALC; columns migrate field by field.
- **L4** `RELOAD_L4` + `ARA0926`: engine bindings (`Ara.Runtime.dll`) changed on disk, or a
  retired generation stays alive (`ARA0927`). The last good code keeps running; restarting
  the play process from a snapshot is the play process owner's job.

## How it fits together

- C# registers each atomic reload through `HostApi::register_modules` (a JSON array, deps
  first): `host/register.rs` validates every descriptor against the view earlier ones
  produce, then commits them all, or nothing changes. Types go into the Schema DB as dynamic
  types, layouts are checked against `SchemaDb::struct_layout`, bevy components are reused,
  added, or re-registered and migrated.
- Cascades: an L3 of module A relinks each module referencing it (same image and EnC
  chain, new ALC bound to A's new generation) when A's public surface is unchanged, or
  rebuilds it (`ESC_DEPENDENCY`). All swap at one boundary (`ReloadReport::cascade`). If a
  dependent no longer compiles, nothing applies (`ARA0925` on both).
- C# systems: `ecs::system::ScriptQuery`, a custom `SystemParam` registering column/resource
  access with bevy's scheduler, one `run_system` call per chunk on the compute task pool
  (serial when writing a resource or taking `Commands`). Conflicts order by `(Order, name)`.
- Behaviours and commands run with exclusive world access published to C# as a `Scope`
  (`host/ctx.rs`). Commands journal first-seen bytes; rejection or a fault restores them;
  `undo` is all or nothing and converts saved bytes across layout migrations.
- Rust never holds pointers into module code: ids (`slot << 16 | index`) into C# tables
  swapped at tick boundaries.

## Integration seams (next task: ara-sim, ara-cmd, ara-host)

- ara-sim: call `tick_boundary` at the start of each fixed tick, then `run_tick`. The C#
  schedule is built in `ScriptHost::rebuild` (systems in `ScriptSet(id)` sets with ordering
  edges); interleaving into ara-sim's schedule means exposing those sets instead of running
  `self.schedule` (keep `rebuild` as the single place that turns the registry into systems).
- ara-cmd: merge `command_catalog()` into the registry; `execute_command`/`undo` already
  produce and revert a `CommandRecord` as one undoable step.
- ara-host / Engine Protocol: agent file writes call `request_compile(.., Agent)` before
  writing; forward `Boundary::reports` and `take_diagnostics()` as protocol events; `RELOAD_L4`
  tells ara-player to restart from a snapshot; `ara_run` goes through `run::RunPool`. D20's
  "apply agent changes when idle / on approval" is a hold on agent-origin updates in the C#
  pending queue (reports already carry `origin`); not built yet.

## Testing

- `cargo test -p ara-script` builds `dotnet/Ara.slnx` (Release) once per test binary.
  - `generated`: generator snapshot, checked-in bindings fresh (`ARA_UPDATE_GENERATED=1`).
  - `host`: registration, systems over chunks, behaviours, commands + undo, faults (prebuilt).
  - `reload`: v1 L1/L3 path. `levels`: L1/L2/L3 edits against a running game.
    `modules`: cascades, relink vs rebuild, breaking edits, cycles. `nondisruption`: the four
    D20 guarantees. `restart`: L4 (runs from a private copy of `Ara.Host`'s output).
  - `aot`: NativeAOT sample. `run_worker`: limits.
- Each CLR test file is its own process (one .NET runtime per process); tests in a file
  share it through `support::clr_lock()`. `support::Live` writes inline modules and ticks.
- Latency: `cargo run -p ara-script --release --example reload_bench [-- --edits 60 --verbose]`
  prints a p50/p95 phase table per level and the system CPU load during the run.

## Gotchas

- One .NET runtime per process. `DOTNET_MODIFIABLE_ASSEMBLIES=debug` (EnC) is set before
  it starts; on non-Windows, start the host before other threads read the environment.
- bevy stores entity indexes complemented (`NonMaxU32`): raw bits 0 are never valid.
- bevy 0.20 resources are entities: a dynamic resource needs an `IsResource` marker.
- Engine binding order is ABI: bind in the order the C# was generated with.
- Windows AMSI scans every new in-memory image (`LoadFromStream`): ~110–250 ms for a
  5.2k-line module, by content (random bytes of the same size: 5 ms), cached by content (a
  relink of the same bytes: ~1 ms). Loading from a file pays Defender's on-access scan
  instead (same cost). Only smaller modules, L1/L2, or a Defender exclusion (the user's call) avoid it.
- NativeAOT publish needs `vswhere.exe` on PATH (`tests/support::publish_aot` adds it).
- `HostCtx` is leaked (one per process): C# threads may call back after `shutdown`.
- Tags (empty structs) are 1 byte in C#; per-frame code must not allocate or lock.

## Measurements (`reload_bench`, 7800X3D shared with other agents' builds)

5.2k-line module, 20k entities, 60 Hz. MEASUREMENTS_PLACEHOLDER
