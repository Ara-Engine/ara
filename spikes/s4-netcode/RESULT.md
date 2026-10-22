# S4 — Schema-owned headless netcode

Date: 2026-10-23. **Behavioral gates PASS using the permitted Renet fallback. The requested Lightyear 0.31 / Replicon 0.45.2 integration FAILS compatibility with the pinned Rust 1.96 toolchain.** This is evidence for a toolchain decision and the schema/session boundary, not approval to ship this throwaway replication implementation.

## Compatibility finding

`compatibility/` is an isolated, locked dependency probe for the exact requested stack. `cargo check --locked` exits 1 before compiling our code:

```text
error: rustc 1.96.0 is not supported by the following packages:
  bevy@0.20.0 requires rustc 1.97.1
  lightyear@0.31.0 requires rustc 1.97.1
  lightyear_connection@0.31.0 requires rustc 1.97.1
  ...
```

The complete stderr is [artifacts/lightyear-rust196.txt](artifacts/lightyear-rust196.txt). Replicon uses the umbrella `bevy` crate even with its default features disabled. In contrast, **`bevy_ecs = 0.20.0` itself builds on 1.96**, and is used by the working fallback. There was no toolchain upgrade, MSRV bypass, dependency patch or downgrade of Bevy ECS.

Consequently, no Lightyear/Replicon runtime, registration adapter, built-in predictor, visibility integration or link conditioner was validated here. Their API/performance suitability remains unproved. We did not claim an empty dependency probe implements those features. Naia/Quinn were not needed: Renet 2.0.0 + renet_netcode 2.0.0 resolved and passed the behavioral tests.

