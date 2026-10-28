# ara-play-host

The owner side of Play: a `ara_host::Service` serving `play.*` (MCP `ara_play`) and
`test.run` (MCP `ara_test`). Registered by the `ara` daemon (`ara serve`, `ara mcp`
when it becomes the daemon) and by the editor (`Editor::open`). Depends on ara-host, ara-play, ara-asset, ara-runtime
(component schemas only at runtime; no simulation runs here).

## Public API

- `game_ui` packages `ui/game.ui`, its stylesheet and binding/resource manifests;
  resource paths stay inside the project and fonts/images travel with Play snapshots.
  Failed optional resource reads travel as nonfatal diagnostics, with original
  project-relative paths preserved for the player and editor console.

- `PlayService::new(root, PlayConfig).with_headless_automation()` enables daemon
  run/capture; native editors keep their render-boundary capture service.
  `PlayConfig::for_project(root)` finds
  `ara-player`/`ara-import-worker` next to the current executable (or `ARA_PLAYER`,
  `ARA_IMPORT_WORKER`), the .NET host (`ScriptPaths::locate`: `ARA_DOTNET_ARTIFACTS`
  or the nearest `dotnet/artifacts/bin`; installed tools beside the executable take
  precedence); two warm headless players, no warm window. Surface attachments
  prewarm a bounded 64x64 Low graphics presenter before hello, without probe GI.
- `open_project(root)`: opens a project with the engine schema world and commands
  (`ara_runtime::engine`) for a host before `Host::open`; clashes become diagnostics.
- `pool::{Pool, PoolConfig, Player}`: pre-spawned single-use players (S5), token-
  checked hellos, heartbeat watchdog (`WATCHDOG` = 5 s), fault classification.
  `Pool::with_scripts` passes module settings for pre-warm before the ready hello.
  Local crash artifacts go beside player logs in `crashes/`; a clean window close
  stops and reaps the whole Play world without reporting a crash. Launches run on
  threads (CreateProcess costs 10-200 ms) and early exits back refills off (0.5 s
  doubling to 30 s); `tests/pool.rs`. The host tick never builds the pool or the asset
  pipeline inline: both build on threads (`pool_now`/`assets_now`); Play waits.
- `assets::Assets`: every `*.import.json` registered with `ara_asset::AssetPipeline`
  (imports in worker processes, non-progressive); `ready_pins` snapshots completed
  persisted generations immediately, so live Play starts with what's available.
  `synchronize` follows in-memory import document revisions, including imports made
  after the pipeline started. First generations stream in while Play runs;
  failed cooks retain the last good generation and report their existing diagnostic.
- `package_scene_with_progress`: export snapshots wait on completed-cook progress,
  not a total deadline. `ARA_EXPORT_COOK_STALL_SECONDS` defaults to 600; completion
  counts and five-second heartbeats flow to the export job, pending IDs name stalls.
- `package_scene_for_export` retains that cook/progress contract while taking an
  explicit shipping script selection, without discovering dev hosts or compilers.
- `PlayService::published` / `PublishedAssets::for_project`: acquire a read-only
  project attachment once. Frame loops read its atomic revision; `snapshot` never
  waits and retains immutable generation IDs, kinds and FileCache provenance.
  Readers cannot initialize/cook/import; only completed owner boundaries publish.
  `progress_revision`/`progress` is the same pattern for `ImportProgress` (done/total,
  pending and failed IDs): published at pipeline start and when a source changes state.
  One `Publisher` per process owns a project's feed (the first; export packaging's own
  `Assets` stays silent until it is dropped); snapshot revisions only grow per feed.
  `cargo test -p ara-play-host --test published` holds a real import pending while
  checking completed generations, retained-snapshot immutability, progress and that
  export packaging leaves the feed alone.
- `codes`: ARA7300 play, 7301 player fault, 7302 unknown world, 7303 assets (real
  import failures only), 7304 test failed, 7305 bad request, 7306 info: a sidecar
  recorded an older importer; it was upgraded (typed `SidecarCommand::Version`
  transaction, canonical write) and reimported, 7307 embedded surface failed.
  Local range; reserve centrally later.
