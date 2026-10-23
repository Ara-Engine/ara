# ara-runtime-script

C# gameplay in the runtime: `ScriptPlugin` (an `ara_runtime::RuntimePlugin`) runs
ara-script modules on the ara-sim world inside `ara-player`. Depends on ara-script,
ara-runtime, ara-sim. The .NET side lives in `dotnet/`.

## Public API

- `ScriptPlugin::new(ScriptSettings { host_dir, compile_server, modules }) -> (plugin,
  Receiver<ScriptEvent>)`; pass the plugin to `Runtime::new`.
- `ScriptEvent`: `Reload(ReloadReport)`, `Diagnostic`, `RestartRequired` (L4),
  `Types(path → *.types.json document)` after load and after each reload.
- Between ticks (the player's RPC handlers): `execute(world, name, params)` runs a C#
  `[Command]` as one undoable step (document addresses like `e_fox` become C# entity
  values) and returns `ScriptHost::describe`; `catalog(world)`; `read(world, entity,
  name)`; `request_compile(world, module, paths)` claims an agent edit;
  `module_of(settings, path)`.
- `ScriptProvider`: the `scripts` state provider (C# components by entity identity and
  C# resources, in snapshots and state hashes).

## How it plugs in

1. `prepare`: `ScriptHost::start` on the empty simulation world (engine binding:
   `ara_components::Transform`), `load_modules`, then the module types join the
   runtime schema, so scene documents may carry C# components.
2. `register`: each C# component becomes an ara-sim external component
   (`SimRegistry::register_external`): documents validate against its schema type, the
   projection queues values in `ExternalWrites`, the host owns the column.
3. `install(Gameplay)`: the host moves into the world (`Scripts` resource, reached only
   through exclusive access); the `scripts` exclusive system applies queued values and
   runs `ScriptHost::run_tick` (C# systems in `(Order, name)` order, then behaviours).
4. `before_tick`: `tick_boundary` (the only place code changes); reports, diagnostics
   and types go out on the channel; `RELOAD_L4` asks the owner for a restart, which
   resumes a fresh player from `play.snapshot` (state kept).

The play service (`ara-play-host`) finds modules (folders with `ara.module.toml`),
writes each module's generated `<Name>.types.json` from `Types` events (so the
editor and `ara check` validate C# components without .NET) and routes agent source
writes (`script.write`) through `request_compile(.., Agent)` before writing.

## Testing

`cargo test -p ara-player --test csharp -- --nocapture` (builds `dotnet/Ara.slnx`):
Outpost module (`crates/ara-player/demo-scripts/Outpost`) in a copy of the demo,
agent adds C# components, Gravity/Movement/Door run, `game.UseMedkit`, an agent edit
hot-reloads while playing, restart keeps C# state. The gate checks the checked-in
`Outpost.types.json` (`ARA_UPDATE_DEMO=1` regenerates it).

## Gotchas

- One .NET runtime per process: pooled players are single-use, so one host per world.
- Entities spawned by C# at runtime (no `Identity`) are not captured in snapshots yet.
- C# commands run immediately between ticks and are not part of replays yet.
- `Scripts` is `Sync` by an explicit `unsafe impl`: never add a `Res<Scripts>` system.
