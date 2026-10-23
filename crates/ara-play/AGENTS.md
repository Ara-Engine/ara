# ara-play

The play-mode wire contract between the edit-world owner (`ara` daemon, editor) and
`ara-player` processes (S5). No engine state; both sides depend on it.

## Public API

- `PROTOCOL` (`ara-play/1`), `HELLO`, `STATUS`, `CRASH_PREFIX`, `TOKEN_ENV`, and the
  `method::*` names (`play.start/stop/pause/resume/step/until/timescale/input/inspect/
  read/patch/capture/watch/record/recording/replay/diff/assets/debug.crash`).
- Params/results: `Hello`, `Start` (+ `SnapshotRef::{Shm, Inline}`, `Role`), `Started`,
  `Input`, `Step`, `Until` + `Compare`, `Stepped` + `Sample`, `Inputs`, `Inspect`, `Read`,
  `Patch` + `Set`, `Patched`, `CaptureParams`, `Captured` + `Mark`, `WatchParams`,
  `SaveReplay`, `Crash`, `Status`, `CrashReport`. All camelCase JSON.
- `PlayScene`: documents (`DocumentData`), project types, seed, tick rate, asset pins
  (`as_…` → 64-hex action digest), cache and nav-cache dirs. `encode`/`decode` (CBOR),
  `documents(&Workspace)`, `workspace()`. `hex`/`unhex` for digests.
- `shm::Section::{create, open, bytes}`: named pagefile-backed sections (Windows);
  other platforms return `Unsupported` and the owner sends `Inline` bytes.
- `link::Link`: non-blocking JSON-RPC 2.0 over a localhost WebSocket (`connect` for
  players, `accept` for owners); `poll`, `wait`, `request`, `notify`, `respond`, `flush`,
  `call`. `link::error(code, message, data)` builds error objects.

## Rules and gotchas

- Players connect back; they never listen. The token travels in `player.hello`, set
  through `ARA_PLAY_TOKEN` (never the command line). Section names embed part of it.
- Windows socket timeouts are ~15.6 ms; `Link` polls with 0.5 ms sleeps instead.
- Accepted sockets inherit the listener's non-blocking mode; `accept` resets it.
- Bump `PROTOCOL` only for incompatible changes; new methods are additive.
- `Role` matches `ara-net` launch roles (Host/Dedicated/Client) plus `Local`.

## Testing

`cargo test -p ara-play` (link round trip over a real socket, section by name).
`cargo clippy -p ara-play --all-targets -- -D warnings`.