- `embed` (D19 Game view): `PlayService::surfaces() -> Surfaces`, taken before the
  host registers the service (the editor's in-process API, no JSON): `poll()` once
  per frame after the host tick, `surfaces(world) -> Vec<SurfaceState { player,
  generation, size, surface: &SharedSurface, latest }>`, `take(world, player)` (the
  caller holds it), `release(world, player, frame)` after its GPU stopped sampling,
  `device(world, player, Vec<DeviceEvent>)`, `resize(world, player, w, h)` (async; a
  new generation arrives through `poll`), `take_timings()`. `EmbedParams`,
  `DeviceEvent`, `method` (wire names mirrored by ara-player; its unit test checks).

## Methods

`run {scene?,ticks?|duration?,seed?,tickHz?}` runs a fresh headless world and returns
tick/hash, logs, events and diagnostics. `capture {scene?,camera?,path?,width?,height?,preset?,ticks?,seed?,marks?}`
captures the same snapshot offscreen to PNG. Both stop and reap their own players
before returning, including on failure; they never create windows.

`play.start {scene?, mode local|host|dedicated, clients, window, embed?: {width, height,
maxFps?}, startPaused, seed, tickHz, hotReload}` → `{world: "play:N", players[{role,
pid, started, surface?}], bootMs, prepareMs, transport}`; `device {player, events}`
(raw input to one embedded player), `resize {player, width, height}` → `{surface}`;
`stop`, `pause`, `resume`, `step`, `until`, `input`, `timescale`, `inspect`, `read`,
`patch` (acked after the applying tick), `capture` (hash, optional labelled PNG),
`watch`, `record`, `recording` (save), `replay`, `diff`, `keep`, `restart`
(`keepState` resumes from the current tick), `debug.crash`, `pool`, `status`,
`captureAudio` (offline render through the play audio path to an analysed WAV, with
how each source is heard), and C#:
`command`, `commands`, `events`; `script.write` (MCP `ara_play` action `write`). `world` defaults to the most recent world (editor
toolbar bindings rely on it). Pause/resume/step/input/patch/timescale broadcast to
every player of a world; reads go to the authority.

`background:true` returns `{world,status:"starting",players:[],progress}` immediately.
`play.status.worlds` reports starting/stopping/failed or running, with startup
`progress {phase,completed,total,elapsedMs}`. Stop cancels queued/active preparation
without joining it; omitted/false keeps the synchronous world-ready response.

- `play.keep`: diff → drop `driven` values unless `includeDriven` → `prev` check
  against the current scene (conflicts reported, not applied) → one `property.set`
  batch through `Cx::apply` (agent edits enter review as usual).
- Faults: exit code + `ARA_PLAYER_CRASH` stderr line + 5 s silence (extended by
  announced busy phases; player liveness runs on an independent IO thread) →
  `player_panicked | player_crashed | player_unresponsive | player_exited`, logged with a
  `play.restart` fix; `play.status` holds the structured fault.
- `play.stop` removes the active world and returns `status: "stopping"` immediately.
  An owned worker sends STOP and reaps its children; `play.status.stopping` lists
  outstanding shutdowns. Host ticks only join workers that have already finished.
- `test.run {steps, seed, determinism}` runs a spec (`spec.rs`: input, step, until,
  expect, patch, watch, capture, hash) in fresh headless players; `determinism` runs
  it twice in separate processes and requires equal final hashes.

- C# (`scripts.rs`): the shared D30 inventory publishes gameplay schemas before
  validation and sends private module inputs to players (`PlayScene::scripts`).
  `script.write` prepares a `source.replace` transaction on a worker, makes worlds
  claim agent paths (`play.compile`), then waits for materialization. Player `play.event`s: reloads and
  diagnostics (buffered for `play.events`, logged), `types` (the owner keeps each
  module's private `.ara/scripts/` schema current, canonical), `restart`
  (L4: resume a fresh player from `play.snapshot`). The C# catalog is published to the
  host (`Cx::publish_commands("play", ..)`) on start, after reloads and on stop.

## Gotchas

- Host calls are serialized on the daemon thread; long steps block other calls.
  Players heartbeat between 30-tick chunks so the watchdog never kills a busy step.
- Background Play uses placeholders until first persisted generations arrive, even with
  `hotReload:false`; subsequent reimports obey `hotReload`. Progressive previews
  remain disabled: players only load final cooks already persisted on disk.
  Synchronous starts wait for required import pins before returning world-ready.
- `host`/`dedicated` worlds are replicated (`ara-runtime-net`): the authority starts
  first, each client then gets its join from the authority (`play.join`) and connects
  through its own proxy (`conditions[i]` simulates the link). `play.capture`,
  `play.read` and `play.inspect` take `player` (0 = authority, 1.. = clients).
- Players inherit the daemon's environment: SDL3.dll (ara-platform) on PATH, and in
  debug builds the Vulkan validation layer (`VK_LAYER_PATH`) for captures/windows.
- Embedded worlds: headless pool players get `play.embed` before `play.start`
  (dedicated servers get no surface). The service pulls each ring's unnamed NT
  handles out of the child (`DuplicateHandle`; the child never holds a handle to
  the owner) and owns them until stop/restart/resize; `Surfaces` owns its own
  duplicates. Frames and new generations are read on host ticks; without a
  `Surfaces` handle frames go straight back. Taking `surfaces()` again makes every
  player start a fresh generation, so slots a lost handle held come back.

## Testing

Unit coverage lives in `ara-play`; the end-to-end gate is `ara-player`'s
`tests/scenario.rs`. `cargo clippy -p ara-play-host --all-targets -- -D warnings`.
