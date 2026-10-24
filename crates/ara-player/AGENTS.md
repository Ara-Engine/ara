# ara-player

The S5 play process. The edit-world owner (`ara` daemon via `ara-play-host`, later the
editor) pre-spawns it into a single-use pool; it initializes, connects back over
localhost WebSocket with the session token (`ARA_PLAY_TOKEN`), idles at zero CPU, and
on `play.start` maps the shared-memory snapshot, builds an `ara_runtime::Runtime` and
plays: windowed through SDL3 (`--window`) or headless. It exits on `play.stop`.

## Command line

`ara-player --owner <port> [--name slot] [--window] [--size 1280x720] [--gpu-cache dir]`.
Never run by hand: it needs an owner and the token. `build.rs` embeds the Ara
exe icon (`assets/brand/ara.ico`); ara-platform sets the window icon.

## Modules

- `main.rs`: pool lifecycle, the loop (link polling, host-time ticking with bounded
  catch-up, deferred patch acks, 250 ms `play.status` heartbeats, presentation),
  method dispatch.
- `net.rs`: `Play` (derefs to the runtime in every role): `Local`, `Host` (ara-net
  server owns the runtime and steps it; `play.join` issues client tokens) or `Mirror`
  (a client's never-stepped presentation world). Networked processes keep only their
  own player's inputs; step/until/pause/patch/replay are local-only.
- `world.rs`: one world: build from `PlayScene` (pinned asset generations from the
  content-addressed cache), step/until in 30-tick chunks (heartbeats between), input,
  inspect, `read_path` (`entity.Component.field`, `x/y/z/w`, `r/g/b/a`), patch (edits →
  baseline-checked ops at the next tick), capture, watch, record/replay, assets.
- `device.rs`: windowed players: SDL window, GPU device and pipelines warmed with a
  one-box frame before Play, SDL events → `input::Mapper` (Esc closes), presentation
  and swapchain recreation, FPS/p95 frame time for status.
- `input.rs`: `DeviceEvent` (key, mouseButton, mouseMotion, focusLost) and `Mapper`,
  the one mapping windowed and embedded players share (WASD/arrows Move, Space Jump
  once per press, Left Shift Sprint, right-drag Look; focus loss releases all).
- `embed.rs`: embedded players (D19): `play.embed {width, height, maxFps?}` before
  `play.start` → headless presenter + `ara_gpu` `ExportRing` (3 slots); frames are
  rendered when due (frame cap) and a slot is free, copied, fenced, then announced
  (`play.frame`); `play.release`, `play.device`, `play.resize` (request: answers the
  surface; notification: coalesced, answered with `play.surface`).
- `crash.rs`: panic hook + Windows unhandled-exception filter + no WER dialog; one
  `ARA_PLAYER_CRASH {json}` stderr line per fatal path. `play.debug.crash` triggers a
  panic, a real `STATUS_ACCESS_VIOLATION` (RaiseException) or a hang.
- Startup enters `ara-crash` monitor mode before parsing player arguments; normal
  players retain a `Capture` guard for native minidumps and panic reports. Dumps go
  under the pool logs' `crashes/` directory (`ARA_CRASH_DIR` for direct probes).
- Pools pass `ARA_PLAY_SCRIPTS`; modules and their empty ECS world are warmed before
  hello, then handed to Play without rebinding component IDs or running gameplay early.
- Windowed frames include GPU-composited foliage/VFX and a native 2D path without
  pixel readbacks. `presented: true` captures read the composited 3D output.
- The first window frame announces bounded GPU warm-up grace; completion restores
  the normal watchdog immediately, after scene-dependent pipelines are prepared.
- The first presented frame (windowed or embedded) announces `busy("upload")`: real
  scenes upload for seconds, and owner calls (`play.commands` right after start)
  would otherwise hit the 1 s watchdog mid-upload.

## Semantics

- Patches apply at the next tick boundary. Running: acked after that tick. Paused:
  the player steps exactly one tick to reach it, then acks.
- Device input and agent `play.input` both go through `Session::inject`, so they are
  recorded and replayable. Local and listen-host players are input owner 1; clients
  are players 2.. (movement, orbit, sprint and jump are predicted through `ara-runtime-net`).
- Headless players create a GPU device only on their first `play.capture` with a path.
- Replay files carry portable snapshots (native physics checkpoints stay in-process):
  `play.replay` verifies exactly from recordings started at tick 0 (the fresh build);
  a mid-play recording diverges at its first tick once bodies are in contact.
- Windowed frames extract once, then present on the frame thread while a scoped worker
  runs acoustics and voice updates (they only read the simulation between ticks).
- Streamed worlds settle in headless players (reproducible arrivals, agent tests) and
  stream live in windowed ones.

## Demo

`demo/` is a project: `levels/demo.scene` (ground with a navmesh surface, walls,
pushable crates, the skinned Fox with an idle/walk/run blend tree driven by the Jolt
character controller, four wandering drones on Detour crowds, sun + lamps + spot,
follow camera, dusk sky) and `assets/fox/Fox.glb` (Khronos sample, CC0/CC-BY 4.0,
see its LICENSE.md) with its import sidecar.

## Testing

Set the GPU environment first (`source ../../.sn/env.sh` or the ara-render `run.ps1`
pattern: SDL3.dll and the Vulkan SDK `Bin` on PATH, `VK_LAYER_PATH`, an empty
`VK_IMPLICIT_LAYER_PATH`). Then
`CARGO_BUILD_JOBS=2 cargo test -p ara-player -- --nocapture`: `tests/scenario.rs` drives
the demo over MCP end to end (pool boot, Move 120 ticks, grounded, labelled capture,
patch latency, pause, light patch, keep changes → document, determinism via
`test.run`, replay, Host + 2, panic/access-violation/hang isolation with restart) and
prints the measured numbers. It builds `ara-import-worker` if missing. `evidence/` holds
reviewed captures: walked with marks, relit after the patch, windowed, walked with
the campfire VFX, the 2D platformer with marks, and the mixed scene (image, WAV and
loudness analysis).
`tests/two_d.rs` plays the ara-render2d platformer sample (atlases imported through
the worker, the scout as a 2D player, an ara-render2d capture with marks).
`tests/mixed.rs` plays the demo with its campfire effect and an agent-attached sound:
`play.captureAudio` (ara-runtime-audio: mixer, room reverb, acoustics against Jolt,
audibility report) and a VFX-composited capture. Windowed players play live audio
through SDL (`AUDIO_WATERMARK` queued frames); without a device play stays silent.
`tests/csharp.rs` (builds `dotnet/Ara.slnx`) plays a demo copy with the Outpost C#
module (`demo-scripts/Outpost`: Door, UseMedkit, Gravity/Movement): published types,
agent-added C# components, `play.command`, an agent `script.write` hot reload while
playing, `restart {keepState}`. `ARA_UPDATE_DEMO=1` refreshes `Outpost.types.json`.
`tests/valley.rs` (builds .NET) plays `levels/valley.scene` networked (host + 1):
terrain + foliage, the 4-cell streamed world with baked acoustics, the
motion-matched Fox walking into the next cell, the guard NPC, campfire VFX + sound,
music and agent-added C# gameplay; captures from both processes, audio, timings.
The Valley files are generated by `ara-runtime-world`'s `valley` test.
`tests/embed.rs` (ignored; `--ignored`) is the D19 gate: the demo embedded through
`PlayService::surfaces`, 90 frames imported on a second device of the test process
(not black, close to a presented capture), W via `play.device` walks the Fox, typed
and JSON resizes (new generations), stop; prints latency and per-frame cost.
`cargo clippy -p ara-player --all-targets -- -D warnings`.
`tests/prerequisites.rs` probes missing Vulkan/.NET startup diagnostics and doctor
hints. Missing model sources stay playable with ARA7303 placeholders (`tests/pins.rs`).
