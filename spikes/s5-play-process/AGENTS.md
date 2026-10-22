# AGENTS.md: S5 play-in-a-child-process spike

Throwaway M0 spike (not a workspace member). It proves play mode can run in a child process and
still feel instant. Results and the proposed decision are in [`RESULT.md`](RESULT.md).

## Layout

| Path | Purpose |
|---|---|
| `src/model.rs` | Components (`SceneId`, `Label`, `Transform`, `Orbit`, `Tint`), the 60 Hz orbit system, `Sim`, canonical state hash |
| `src/registry.rs` | Component registry (schema-DB stand-in) and the `Op` vocabulary with automatic inverses |
| `src/scene.rs` | JSON scene document (`ara.scene/0`) and the deterministic generator |
| `src/snapshot.rs` | Columnar binary snapshot (`ARASNAP1`) |
| `src/shm.rs` | Windows named shared-memory section |
| `src/rpc.rs`, `src/protocol.rs` | JSON-RPC 2.0 over localhost WebSocket, blob frames, the typed `play.*` methods |
| `src/player.rs` | Child player: hello, idle, fixed-tick loop serving the live link |
| `src/crash.rs`, `src/gpu.rs` | Player crash reporters and forced crashes; optional Vulkan device init |
| `src/host/` | Editor host: player spawning and calls, suites (`boot`, `live`, `faults`), stats |
| `run.ps1` | Builds and regenerates `evidence/` |

## Commands

Run from this directory with `$env:CARGO_BUILD_JOBS = '4'`:

- `./run.ps1`: full evidence pass (about 90 s).
- `cargo test --release`: snapshot fidelity, op inverses and atomicity, ID ordering, determinism.
- `cargo clippy --release --all-targets -- -W clippy::unwrap_used -W clippy::undocumented_unsafe_blocks`
- `cargo fmt --all -- --check`

One suite at a time:
`./target/release/ara-s5-play.exe host --scene scenes/orbit-5k.json --out target/x.json --reps 5 --suites live`.
Suites: `snapshot,boot,live,keep,crash,multi`. `--gpu` adds Vulkan-initialising players to `boot`.

## Gotchas

- Every player the host starts is owned by a `Proc` that kills it on drop; a player also exits
  when its link closes. Check `tasklist /FI "IMAGENAME eq ara-s5-play*"` after interrupted runs.
- `ARA_S5_TRACE=1` prints spawn→hello per player; `ARA_S5_NAGLE=1` leaves Nagle on.
- Socket read timeouts on Windows round up to 15.6 ms; the player paces ticks with
  `thread::sleep`, never with socket timeouts.
- The shm transport is Windows-only here (`shm_open` + `mmap` is the POSIX equivalent).
- Scenes are generated into `scenes/` (gitignored); regenerate after changing the generator.
