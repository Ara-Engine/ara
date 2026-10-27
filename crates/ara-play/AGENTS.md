# ara-play

The play-mode wire contract between the edit-world owner (`ara` daemon, editor) and
`ara-player` processes (S5). No engine state; both sides depend on it.

## Public API

- `PROTOCOL` (`ara-play/1`), `HELLO`, `STATUS`, `CRASH_PREFIX`, `TOKEN_ENV`, and the
  `method::*` names (`play.start/stop/pause/resume/step/until/timescale/input/inspect/
  read/patch/capture/watch/record/recording/replay/diff/assets/debug.crash`).
- Params/results: `Hello`, `Start` (+ `SnapshotRef::{Shm, Inline}`, `Role`), `Started`,
  `Input`, `Step`, `Until` + `Compare`, `Stepped` + `Sample`, `Inputs`, `Inspect`, `Read`,
  `Patch` + `Set`, `Patched`, `CaptureParams` (`presented`: the windowed path), `Captured` + `Mark`, `WatchParams`,
  `SaveReplay`, `Crash`, `Status`, `CrashReport`. All camelCase JSON.
- `PlayScene`: documents (`DocumentData`), project types, seed, tick rate, asset pins
  (`as_…` → 64-hex action digest), cache and nav-cache dirs, `imports` (imported
  asset ID → path, so cell cooking resolves references). `encode`/`decode` (CBOR),
  `documents(&Workspace)`, `workspace()`. `hex`/`unhex` for digests.
- `PlayScene.ui` embeds native markup/styles, typed ECS bindings, fonts/images,
  feedback counters and music routes. It carries presentation data, never game state.
- `shm::Section::{create, open, bytes}`: named pagefile-backed sections (Windows);
  other platforms return `Unsupported` and the owner sends `Inline` bytes.
- `link::Link`: non-blocking JSON-RPC 2.0 over a localhost WebSocket (`connect` for
  players, `accept` for owners); `poll`, `wait`, `request`, `notify`, `respond`, `flush`,
  `call`. `link::error(code, message, data)` builds error objects.


## C# messages

`play.command` (`Command`), `play.commands`, `play.compile` (`Compile`: claim agent
edits), `play.snapshot` (portable hex CBOR) and `Start.resume`; `play.event`
notifications (`Event`: `reload`, `diagnostic`, `restart`, `types`);
`PlayScene.scripts` (`ScriptScene`: host dir, compile server, module folders).
`ScriptScene.host` is typed: `Development` by default for older snapshots;
`NativeAot { manifest }` for exports. `ScriptScene::shipping` clears author paths;
the standalone entry calls `resolve_shipping(export_root)` before building a world.
## Rules and gotchas

- Players connect back; they never listen. The token travels in `player.hello`, set
  through `ARA_PLAY_TOKEN` (never the command line). Section names embed part of it.
- Windows socket timeouts are ~15.6 ms; `Link` polls with 0.5 ms sleeps instead.
- Player `link::Pump` owns the socket on a dedicated IO thread after hello, with
  bounded queues and 250 ms liveness independent of simulation/uploads. `Status`
  remains a simulation snapshot; `HEARTBEAT` carries only process liveness.
- `method::IMPORTS` carries read-only `ImportState` before Start and at changed
  progress revisions. Absent IDs are failed/unregistered; existing pins stay live.
- Accepted sockets inherit the listener's non-blocking mode; `accept` resets it.
- Bump `PROTOCOL` only for incompatible changes; new methods are additive.
- `Role` matches `ara-net` launch roles (Host/Dedicated/Client) plus `Local`.
- Networked roles: `Start.net` (`NetStart`: players, per-client `LinkConditions`, and
  for clients the `Joined` from the authority's `play.join`). The join token is private
  launch data: owners pass it only to that client's `play.start`, never into results.
  `Status.net` carries authority traffic or client join/prediction numbers.

## Testing

`cargo test -p ara-play` (link round trip over a real socket, section by name).
`cargo clippy -p ara-play --all-targets -- -D warnings`.
