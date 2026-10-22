# S5: Play mode in a child process

**PASS, 2026-10-23 (Windows).** Play runs in a separate player process and still feels instant. A pre-spawned (pooled) player reaches its first simulated tick **3.5 ms** after Play at 5k entities and **30 ms** at 50k. That is within 1–4 ms of simulating in the editor's own process. A cold spawn takes 16 ms (40 ms at 50k). A live patch round trip is **16.3 ms p95**. Crashes and hangs never reach the host. "Keep changes" produces an invertible op list. One server and two clients are all running 34 ms after Play cold, or 5 ms from the pool.

## Criteria

| Criterion | Target | Result (p50 / p95) | Verdict |
|---|---|---|---|
| Play → first tick, warm | < 1 s | Pooled player over shm: **3.53 / 4.27 ms** (5k), **30.20 / 37.88 ms** (50k). Cold paths also pass: cold process **16.36 / 33.20 ms**, never-executed binary **162 / 186 ms**, cold process that also creates a Vulkan device **238 / 534 ms**. | **PASS** |
| Live patch round trip | < 50 ms p95 | **8.87 / 16.32 ms** at 5k, **10.11 / 17.48 ms** at 50k, over 200 patches spread across the tick phase. Each patch was applied at the next tick boundary and acked after exactly one tick, 200/200. 0 read-back mismatches. | **PASS** |
| Host survives player crashes | survive, report, offer restart | Panic → `player_panicked` (exit 0x65, the player's message and location). Access violation → `player_crashed` (0xC0000005, faulting instruction address). Hang → `player_unresponsive` (killed after 1.05 s without a heartbeat). Each fault carries a `restart` action; taking it reaches tick 1 in ~17 ms. The host continued, and the edit-world hash was unchanged. | **PASS** |
| Keep changes as an op list | works | 5 selected entities give 14 preview ops (set, create and delete). Unticking the simulation-driven fields leaves 5, applied as one transaction in 0.03 ms. The inverse restores the original edit-world hash; redo reproduces the kept hash; 0 conflicts. | **PASS** |
| Three player processes together | launch + handshake | 1 server + 2 clients, with clients completing a UDP hello/welcome before reporting running: **33.70 / 35.12 ms** cold, **5.10 / 5.38 ms** pooled. The server saw both clients in 20/20 sessions. | **PASS** |

## What was built

One standalone binary, `ara-s5-play` (bevy_ecs 0.20, tungstenite 0.30, windows-sys, ash; no async runtime). It has four subcommands: `gen-scene`, `host`, `player` (spawned by the host, never run by hand) and `probe-timers`.

**Edit world.** A `bevy_ecs` world loaded from a canonical JSON scene file (`ara.scene/0`). It is a flat entity map keyed by base-36 short IDs, with keys sorted by `BTreeMap`/`serde_json::Value`. Every entity has a `SceneId` (the only address that crosses the process boundary) plus `Label`, `Transform` and `Tint`. 80% also have `Orbit`, which gives two archetypes. The generator is deterministic. Scenes: 5,000 entities (3.3 MB pretty JSON, parsed and spawned in 31 ms) and 50,000 (33.5 MB, 267 ms).

**Registry and ops.** A typed `PlayComponent` trait drives one type-erased `ComponentKind` per component. Each kind provides:
- JSON get and batch insert, validated by deserializing;
- a binary snapshot column;
- canonical hash bytes.

This is the A2 schema-DB stand-in. One `Op` vocabulary (`set`, `remove`, `create`, `delete`) carries `prev`/`snapshot`, so every op has an automatic inverse. `apply_ops` is atomic: the first failure rolls back and names the op index. The same code applies live patches in the player and "Keep changes" in the host.

**Snapshot transports.** The host encodes the edit world when Play is pressed. There are three transports:
- **wsJson:** the canonical JSON document, in a WebSocket binary frame.
- **wsBinary:** the `ARASNAP1` columnar snapshot, in a WebSocket binary frame. The snapshot is entities in stable-ID order, then per component a presence bitset plus packed values.
- **shm:** the same bytes in a named, pagefile-backed Windows section (`CreateFileMappingW`). The player maps it read-only by name.

The player keeps the snapshot as its diff baseline. For shm it simply keeps the mapping.

**Live link.** JSON-RPC 2.0 in WebSocket text frames over localhost TCP, the A5 shape. The host listens on an ephemeral port, and the player connects back to it, as Godot's game connects to the editor's debugger port 6007. Details:
- A per-session 128-bit token is checked in `player.hello`.
- `TCP_NODELAY` is set.
- Bulk data uses binary frames that start with an 8-byte blob ID, referenced from the JSON-RPC message that follows.
- Methods are typed through a `Method` trait (name, params, result). See the [M2 shape](#proposed-process-and-protocol-shape-for-m2-play-mode).

**Player loop.** The player sends hello, then idles, blocked on its socket at zero CPU. That idle state is the pool state.

On `play.start` it decodes the snapshot and runs tick 1 immediately, then answers. The answer is the "first tick visible in the host" event. From then on the loop runs at 60 Hz:
- Each tick is paced by `thread::sleep`, with wakeups at most 1 ms apart. Between ticks, inspect, capture and diff requests are answered at once.
- Patches queue until the next tick boundary. They are applied atomically, the tick runs, and the ack carries the read-back value.
- `pause`/`resume`/`step` control time. `step` runs N ticks as fast as possible.
- A `play.status` heartbeat goes out every 250 ms.
- Catch-up is bounded to 5 ticks.

**Crash isolation.** The player installs:
- a panic hook;
- `SetUnhandledExceptionFilter`;
- `SetErrorMode(SEM_NOGPFAULTERRORBOX)`.

Each crash path writes one `ARA_PLAYER_CRASH {json}` line to stderr. The host pipes stderr and classifies the exit code. A watchdog kills a player that sends no heartbeat for 1 s. `play.debug.crash` forces a panic, a write to an unmapped address, or an infinite sleep.

**Multi-client.** A server-role player binds UDP and reports the address in its hello. Clients receive `connectTo` in `play.start` and finish a UDP hello/welcome with the server before answering. All three map the same shm snapshot.

**Optional `--gpu-init`.** The player creates a Vulkan 1.3 instance and device at startup. This stands in for the scene-independent startup a real player pays.

## Exact commands

PowerShell 5.1, from the repository root. Rust 1.96.0 comes from the repository toolchain. No global installs.

```powershell
Set-Location spikes/s5-play-process
$env:CARGO_BUILD_JOBS = '4'
./run.ps1            # release build, scenes, evidence/*.json (about 90 s)
cargo test --release
cargo clippy --release --all-targets -- -W clippy::unwrap_used -W clippy::undocumented_unsafe_blocks
cargo fmt --all -- --check
```

`run.ps1` runs four steps:
1. `gen-scene` for 5k and 50k entities into `scenes/` (gitignored).
2. `probe-timers`, writing [timers.json](evidence/timers.json).
3. `host` with every suite at 5k plus `--gpu`, writing [host-5k.json](evidence/host-5k.json); suites `snapshot,boot,live,keep` at 50k, writing [host-50k.json](evidence/host-50k.json); and the live suite with `ARA_S5_NAGLE=1`, writing [live-nagle-5k.json](evidence/live-nagle-5k.json).
4. A [machine record](evidence/machine.json).

To run one suite:

```powershell
./target/release/ara-s5-play.exe host --scene scenes/orbit-5k.json --out target/x.json --reps 5 --suites live
```

The reports keep every raw sample.

## Machine and method

| Item | Value |
|---|---|
| OS | Windows 11 Pro 10.0.26300 |
| CPU | Ryzen 7 7800X3D (16 logical) |
| RAM | 31.1 GiB |
| GPU | RTX 4070 SUPER (driver 32.0.16.1714) |
| Rust | 1.96.0, release profile |
| Antivirus | Defender real-time protection on |
| Player executable | 2.56 MB, statically linked Rust |

Other agents were using the PC during the runs, so CPU frequency was not fixed.

Latencies are host-side wall times from `Instant`. p50/p95 use nearest rank. n = 20 per boot strategy (10 for the fresh-binary, Vulkan and multi-client rows) and 200 patch or inspect round trips. All numbers below are p50 / p95 in milliseconds unless a unit says otherwise.

### Snapshot size and transfer (pooled player, Play → tick 1)

"Prepare" is the host-side encode; for shm it also includes creating the section and copying into it. "Transfer + protocol" is the total minus prepare, decode and tick: socket or mapping, JSON-RPC and wakeups. "Decode + spawn" is reported by the player.

| Entities | Transport | Bytes | Prepare | Transfer + protocol | Decode + spawn | Tick 1 | **Play → tick 1** |
|---|---|---:|---:|---:|---:|---:|---:|
| 5k | wsJson | 1,562,325 | 18.73 / 22.94 | 1.76 / 20.81 | 22.00 / 24.60 | 0.18 / 0.21 | 43.72 / 63.81 |
| 5k | wsBinary | 474,477 | 0.36 / 0.46 | 0.42 / 0.55 | 2.38 / 2.72 | 0.17 / 0.20 | **3.32 / 3.76** |
| 5k | shm | 474,477 | 0.61 / 0.84 | **0.10 / 0.12** | 2.59 / 3.49 | 0.18 / 0.26 | **3.49 / 4.41** |
| 50k | wsJson | 15,674,731 | 261.48 / 303.08 | 18.75 / 31.68 | 274.92 / 302.30 | 1.47 / 1.82 | 567.22 / 622.53 |
| 50k | wsBinary | 4,793,977 | 3.21 / 4.29 | 3.70 / 20.03 | 25.26 / 30.43 | 1.50 / 1.85 | 36.11 / 48.95 |
| 50k | shm | 4,793,977 | 4.56 / 4.96 | **0.16 / 0.29** | 23.83 / 27.89 | 1.47 / 1.78 | **30.19 / 33.93** |

Each transport was also checked for fidelity, at both sizes. The player started paused, captured its state hash, then stepped 120 ticks:
- The tick-0 hash equalled the edit world's.
- The tick-120 hash equalled an in-process reference simulation.

The f32 → JSON → f32 round trip is exact.

### Boot: Play → first tick visible in the host

| Strategy | 5k | 50k |
|---|---:|---:|
| In-process simulate (encode, decode, tick; no isolation) | 2.37 / 2.81 | 26.10 / 31.54 |
| **Warm pool, shm** (player already running and connected) | **3.53 / 4.27** | **30.20 / 37.88** |
| Warm pool, wsBinary | 3.45 / 3.76 | 32.34 / 58.83 |
| Cold process, binary in OS cache, shm | 16.36 / 33.20 (max 311) | 39.94 / 44.03 |
| ↳ spawn → hello (process creation, image load, connect) | 13.35 / 28.93 | 13.92 / 14.45 |
| Fresh binary (never executed), shm | 162.31 / 186.30 | 179.86 / 205.70 |
| ↳ spawn → hello | 159.62 / 183.20 | 154.36 / 175.77 |
| Cold process + Vulkan instance and device | 238.44 / 533.77 | n/a |
| ↳ Vulkan total: loader and instance 180.62 / 193.21, device 39.57 / 97.26 | 220.99 / 277.49 | n/a |
| Warm pool + Vulkan (device created before Play) | **3.33 / 3.84** | n/a |

An idle pooled player costs 6.6 MiB of working set (0.9 MiB private), or 72.0 MiB (55.5 MiB private) with a Vulkan device. It blocks on its socket while idle, so it uses no CPU. In development traces (`ARA_S5_TRACE=1`), `main` → hello inside the player took about 2 ms; the rest of spawn → hello is the OS.

### Live link (running player, 60 Hz)

| Measure | 5k | 50k |
|---|---:|---:|
| `play.inspect` round trip | 1.55 / 1.64 | 1.52 / 1.67 |
| **`play.patch` round trip, running** (applied at the next tick, acked after it) | **8.87 / 16.32** (max 17.35) | **10.11 / 17.48** (max 29.98) |
| Ticks run before each ack | 1 in 200/200 | 1 in 200/200 |
| `play.patch` round trip, paused (applied immediately) | 1.56 / 1.62 | 1.55 / 1.64 |
| `play.capture` (state hash) round trip | 1.45 / 1.73 | 4.36 / 5.33 |
| `play.pause` round trip | 1.05 ms | 1.07 ms |
| `play.step` 60 ticks round trip | 9.08 ms | 107.63 ms |
| Tick count after 200 ms paused | unchanged (239 → 239) | unchanged |
| Tick count after step 60 | exact (+60) | exact (+60) |
| Tick rate after resume, measured over 1 s | 60.87 Hz | 60.65 Hz |
| Worst tick-start lateness in one heartbeat window | 0.54 ms | 0.84 ms |
| With Nagle left on (`ARA_S5_NAGLE=1`): patch / inspect | 8.82 / 16.59; 1.54 / 1.64 | n/a |

Timer probe ([timers.json](evidence/timers.json)):
- `thread::sleep(1 ms)` takes 1.52 / 1.59 ms.
- `thread::sleep(250 µs)` takes 0.52 / 1.00 ms.
- A **2 ms socket read timeout takes 15.49 / 17.73 ms**.

### Keep changes

During play, one `play.patch` changed `Orbit.radius` on `0002`, `Tint.rgba` on `0003` and `Label` on `0004`. The same patch created an entity, which received the next free ID `03ux`, and deleted `0005`. After about 30 ticks the player was paused and `play.diff` was called for those five entities. It returned 14 ops, each with `prev` = the snapshot value:
- the three tweaks;
- simulation-driven `Orbit.phase`, `Transform.position` and `Transform.rotation` for the three orbiters;
- `create 03ux` with all its components;
- `delete 0005` with its snapshot.

The host then acted as the editor's preview:
- It unticked the simulation-driven fields.
- It checked each op's `prev` against the current edit world (0 conflicts).
- It applied the 5 remaining ops as one transaction.

Checks after applying:
- The edit world held exactly the chosen values; `Orbit.radius` was 9.5 in both worlds.
- The inverse ops restored the original edit-world hash.
- Re-applying the ops reproduced the kept hash.

The chosen ops, their inverses and the full preview are in [host-5k.json](evidence/host-5k.json) under `keep`.

| | 5k | 50k |
|---|---:|---:|
| `play.diff` for 5 entities, round trip | 4.4 ms | 26.3 ms |
| Apply 5 ops to the edit world | 0.027 ms | 0.029 ms |
| `play.diff` for every entity | 12,005 ops in 72 ms | 119,999 ops in 640 ms |

### Crash isolation (5k, then the "restart" action)

| Forced fault | Structured error | Exit | Detected | Reported | Restart → tick 1 |
|---|---|---|---:|---:|---:|
| Panic | `player_panicked`; cause `{kind: panic, message, location: src\crash.rs:41:29, thread: main}` | 0x00000065 | 1.7 ms | 4.8 ms | 17.0 ms |
| Access violation | `player_crashed`; cause `{kind: exception, code: 0xC0000005, name: STATUS_ACCESS_VIOLATION, address}` | 0xC0000005 | 2.2 ms | 3.6 ms | 16.7 ms |
| Hang (infinite sleep) | `player_unresponsive`; killed | none | 1,053.6 ms | 1,055.7 ms | 16.6 ms |

Each fault also reports the last heartbeat tick, the stderr tail and `actions: [{id: "restart", title: "Restart play from the same snapshot"}]`.

### Multi-client (1 server + 2 clients)

| | All three running |
|---|---:|
| Cold: spawn all three, connect, shared shm snapshot, start, client UDP handshakes | 33.70 / 35.12 |
| Pooled: three idle players already connected | 5.10 / 5.38 |
| Client UDP hello → welcome | 0.50 / 1.32 |

## Findings and gotchas

1. **The process boundary is nearly free.** A pooled player is 1.2 ms slower than in-process simulation at 5k and 4.1 ms slower at 50k. Spawning a process (13–14 ms) costs less than decoding 50k entities (24 ms).
2. **Engine init dominates a cold player, not the OS.** Vulkan instance creation alone takes about 180 ms, mostly the NVIDIA ICD load. The machine registers eight implicit overlay layers (Steam, EOS, Overwolf, OBS, PRISM); pointing `VK_IMPLICIT_LAYER_PATH` at an empty directory saved only about 25 ms in a development run. Device creation adds 40 ms. A real player will add a window, a swapchain, pipelines and the C# runtime on top. The pool hides all of it: warm with a Vulkan device is 3.3 ms.
3. **Rebuilt binaries pay about 150 ms once.** The first launch of a never-executed executable takes 155–160 ms from spawn to hello, against 14 ms for a known one. That fits Defender's first-execution scan, with real-time protection on. Every rebuild of the player would pay this on the next Play. Respawning the pool right after a build moves the cost off the critical path.
4. **Decode and spawn is the warm cost at scale.** At 50k it is 24 of 30 ms. The decoder runs one `insert_batch` per column, which re-archetypes every entity four times. Spawning complete bundles per archetype, or a later chunk-level copy, would cut this. It doesn't matter at 5k.
5. **JSON is the wrong play transport.** It is 3.3× larger and about 13–19× slower end to end, with encode and decode each about 20 ms at 5k and 260–275 ms at 50k. Keep JSON for files; use binary for Play.
6. **Large loopback socket sends stall intermittently; shm doesn't.** A 5 MB WebSocket frame took either about 3 ms or 11–21 ms, with no pattern. The same bytes over shm took 0.13–0.85 ms. Shm also lets N players map one encoded snapshot.
7. **Patch latency is the wait for the next tick.** p50 is about half a tick and p95 about one tick (16.7 ms at 60 Hz). That is about a third of the budget, so lower tick rates need more care: at 30 Hz the p95 would be about 33 ms. Patching a field that a system writes every tick (e.g. `Transform` on an orbiter) is overwritten by that tick, and the read-back shows it. The inspector should mark such fields as "driven by" a system.
8. **Windows socket timeouts are coarse.** A 2 ms `SO_RCVTIMEO` waits 15.5 ms (the default 15.6 ms tick), so socket timeouts can't pace a 60 Hz loop. `thread::sleep` uses a high-resolution timer (1 ms → 1.52 ms), so the player sleeps and polls. The 1.5 ms inspect round trip is that poll interval. A production player should poll once per rendered frame, or use an I/O thread with an event and a high-resolution timer.
9. **`TCP_NODELAY` made no measurable difference** on Windows loopback for request/response traffic (patch p95 16.59 vs 16.32 ms). Keep it on for pipelined streams anyway.
10. **Crash plumbing needs three small things on Windows:**
    - `SEM_NOGPFAULTERRORBOX` and an unhandled-exception filter, so a crashed player exits at once with its NTSTATUS rather than through Windows Error Reporting;
    - a panic hook that writes one machine-readable line, since an unwinding panic exits with 101;
    - a heartbeat watchdog, because a hang produces no exit at all.

    The watchdog must drain queued heartbeats before judging staleness. An early version flagged a healthy player after the host had been busy for 1 s.
11. **Windows socket and UDP quirks:**
    - Accepted sockets inherit the listener's non-blocking mode.
    - An ICMP "port unreachable" on UDP comes back as `WSAECONNRESET` from a later `recv`; treat it as "retry".
12. **One outlier was not explained.** One cold spawn in twenty, at 5k, took 308 ms from spawn to hello. It didn't recur in the 50k run (max 45 ms) or in development runs. The PC was shared. A pooled player is not exposed to it.
13. **Security notes for production:**
    - The `Local\` section namespace lets any process in the same session open a section by name, so names carry part of the session token (done here). Production should use an explicit DACL or inherited handles.
    - The token is passed on the command line, which other same-user processes can read. Pass it through an inherited pipe or the environment instead.
14. **The diff baseline is decoded on every call.** That is fine at 5k (4.4 ms) but 26 ms at 50k. Decode only the requested entities, or keep the baseline world.

**Godot comparison.** Godot's editor runs a TCP debugger server; the game connects back to it and is edited with `scene:live_*` messages addressed by node path. With `run/auto_save/save_before_running` on, Godot saves the scene before running, and the game reloads it from disk. Ara instead snapshots the in-memory edit world (unsaved edits included), addresses everything by stable ID, and reuses the authoring op vocabulary. That makes Keep changes a diff rather than a special save path.

## Recommendation

**Keep A6: play runs in a child process.** Add a **pre-spawned player pool** as the default:
- Keep one idle, fully initialised player per configuration. A Host + N or Dedicated + N setup keeps N + 1.
- Each player is single-use: after Stop it exits, and the pool immediately spawns a replacement.
- Respawn the pool after every player rebuild, so Defender scans, image loads and GPU/C# init happen before the user presses Play.

The pool isn't needed to meet < 1 s today, since even a cold player with a Vulkan device starts in about 240 ms. It is what keeps Play at 3–30 ms as player startup grows. An idle player costs about 7 MiB, or about 72 MiB with a GPU device, and no CPU. If a pool player is missing or has died, fall back to a cold spawn.

**Snapshot transfer:** the columnar binary snapshot in a shared-memory section (Windows named section; `shm_open` + `mmap` on macOS and Linux). Use an EP binary frame for remote players such as ara-bridge on devices. JSON stays the file format only.

**Live edits:** the same typed ops as authoring, applied at the next tick boundary and acked after that tick. The expected p95 is one tick.

**Specify the in-process "simulate" mode, but don't make it the Play path.** At 5k it saves only about 1 ms and gives up isolation. Use it only where a process can't help:

| Use simulate when | Why |
|---|---|
| Scene-view "Simulate" (UE-style physics preview of the edit world, no game start) | Users drag edit objects while physics runs; no game code or input is involved. |
| Batches of very short headless worlds in `ara test` or an agent's `sn_play` loop (hundreds of seed or replay variants) | It avoids a process per world. Crashes are still contained by running the batch in one worker process. |
| Platforms or sandboxes that forbid spawning processes | It is the only option there. |
| Comparing two worlds at the same tick in one address space (determinism and replay tests) | It allows direct, field-level comparison. |

Simulate runs the same `Sim`, ops and `play.*` methods against a world ID inside the edit-world owner. It wraps calls in `catch_unwind`; there is no protection from access violations or hangs. Because gameplay state lives only in ECS components, the two modes must produce identical hashes, and a determinism test should assert that.

## Proposed process and protocol shape for M2 play mode

**Processes.** The edit-world owner is `ara editor`, or the `ara` daemon when headless. It owns the documents, the listener and the pool. `ara-player` processes connect back to it over localhost WebSocket with the session token. They never accept connections themselves. Agents and the editor UI talk only to the owner, which routes `play.*` calls to the right player by `world` ID (for example `play:3`). There is one auth boundary, and players need no MCP surface of their own.

**Lifecycle.**

```text
spawning ─hello─▶ idle (pooled: runtime + GPU + C# host initialised, no world)
idle ─play.start─▶ loading ─tick 1─▶ playing ⇄ paused
playing|paused ─play.stop─▶ exited
any state ─link closed | exit code | no heartbeat for 1 s─▶ faulted ─"restart"─▶ (next pool player) loading
```

Players are single-use: a new pool player replaces each one as soon as it is taken. Multiplayer sessions take one server and N clients from the pool together.

**Messages.** JSON-RPC 2.0. The params and results are the types in [`src/protocol.rs`](src/protocol.rs); `seed` and `tickHz` in `play.start` are M2 additions.

| Method | Direction | Shape |
|---|---|---|
| `player.hello` | player → owner | `{token, protocol, pid, role, name, netAddr?, initUs, gpu?}` → `{session}` |
| `play.start` | owner → player | `{snapshot: {transport: shm, section, len} \| {transport: wsBinary, blob}, startPaused, connectTo?, seed, tickHz}` → `{tick, entities, decodeUs, firstTickUs, netHandshakeUs?}` |
| `play.inspect` | owner → player | `{target, components?}` → `{tick, components}` (answered between ticks) |
| `play.patch` | owner → player | `{ops: Op[]}` → `{appliedAtTick, tick, inverse, readBack}` (atomic, at the next tick boundary) |
| `play.pause` / `play.resume` | owner → player | `{tick}` |
| `play.step` | owner → player | `{ticks}` → `{tick, hash, entities}` (paused players only) |
| `play.capture` | owner → player | `{tick, hash, entities}`; M2 adds `view.capture` frames as blobs |
| `play.diff` | owner → player | `{targets?, components?}` → `{tick, ops}` with `prev` = snapshot value |
| `play.stop` | owner → player | `{tick}`; the player then exits |
| `play.status` | player → owner, every 250 ms | `{tick, paused, clients, maxLateUs}`; doubles as the liveness signal |
| `play.debug.crash` | owner → player | test builds only |

Methods to add in M2, on the same channel: `play.input` (action injection), `play.watch`/`until`, `play.record`, and a subscription stream of runtime op or change events for the inspector.

**Snapshot format.** `ARASNAP1`. It holds a header (version, entity count, next ID) plus stable IDs in ascending order. Each component is one column: a presence bitset followed by packed values. The column codecs are generated from the schema DB. M2 adds a schema hash to the header (a player built against a different schema refuses the snapshot) and resources such as RNG state and the session seed. Equal worlds encode to identical bytes, so the snapshot hash can key a play-start cache.

**How it reuses the Engine Protocol (A5).** It reuses the protocol's pieces rather than defining a second one:
- **Envelope and transport.** The same JSON-RPC 2.0 envelope and the same WebSocket transport, with token auth, structured errors-as-data and EP binary frames for bulk data.
- **Method definitions.** The same typed method registry: `Method` here is what the registry generates.
- **Ops.** The same op vocabulary and stable-ID addressing: `play.patch` takes exactly the ops `world.transact` takes, and `play.diff` returns ops that `world.transact` can apply.
- **Players as EP peers.** A player serves a scoped subset of EP world methods (`world.get`, `world.query`, `world.transact`, `sim.step`, `view.capture`) under `world: "play:N"`. MCP tools such as `sn_inspect` and `sn_play` therefore work on play worlds unchanged, and the in-process simulate mode serves the same methods under `world: "sim:N"`.

## Proposed decision

Text for a `docs/decisions.md` row under "Spike results (M0)":

| Spike | Result | Decision | Evidence |
|---|---|---|---|
| S5 | **PASS** (2026-10-23) | **Play runs in a child `ara-player` process (A6 kept), fed from a pre-spawned, single-use player pool.**<br>• Pooled Play → first tick: 3.5 ms at 5k entities, 30 ms at 50k (only 1–4 ms slower than in-process). Cold: 16 ms; with a Vulkan device 240 ms.<br>• Snapshot: columnar binary (`ARASNAP1`) over shared memory; JSON stays the file format only (13–19× slower).<br>• Live link: Engine Protocol JSON-RPC over localhost WebSocket; the player connects back to the edit-world owner with the session token.<br>• Live edits are authoring ops applied at the next tick boundary: patch round trip p95 16.3 ms at 60 Hz.<br>• "Keep changes" = `play.diff` ops against the start snapshot, rebased with `prev` conflict checks and applied as one transaction.<br>• Crash/hang isolation: exit code + crash line + 1 s heartbeat watchdog, with a restart action.<br>• In-process "simulate" is a specified secondary mode (scene-view Simulate, batched headless tests, no-spawn platforms) serving the same `play.*` methods. | [`spikes/s5-play-process/RESULT.md`](../spikes/s5-play-process/RESULT.md) |

No locked decision needs to change. This result settles the open item "Play-mode process model: child process vs an in-process simulate fallback (S5)": both exist, and the child process is the default.
