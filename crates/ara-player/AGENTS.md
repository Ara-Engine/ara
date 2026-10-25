# ara-player

The standalone game and S5 Play process. The edit-world owner (`ara` daemon via
`ara-play-host`, or the editor) pre-spawns Play into a single-use pool; it connects over
localhost WebSocket with the session token (`ARA_PLAY_TOKEN`), idles at zero CPU, and
on `play.start` maps the shared-memory snapshot, builds an `ara_runtime::Runtime` and
plays: windowed through SDL3 (`--window`) or headless. It exits on `play.stop`.

## Command line

`ara-player --owner <port> [--name slot] [--window] [--size 1280x720] [--gpu-cache dir]
[--quality auto|low|medium|high|epic] [--ray-tracing[=true|false]]`.
Without `--owner`, loads `ara-launch.json` beside the executable (or `--root DIR`),
reads the authored PlayScene/cooked generations from game.arapack, and opens a game
window. `--headless [--capture PNG] [--ticks N]` captures without a window;
`--frames N` bounds a windowed probe. Shipped assets never discover or import source
files. The same World, input mapper, animation/physics and presenters run both modes.
`--offscreen --frames N` keeps the continuous render/present path hidden (perf mode,
`--perf-output`, implies it; reports label `hiddenWindow`, no compositor latency).
`--headless` is the separate still-capture path. Owner-mode probes honour `--offscreen`;
`ARA_PLAY_OFFSCREEN=1` adds it to pooled windowed players (hidden entry-latency runs).
`--read entity.Component.field` adds a value to `ARA_RESULT.reads` after simulation;
repeat it to inspect shipped C# or Rust component fields. Shipping hosts resolve their
NativeAOT metadata against the export folder; development host paths are rejected.
Only pooled Play requires the owner token. `build.rs` embeds the Ara
exe icon (`assets/brand/ara.ico`); ara-platform sets the window icon.

Build with `--features tracy` to record literal CPU zones across Play world build,
extraction, native instance updates, GPU recording/timeline waits, presentation
and audio. The optional client starts in the executable; normal builds erase zones.
`tests/entry_latency.rs` (ignored real-project probe): set `ARA_PLAY_ENTRY_PROJECT`,
`ARA_BENCH_PLAYER` (optimized player) and `CARGO_BIN_EXE_ara-import-worker`; it reports
three warm sessions. `ARA_PLAY_ENTRY_WINDOW=1` = windowed, `_BACKGROUND=1` = nonblocking
start API, `_COLD=1` skips copied caches. Surface-attached pools prewarm a 64x64 Low
presenter before hello and reuse its device at embed; surface-less services allocate no GPU.

## Modules

- `packed.rs` and `standalone.rs`: the authored exported world and immutable pack source;
  ara-pack's bounded `native-decode` is linked into this player and its runtime templates.
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
- `link::Pump` owns Play transport on a dedicated heartbeat thread, so C# calls,
  simulation and uploads cannot suppress liveness. Bounded queues preserve order.
  `tests/watchdog.rs` holds simulation past the watchdog, then checks real silence.
- Import-state notifications style first-cook bounds translucent grey, failed or
  missing sources magenta, and retain last-good models during reimport. Native
  root/foliage arrivals fade over twelve presented frames; captures finish the fade.
- Startup enters `ara-crash` monitor mode before parsing player arguments; normal
  players retain a `Capture` guard for native minidumps and panic reports. Dumps go
  under the pool logs' `crashes/` directory (`ARA_CRASH_DIR` for direct probes).
- Pools pass `ARA_PLAY_SCRIPTS`; modules and their empty ECS world are warmed before
  hello, then handed to Play without rebinding component IDs or running gameplay early.
- Windowed frames include GPU-composited foliage/VFX and a native 2D path without
  pixel readbacks. `presented: true` captures read the composited 3D output.
- Windowed, embedded and headless presentation share the native raster view on
  every GPU. Project `settings.rendering` travels in PlayScene; `--quality` and
  `--ray-tracing` override it. Experimental RT requires an explicit opt-in and a
  capable logical device, selected before warm-up (the pool reads project policy).
  Preset auto-detection otherwise selects raster tiers. Resolve
  terrain/prefab assets at content changes, never on ordinary instance motion.
- D19 exports `Presenter::output()` at `output_size()`, including retained last-good
  output; `renderer()` owns the shared GPU but is only the bootstrap raster view.
- The first window frame announces bounded GPU warm-up grace; completion restores
  the normal watchdog immediately, after scene-dependent pipelines are prepared.
- The first presented frame (windowed or embedded) announces `busy("upload")`: real
  scenes upload for seconds, and owner calls (`play.commands` right after start)
  otherwise wait behind the upload. Independent liveness still runs throughout.

## Semantics

- Patches apply at the next tick boundary. Running: acked after that tick. Paused:
  the player steps exactly one tick to reach it, then acks.
- Device input and agent `play.input` both go through `Session::inject`, so they are
  recorded and replayable. Local and listen-host players are input owner 1; clients
  are players 2.. (movement, orbit, sprint and jump are predicted through `ara-runtime-net`).
- Headless players without a Surfaces attachment create a GPU device only on their
  first `play.capture` with a path; attached pools prewarm the bounded graphics view.
- Replay files carry portable snapshots (native physics checkpoints stay in-process):
  `play.replay` verifies exactly from recordings started at tick 0 (the fresh build);
  a mid-play recording diverges at its first tick once bodies are in contact.
- Windowed frames extract once, then present on the frame thread while a scoped worker
  runs acoustics and voice updates (they only read the simulation between ticks).
- Streamed worlds settle in headless players (reproducible arrivals) and stream live in windowed ones.

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
`tests/two_d.rs` plays the ara-render2d platformer (worker-imported atlases, 2D player, marked capture).
`tests/mixed.rs` plays the demo with its campfire effect and an agent-attached sound:
`play.captureAudio` (ara-runtime-audio: mixer, room reverb, acoustics against Jolt,
audibility report) and a VFX-composited capture. Windowed players play live audio
through SDL (`AUDIO_WATERMARK` queued frames); without a device play stays silent.
`tests/streamed_music.rs` imports a 181-second score through MCP and captures its
headless music playback deterministically. It also renders past the end and loops
through bounded producers, checking aggregate queue accounting and late audibility.
`tests/model_authoring.rs` selects source kit nodes through MCP and captures lamp
factor edits, source-pivot changes and enable/disable transitions in headless Play.
`tests/csharp.rs` covers published types, C# commands/hot reload and stateful restart;
`tests/valley.rs` covers networked C#, terrain/foliage, streamed cells and audio.
Both build .NET. Valley fixtures come from `ara-runtime-world`'s `valley` test.
`tests/pending_game.rs` captures pending, failed and published models in a real child.
`tests/embed.rs` (ignored; `--ignored`) is the D19 gate: the demo embedded through
`PlayService::surfaces`, 90 frames imported on a second device of the test process
(not black, close to a presented capture), W via `play.device` walks the Fox, typed
and JSON resizes (new generations), stop; prints latency and per-frame cost.
`cargo clippy -p ara-player --all-targets -- -D warnings`.
`tests/prerequisites.rs` probes missing Vulkan/.NET startup diagnostics and doctor
hints. Missing model sources stay playable with ARA7310 placeholders (`tests/pins.rs`).