Primary package references: [Lightyear 0.31.0](https://crates.io/crates/lightyear/0.31.0), [Replicon 0.45.2](https://crates.io/crates/bevy_replicon/0.45.2), [Bevy ECS 0.20.0](https://crates.io/crates/bevy_ecs/0.20.0), [Renet 2.0.0](https://crates.io/crates/renet/2.0.0), [Renet netcode 2.0.0](https://crates.io/crates/renet_netcode/2.0.0). Cargo.lock and the actual compiler probe, rather than version metadata alone, establish the result.

## What was built

- `schema.rs`: Ara-owned component/intent declaration macros. Four component declarations carry stable schema ID, audience, quantization, interpolation and rollback policy; two intent declarations carry stable ID and rate. One declaration generates the Rust component, serde codec implementation and metadata. This is a small declaration prototype, not a general reflection compiler.
- `gameplay.rs`: no Lightyear, Replicon, Renet, codec or socket imports. Four motors, a shared door, 45 pickup slots and four private inventories live in ECS components. A separate ECS clock makes 51 authoritative ECS entities, of which **50 logical entity slots replicate**. ID order is fixed by the scene, never by process-local Bevy entity bits.
- `MoveInput`, complete restorable `Motor`, and one `step_motor` function run offline, on the authority and in the owner's ECS prediction world. Fixed tick is 60 Hz; snapshots are 20 Hz. Integer millimetres provide declared wire quantization without a floating-point rounding seam. No simulation wall clock, global state or RNG is used.
- `ToggleDoor` and `PickUp` are gameplay intents, not document edits. The connection supplies caller identity. The authority validates caller bounds, target, range, item availability, sequence and cooldown, then mutates at a tick boundary. Toggle limit: 5/s; pickup limit: 10/s. Duplicate/rejected sequence identities cannot be reused to bypass validation. Public pickup disappearance is intentionally visible; private inventory contents are filtered separately.
- `adapter.rs`: the Renet fallback registers and validates the four component and two intent policies. Unsupported policy combinations fail rather than silently degrading. A snapshot contains all public components and **only one recipient's inventory**; filtering precedes encoding. Active predicted motor state is an ECS component; retained copies are rollback checkpoints. Remote presentation samples two checkpoints without mutating simulation.
- `session.rs`: secure netcode tokens issued by the authority, encrypted UDP, Renet unreliable input/snapshot channels and reliable ordered intent/baseline/final channels. Keys and tokens are generated per run and remain in ignored runtime artifacts. Inputs repeat the most recent 24 sampled ticks; **future bot inputs are not sent**. Missing/late input is neutral, not server rollback. Intent requests up to 60 ticks late are assigned the next authority tick; future requests are bounded to 120 ticks. Applied frames retain the assigned tick.
- Local: one authoritative world, direct typed frames, no transport, codec instance, replica world or prediction history. Listen: one authority world plus an in-process host endpoint with direct typed input and confirmed projection; remote clients are processes. The host is stepped once, with no duplicate prediction. Dedicated: a separate authority process and three/four separate bot processes.
- `conditioner.rs` / `harness.rs`: actual loopback UDP proxies impair encrypted datagrams **below** Renet's reliability layer. Directional nominal delay is 50 ms, uniform jitter is ±20 ms, and loss probability is 5% independently in each direction. Delays also cause reordering. The seed is 21668. The proxy services I/O roughly every millisecond; Renet packet flushes are capped at 60 Hz. Initial baselines share an authority start barrier; late baselines use current state.
- `run.ps1`: the repeatable gates, measurements, process scenarios and persisted replay verification. A child-process guard kills/reaps every owned server/client on both success and error. Ports are allocated by binding loopback port 0. No services, global installs or persistent background processes are needed.

## Exact commands

From the repository root, PowerShell 5.1:

```powershell
& .\spikes\s4-netcode\run.ps1
```

It sets `CARGO_BUILD_JOBS=4`, runs the checks below, writes measurements under `artifacts/`, and keeps per-run logs, connection tokens and replay records under ignored `artifacts/runtime/`. Paths printed in committed JSON identify the measured runs on this machine; a fresh run creates new paths. Runtime tokens/logs are not committed.

Individual commands, starting from the repository root:

```powershell
Set-Location spikes/s4-netcode
$env:CARGO_BUILD_JOBS = '4'

cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo clippy --locked --no-default-features --all-targets -- -D warnings
cargo test --locked --no-default-features

cargo run --locked --release --no-default-features -- local
cargo run --locked --release --no-default-features -- bench
cargo run --locked --release -- bench
cargo run --locked --release -- harness dedicated3 impaired 21668 artifacts/dedicated3-impaired.json
cargo run --locked --release -- harness dedicated4 impaired 21668 artifacts/dedicated4-impaired.json
cargo run --locked --release -- harness listen impaired 21668 artifacts/listen-impaired.json
cargo run --locked --release -- harness dedicated4 clean 21668 artifacts/dedicated4-clean.json
cargo run --locked --release -- harness dedicated4-live impaired 21668 artifacts/dedicated4-live-impaired.json

$run = Get-Content -Raw artifacts/dedicated4-impaired.json | ConvertFrom-Json
cargo run --locked --release -- replay (Join-Path $run.runtime_artifacts 'replay.json')

# Expected failure on the unchanged pinned toolchain:
cargo check --locked --manifest-path compatibility/Cargo.toml
```

The root spike has its own empty `[workspace]`, Cargo.lock and build-output `.gitignore`; the compatibility probe is another independent workspace. No rendering or dev server is used.

## Machine and methodology

Windows 11 Pro, Ryzen 7 7800X3D (8 cores/16 threads), 33,410,686,976 bytes RAM (31.12 GiB); Rust/Cargo 1.96.0, `x86_64-pc-windows-msvc`, LLVM 22.1.2. Release profile, optimized with debug information, four Cargo jobs. [Machine record](artifacts/machine.json). This is a shared PC: no CPU affinity, thermal controls or isolated performance environment. Numbers are measurements of this workload, not platform budgets.

Each network scenario runs 600 fixed ticks (10 simulation seconds), stops inputs, freezes authority at tick 600, reliably sends final checkpoints and waits for client acknowledgements. Clients reconcile and assert their owner prediction equals the authoritative motor. Each recipient projection is compared field by field at the same final tick. Late player 3 joins roughly three seconds after authority starts, after the door opens at tick 40 and three pickups occur at tick 60; it collects its own item at tick 260. Characters move out/back at ticks 100–219 and 300–419; the late player participates in the second interval.

The authority records the actual input and attempted intent frame at every tick, including commands rejected by gameplay validation. Replay creates a fresh Local simulation, checks all 600 full authoritative checkpoints (including private inventories and cooldown/sequence state), and compares the final stable FNV-1a state hash. The persisted JSON replay is then independently loaded and replayed by the harness. It contains no wall-clock simulation dependencies. First-difference diagnostics include tick, entity and field; a test deliberately corrupts motor X and door change tick to verify them.

## Pass/fail criteria

| Criterion | Result | Evidence |
|---|---|---|
| Character, door and inventory behave consistently in Local, Listen and Dedicated | **PASS, Renet fallback** | Buffered four-player Local, Listen, clean Dedicated and impaired Dedicated end at full authoritative hash `e013eb0e61ea0d88`; all inventories have their own pickup, door is open, motors return to origin. Dedicated-three matches the equivalent three-player Local script at `65aa66aa3f856001`. |
| Late join | **PASS** | Impaired Dedicated and Listen baseline at tick 198: open door, exactly three unavailable pickups, correct private inventory. Clean baseline tick 182; tight-window baseline tick 197. Final recipient projections agree. |
| Behavior under packet loss | **PASS within tested profile** | All primary sessions converge with approximately 5% actual packet loss each direction. The tight-window run also converges after 223 real corrections; see the missing-input qualification below. |
| Replay reproduces state hash | **PASS** | All five network scenarios replay all 600 checkpoints and match their authoritative hash. Tight-window hash `7000eae6cfe1e0fe` also reproduces exactly. |
| Inventory privacy | **PASS** | Zero foreign-owner inventory values across 3,251 received/decoded snapshots/baselines/finals in the five final scenarios. Every decoded packet's owner is checked before use, including stale packets; the golden codec test uses distinct inventory sentinels. |
| Local has zero serialization and network allocation | **PASS, structural proof** | Both default-feature and network-free Local tests pass. Gameplay serialization bytes = 0, network ECS resources = 0, transport objects = 0, history entries = 0. Local's one ECS resource equals a fresh Bevy world's baseline. The network-free dependency tree has no Renet, renetcode or postcard. |
| Exact Lightyear 0.31 + Replicon 0.45.2 adapter on pinned Rust | **FAIL / blocked** | Both Lightyear and umbrella Bevy require Rust 1.97.1; no such runtime adapter was compiled or tested. |

Local zero values describe absent facilities, not a global allocator or OS syscall trace. [`LocalSession`](src/local.rs) owns only `Simulation`; its resource count equals `World::new()`. The codec/transport modules are excluded entirely with `--no-default-features`, and [the captured dependency tree](artifacts/local-dependencies.txt) demonstrates that exclusion. CLI JSON diagnostics and separately requested replay recording are outside the ordinary Local tick path.

All local gates passed: formatting, scoped `cargo check`, Clippy with warnings denied for both feature configurations, six default-feature tests and three network-free tests. All five socket/process scenarios passed. The only expected failing command is the requested-stack compatibility probe.

## Measured bandwidth

Four players, **50 logical replicated entities**, 60 Hz simulation, 20 Hz full snapshots. The measurement window is authority elapsed seconds 5–9 (3.999391 measured seconds), after the late client connects; it includes movement and the final neutral interval. All 50 public slots are sent each snapshot; the 45 pickups are static during this window. There is no delta compression or dormancy optimization.

Counts are **offered encrypted UDP payload bytes before impairment**, including Renet headers/ACKs and netcode overhead, excluding IPv4/UDP/Ethernet headers. Each proxy counts independently in each direction. Thus these are transport-level application datagrams, not serde-only component bytes or NIC wire bytes. Handshake/initial baseline traffic is outside this steady-state window.

| Client | Downstream bytes/s | Upstream bytes/s |
|---|---:|---:|
| 0 | 4,355.2 | 8,861.6 |
| 1 | 4,320.7 | 8,862.6 |
| 2 | 4,324.2 | 8,854.6 |
| 3, late joiner | 4,294.4 | 8,847.3 |
| Mean | **4,323.6** | **8,856.5** |

[Raw impaired measurement](artifacts/dedicated4-impaired.json). Actual whole-run drops: downstream **5.085%**, upstream **4.979%**. Sampled one-way delays span approximately 30.0–70.0 ms; client means are 49.0–50.8 ms. The 100 ms RTT is the configured network delay sum; packet batching and OS scheduling add latency. Jitter here means ±20 ms **per direction**, not a 20 ms standard deviation or a hard ±20 ms RTT envelope.

Clean four-player mean: **4,212.6 downstream / 8,828.5 upstream bytes/s**. Impaired listen mean per remote client: **4,321.1 downstream / 8,856.6 upstream bytes/s**. Host input/presentation uses a direct endpoint, not an extra network connection.

An exploratory pre-change run flushed on every ~1 ms poll: roughly **20.2 kB/s down / 24.2 kB/s up** per client ([raw record](artifacts/uncapped-flush-dedicated3.json)). The final 60 Hz flush run for the same three-client topology is **4.32 kB/s down / 8.85 kB/s up**. ACK traffic must be counted and packet scheduling must be explicit. These are before/after observations on a shared machine, not an isolated statistical comparison.

## Corrections and replay

The primary 12-tick prediction-lead profile received all **840 non-neutral player input ticks** in four-player authority and produced **0 nonzero corrections across 705 checked owner checkpoints**. This does not prove prediction under every loss burst. The separate `dedicated4-live` profile reduces the configured lead to six ticks, sends only already-sampled inputs, and asserts that real corrections actually occur.

| Owner | Checked checkpoints | Nonzero corrections | Corrections/s of active simulation | Mean magnitude, mm | p95, mm | Max, mm |
|---|---:|---:|---:|---:|---:|---:|
| 0 | 193 | 71 | 7.10 | 56.34 | 100 | 125 |
| 1 | 184 | 68 | 6.80 | 54.41 | 75 | 150 |
| 2 | 190 | 47 | 4.70 | 34.57 | 75 | 75 |
| 3 | 125 | 37 | 5.51 | 49.32 | 75 | 125 |

Total: **223/692 checkpoints (32.23%)**, **50 mm** mean, **150 mm** maximum. [Raw correction run](artifacts/dedicated4-live-impaired.json). Magnitude is Euclidean distance between authoritative and predicted motor state at the **same tick**, before restore/replay; this is not rendered-frame displacement. The adapter recorded 4,742 replayed motor ticks in this run. Its deliberately simple reconciliation also replays checkpoints that match, so resimulated-tick count is not correction count.

**Tight-window qualification:** with neutral fallback for late inputs, only 388/840 intended non-neutral input ticks reach authority on time. The final authoritative X values are 250, -200, 125 and -75 mm, rather than the ideal script's zero. This run **does not pass ideal-script movement equality** (`ideal_script_outcome_match=false`). It does pass same-tick replica convergence, owner correction flush, exact Local replay, door/pickup behavior and privacy. Three intents were assigned a later authority tick. This is a demonstrated input-buffering/timeline tradeoff, not hidden nondeterminism or a claim that packet loss preserves every intended movement input. M2 needs proper synchronized input delay, bounded clock adjustment and explicit missing-input policy, preferably upstream facilities.

The focused restore test also forces a two-axis motor correction at tick 5, restores complete motor state and replays five inputs to the exact expected state. Replay diagnostic tests report, for example, `tick 121 player 2.motor.x: 551 != 550`, or `tick 121 door.changed`.

## Local overhead

Eight retained paired samples, 200,000 ticks/sample, first sample discarded. Four active motor components; construction, scene extraction, command construction, reports and replay recording are excluded. The bare baseline calls the same motor function through indexed `bevy_ecs` borrows. Direct simulation additionally performs bounded input validation, contiguous-tick checking, the command boundary and ECS clock bookkeeping. Local adds only its session wrapper. Raw timings: [network-free](artifacts/bench-local.json), [network feature enabled, Local path](artifacts/bench-network-enabled.json).

| Configuration | Bare ECS motor loop, median ns/tick | Direct simulation, median ns/tick | Local session, median ns/tick | Local / direct |
|---|---:|---:|---:|---:|
| Network dependencies excluded | 34.736 | 46.948 | 46.734 | **0.995×** |
| Network feature compiled in | 29.977 | 43.722 | 43.981 | **1.006×** |

Local is **1.345×** the bare four-motor loop with networking compiled out: roughly **12 ns/tick** of simulation contract/clock work, not wire processing. The session wrapper is within measurement noise of identical direct simulation. Cross-build timing differences reflect code generation and shared-host noise; they are not evidence that linking networking speeds anything up. No universal zero-CPU-overhead claim is made. The meaningful D4 result is zero mandatory networking work/allocation/serialization on the ordinary Local path.

## Findings and limits

1. **The compiler/toolchain boundary is the first upstream blocker.** This spike does not establish a runtime defect in Lightyear or Replicon. A parent-owned toolchain decision is required before evaluating that exact stack. Older Lightyear releases would evaluate a different architecture, so they were not silently substituted.
2. **Policy and session boundaries work without backend names in gameplay.** The fallback's policy table validates this exact four-component scene and manually projects its fixed layout. M2 should generate registrations, field codecs and recipient projections from the schema DB; this is not a completed derive system or script binding generator.
3. **Tick-stamped inputs and discrete intents need different lateness semantics.** Stale movement samples are discarded; bounded late intent requests enter the next tick and undergo ordinary sequence/cooldown validation. Demanding the sender's original tick rejected a valid reliably retransmitted door action in an earlier run. Replay records actual execution ticks, not presumed delivery ticks.
4. **Start barriers and packet flush cadence matter.** A client must not advance an initial prediction timeline while the authority waits for handshakes. Polling sockets and flushing packets are separate rates. Otherwise clock offsets and ACK bandwidth obscure the netcode result.
5. **Privacy must be enforced before encoding**, including join/final baselines. Server replay artifacts intentionally contain private authoritative state on disk; they are never sent through the gameplay protocol. Public pickup availability can reveal that an item was collected; it does not contain another player's inventory component.
6. **This is a bounded fixed-scene demonstration.** No dynamic spawn/despawn, generations/reconnect, relevancy transitions beyond one all-relevant room, change compression, congestion tuning, persistence, malicious-client hardening, cross-platform replay, physics prediction or C# integration was tested. The process coordinator uses wall clock only for transport pacing; OS scheduling means the same link seed is not a bit-identical packet replay.
7. **Replay scope is same-build/same-machine gameplay.** Schema identity/version is checked; the initial scene is the deterministic constructor. It is not a general replay container with build/asset hashes, arbitrary initial world snapshots or captured packet scheduling. FNV-1a is a diagnostic hash, not cryptographic integrity.
8. **The fallback predictor is intentionally disposable.** In particular, replaying matching checkpoints is wasteful, and the six-tick run exposes its primitive clock/input buffering. Do not grow it into a second generic prediction framework before the requested upstream stack has a fair supported-toolchain evaluation.

## Recommendation and proposed M2 boundary

Keep D4 and A10: one schema-owned gameplay model, server authority, selective owner prediction, remote presentation interpolation, and Local/Listen/Dedicated session semantics. The fallback proves these can coexist while Local excludes networking entirely. **Do not lock Lightyear/Replicon based on S4**, because its integration was not runnable on the agreed toolchain. First decide Rust >=1.97.1 (in parent-owned files), then rerun these exact gates against Lightyear/Replicon behind the same schema seam. Retain Renet as a demonstrated lower-layer fallback, not a recommendation to ship this custom predictor.

Proposed crate responsibilities:

- `ara-schema`: stable schema/field IDs, versioning, documented policies and generated codecs/validation; no Lightyear/Replicon types in generated gameplay APIs.
- `ara-sim`: fixed tick, `SimulationFeature`, typed input/intent boundary, authoritative components, checkpoint capture/restore and Local sessions. It has no `ara-net` dependency.
- `ara-net` (opt-in): session allocation/connection lifecycle, schema-generated upstream registration, recipient filtering, input timelines, prediction history, interpolation buffers, transport adapters and diagnostics. Network dependencies are private. It consumes `ara-schema` and `ara-sim`, never the reverse.
- Replay writer and agent scenario tooling are opt-in observers. A normal Local session has neither network nor recorder buffers. The editor can launch Host+N or Dedicated+N using the same typed action API.

Rust API sketch for **M2 Multiplayer v1** (design only; these derives/types are not claimed implemented):

```rust
// ara-schema / generated gameplay contract
#[derive(Component, AraSchema)]
#[ara(id = "outpost.motor", version = 1)]
#[replicate(audience = Relevant)]
#[simulation(rollback = Owner)]
struct MotorState {
    /// Horizontal position in millimetres.
    #[net(quantize = Millimetres, interpolate = Linear)]
    x: i32,
    /// Second horizontal axis in millimetres.
    #[net(quantize = Millimetres, interpolate = Linear)]
    z: i32,
}

#[derive(Component, AraSchema)]
#[ara(id = "outpost.inventory", version = 1)]
#[replicate(audience = OwnerOnly)]
struct Inventory {
    /// Bounded slots; the generated codec rejects oversized collections.
    items: BoundedVec<ItemId, 32>,
}

#[derive(AraIntent)]
#[ara(id = "outpost.toggle-door", version = 1)]
#[intent(execute = AuthorityTick, caller = Player, rate_per_second = 5)]
struct ToggleDoor {
    /// Stable typed handle; authority checks range and interaction permission.
    target: EntityHandle<Door>,
}

impl SimulationFeature for CharacterMovement {
    type Input = MoveInput;
    type State = MotorState; // Complete restorable state, held in ECS.

    fn step(ctx: &FixedContext, state: &mut MotorState, input: &MoveInput) {
        // The identical function runs offline, on authority and during owner replay.
        state.x += i32::from(input.x) * ctx.motor_step_mm();
        state.z += i32::from(input.z) * ctx.motor_step_mm();
    }
}

// Ara integration owns these registrations; gameplay never names an upstream crate.
fn register_outpost(schema: &mut SchemaRegistry) {
    schema.component::<MotorState>();
    schema.component::<Door>();
    schema.component::<Inventory>();
    schema.intent::<ToggleDoor>();
    schema.intent::<PickUp>();
    schema.simulation::<CharacterMovement>();
}

// LocalSession is in ara-sim and allocates one world, no network adapter.
let mut local = LocalSession::new(world, features);
local.submit(player, ToggleDoor { target: door });
local.step();

// Available only with the multiplayer feature; ara-net chooses the private adapter.
let session = NetSession::start(
    SessionMode::Listen { local_players: 1 }, // or Dedicated / Client
    schema,
    NetProfile::adverse(100, 20, 0.05),
)?;
session.submit(player, ToggleDoor { target: door });
```

Replication audience, quantization, interpolation and rollback must remain independent policies. Owner identity comes from authenticated session context, not the inventory's payload or an arbitrary client-supplied caller. The adapter compiles schema policy to upstream registrations and filters; it does not become a second schema source. Changing backend must preserve schema IDs, action semantics, same-tick checkpoints and test artifacts.

## Proposed decision

> **S4: preserve schema-owned multiplayer and an independent Local simulation path.** The Local/Listen/Dedicated character, shared-door, private-inventory, late-join, adverse-link and same-machine replay gates pass with Bevy ECS 0.20 + Renet 2.0/netcode. Local creates no network resources or gameplay serialization. The exact Lightyear 0.31 + Replicon 0.45.2 stack cannot be evaluated on pinned Rust 1.96 because Lightyear and umbrella Bevy require Rust 1.97.1. Defer selection of that runtime stack until an explicit toolchain decision and a rerun of these gates. M2's `ara-net` is optional and private behind `ara-schema` / `ara-sim`; no upstream networking types enter gameplay declarations. Keep the spike's custom replication/prediction disposable.

This section is proposed text for the parent agent. No locked decision, milestone, root README or AGENTS file was edited.
