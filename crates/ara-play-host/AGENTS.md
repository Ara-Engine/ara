# ara-play-host

The owner side of Play: a `ara_host::Service` serving `play.*` (MCP `ara_play`) and
`test.run` (MCP `ara_test`). Registered by the `ara` daemon (`ara serve`, `ara mcp`
when it becomes the daemon) and by the editor (`Editor::open`). Depends on ara-host, ara-play, ara-asset, ara-runtime
(component schemas only at runtime; no simulation runs here).

## Public API

- `PlayService::new(root, PlayConfig)`; `PlayConfig::for_project(root)` finds
  `ara-player`/`ara-import-worker` next to the current executable (or `ARA_PLAYER`,
  `ARA_IMPORT_WORKER`), the .NET host (`ScriptPaths::locate`: `ARA_DOTNET_ARTIFACTS`
  or the nearest `dotnet/artifacts/bin`); two warm headless players, no warm window.
- `open_project(root)`: opens a project with the engine schema world and commands
  (`ara_runtime::engine`) for a host before `Host::open`; clashes become diagnostics.
- `pool::{Pool, PoolConfig, Player}`: pre-spawned single-use players (S5), token-
  checked hellos, heartbeat watchdog (`WATCHDOG` = 1 s), fault classification.
- `assets::Assets`: every `*.import.json` registered with `ara_asset::AssetPipeline`
  (imports in worker processes); `pins` waits for published generations.
- `codes`: ARA7300 play, 7301 player fault, 7302 unknown world, 7303 assets (real
  import failures only), 7304 test failed, 7305 bad request, 7306 info: a sidecar
  recorded an older importer; it was upgraded (typed `SidecarCommand::Version`
  transaction, canonical write) and reimported. Local range; reserve centrally later.

## Methods

`play.start {scene?, mode local|host|dedicated, clients, window, startPaused, seed,
tickHz, hotReload}` → `{world: "play:N", players[], bootMs, prepareMs, transport}`;
`stop`, `pause`, `resume`, `step`, `until`, `input`, `timescale`, `inspect`, `read`,
`patch` (acked after the applying tick), `capture` (hash, optional labelled PNG),
`watch`, `record`, `recording` (save), `replay`, `diff`, `keep`, `restart`
(`keepState` resumes from the current tick), `debug.crash`, `pool`, `status`, and C#:
`command`, `commands`, `events`; `script.write` (MCP `ara_play` action `write`). `world` defaults to the most recent world (editor
toolbar bindings rely on it). Pause/resume/step/input/patch/timescale broadcast to
every player of a world; reads go to the authority.

- `play.keep`: diff → drop `driven` values unless `includeDriven` → `prev` check
  against the current scene (conflicts reported, not applied) → one `property.set`
  batch through `Cx::apply` (agent edits enter review as usual).
- Faults: exit code + `ARA_PLAYER_CRASH` stderr line + 1 s silence →
  `player_panicked | player_crashed | player_unresponsive | player_exited`, logged with a
  `play.restart` fix; `play.status` holds the structured fault.
- `test.run {steps, seed, determinism}` runs a spec (`spec.rs`: input, step, until,
  expect, patch, watch, capture, hash) in fresh headless players; `determinism` runs
  it twice in separate processes and requires equal final hashes.

- C# (`scripts.rs`): projects with module folders (`ara.module.toml`) send them to
  players (`PlayScene::scripts`); without a .NET build Play is refused with a setup
  diagnostic. `script.write` from an agent session makes every world claim the path
  (`play.compile`) before the file is written. Player `play.event`s: reloads and
  diagnostics (buffered for `play.events`, logged), `types` (the owner keeps each
  module's `<Name>.types.json` current, canonical, inside the project), `restart`
  (L4: resume a fresh player from `play.snapshot`). The C# catalog is published to the
  host (`Cx::publish_commands("play", ..)`) on start, after reloads and on stop.

## Gotchas

- Host calls are serialized on the daemon thread; long steps block other calls.
  Players heartbeat between 30-tick chunks so the watchdog never kills a busy step.
- The first Play of a fresh clone waits for the first cooks (progressive Fast first).
  Generations published during Play reach a world only with `hotReload`.
- Multi-instance without `ara-net`: every process runs the same deterministic world
  from the same snapshot and broadcast inputs; clients do the S5 UDP hello/welcome.
  `ara-net` replaces this with replication on the same roles.
- Players inherit the daemon's environment: SDL3.dll (ara-platform) on PATH, and in
  debug builds the Vulkan validation layer (`VK_LAYER_PATH`) for captures/windows.

## Testing

Unit coverage lives in `ara-play`; the end-to-end gate is `ara-player`'s
`tests/scenario.rs`. `cargo clippy -p ara-play-host --all-targets -- -D warnings`.
