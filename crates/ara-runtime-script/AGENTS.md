# ara-runtime-script

C# gameplay in the runtime: `ScriptPlugin` (an `ara_runtime::RuntimePlugin`) runs
ara-script modules on the ara-sim world inside `ara-player`. Depends on ara-script,
ara-runtime, ara-sim. The .NET side lives in `dotnet/`.

## Public API

- `ScriptPlugin::new(ScriptSettings { host_dir, compile_server, modules }) -> (plugin,
  Receiver<ScriptEvent>)`; pass the plugin to `Runtime::new`.
- `ScriptHostConfig::{Development, NativeAot}` selects the host; `ScriptPlugin::shipping`
  takes an export-local `scripts.json`. It validates ABI/layout before loading, then
  registers statically linked modules through the same gameplay ABI and snapshots.
  No CoreCLR, Roslyn, module folders, watcher or hot reload is used in shipping.
- `ScriptPlugin::prewarm(settings)` loads CoreCLR, Ara.Runtime, the compile server
  and modules on an empty world. `prepare` moves that world and its host together
  into the runtime; component IDs remain valid. No gameplay tick runs during warmup.
- `ScriptEvent`: `Reload(ReloadReport)`, `Diagnostic`, `RestartRequired` (L4),
  `Types(path → *.types.json document)` after load and after each reload.
- Between ticks (the player's RPC handlers): `execute(session, name, params)` runs a
  C# `[Command]` as one undoable step (document addresses like `e_fox` become C#
  entity values), records it with the next tick as a `script.command` boundary
  command (`COMMAND`, `ScriptCommand`; replays re-run it) and returns
  `ScriptHost::describe`; `catalog(world)`; `read(world, entity, name)`;
  `request_compile(world, module, paths)` claims an agent edit; `module_of(settings,
  path)`.
- `ScriptProvider`: the `scripts` state provider (C# components by entity identity and
  C# resources, in snapshots and state hashes).

## How it plugs in

1. `prepare`: `ScriptHost::start` on the empty simulation world (engine binding:
   `EngineBindings::builtin()`, currently `ara_components::Transform`), `load_modules`, then the module types join the
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

Entities C# spawns get a stable `Identity` right after they appear: systems and
behaviours `e_cs<tick>x<n>`, commands `e_cs<tick that records it>c<n>`, numbered in
content order (Transform + C# values) after earlier names with that prefix. Live
commands and their replays therefore name spawns identically, and ara-sim snapshots,
hashes and restores include them.

## Testing

`cargo test -p ara-player --test csharp -- --nocapture` (builds `dotnet/Ara.slnx`):
Outpost module (`crates/ara-player/demo-scripts/Outpost`) in a copy of the demo,
agent adds C# components, Gravity/Movement/Door run, `game.UseMedkit`, a recording
with a heal and a `game.DropSupplies` spawn replays exactly, an agent edit
hot-reloads while playing, restart keeps C# state (the spawned crate included). The
gate checks the checked-in `Outpost.types.json` (`ARA_UPDATE_DEMO=1` regenerates it).
`--test valley` plays the module networked in the integration level.

## Gotchas

- One .NET runtime per process: pooled players are single-use, so one host per world.
- All C# systems run in the one Gameplay slot (`scripts`); interleaving individual
  `ScriptSet`s with Rust systems needs ara-script to schedule sets separately.
- Replays apply recorded commands at their boundary, after that tick's inputs, live
  patches and streaming (live, they ran just before it).
- `component_names`, `read` and `apply_replica` bridge script-owned columns to
  networking; mirror writes merge received fields and preserve excluded fields.
- `Scripts` is `Sync` by an explicit `unsafe impl`: never add a `Res<Scripts>` system.
