# S4 — Schema-owned headless netcode

Date: 2026-10-23. **Current result: all six gates PASS for Lightyear 0.31 + Replicon 0.45.2 and the Renet reference on Rust 1.99.1. Recommend Lightyear + Replicon behind Ara's schemas.** See **Lightyear rerun** and the revised **Proposed decision** below. The earlier sections retain the original Rust 1.96 measurements and compatibility finding as history; D17 supersedes that toolchain blocker.

## Compatibility finding — original Rust 1.96 run

`compatibility/` is an isolated, locked dependency probe for the exact requested stack. On Rust 1.96, `cargo check --locked` exited 1 before compiling our code:

```text
error: rustc 1.96.0 is not supported by the following packages:
  bevy@0.20.0 requires rustc 1.97.1
  lightyear@0.31.0 requires rustc 1.97.1
  lightyear_connection@0.31.0 requires rustc 1.97.1
  ...
```

The complete stderr is [artifacts/lightyear-rust196.txt](artifacts/lightyear-rust196.txt). Replicon uses the umbrella `bevy` crate even with its default features disabled. In contrast, **`bevy_ecs = 0.20.0` itself builds on 1.96**, and is used by the working fallback. During that original run there was no toolchain upgrade, MSRV bypass, dependency patch or downgrade of Bevy ECS.

At that point, no Lightyear/Replicon runtime, registration adapter, built-in predictor or visibility integration had been validated. The Rust 1.99.1 rerun below now supplies that evidence. Naia/Quinn were not needed: Renet 2.0.0 + renet_netcode 2.0.0 resolved and passed the initial behavioral tests.

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

# Historical failure on 1.96; this now succeeds under D17 / Rust 1.99.1:
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

## Original findings and limits

1. **The compiler/toolchain boundary is the first upstream blocker.** This spike does not establish a runtime defect in Lightyear or Replicon. A parent-owned toolchain decision is required before evaluating that exact stack. Older Lightyear releases would evaluate a different architecture, so they were not silently substituted.
2. **Policy and session boundaries work without backend names in gameplay.** The fallback's policy table validates this exact four-component scene and manually projects its fixed layout. M2 should generate registrations, field codecs and recipient projections from the schema DB; this is not a completed derive system or script binding generator.
3. **Tick-stamped inputs and discrete intents need different lateness semantics.** Stale movement samples are discarded; bounded late intent requests enter the next tick and undergo ordinary sequence/cooldown validation. Demanding the sender's original tick rejected a valid reliably retransmitted door action in an earlier run. Replay records actual execution ticks, not presumed delivery ticks.
4. **Start barriers and packet flush cadence matter.** A client must not advance an initial prediction timeline while the authority waits for handshakes. Polling sockets and flushing packets are separate rates. Otherwise clock offsets and ACK bandwidth obscure the netcode result.
5. **Privacy must be enforced before encoding**, including join/final baselines. Server replay artifacts intentionally contain private authoritative state on disk; they are never sent through the gameplay protocol. Public pickup availability can reveal that an item was collected; it does not contain another player's inventory component.
6. **This is a bounded fixed-scene demonstration.** No dynamic spawn/despawn, generations/reconnect, relevancy transitions beyond one all-relevant room, change compression, congestion tuning, persistence, malicious-client hardening, cross-platform replay, physics prediction or C# integration was tested. The process coordinator uses wall clock only for transport pacing; OS scheduling means the same link seed is not a bit-identical packet replay.
7. **Replay scope is same-build/same-machine gameplay.** Schema identity/version is checked; the initial scene is the deterministic constructor. It is not a general replay container with build/asset hashes, arbitrary initial world snapshots or captured packet scheduling. FNV-1a is a diagnostic hash, not cryptographic integrity.
8. **The fallback predictor is intentionally disposable.** In particular, replaying matching checkpoints is wasteful, and the six-tick run exposes its primitive clock/input buffering. Do not grow it into a second generic prediction framework before the requested upstream stack has a fair supported-toolchain evaluation.

## Original recommendation and proposed M2 boundary

The original recommendation was to retain D4/A10 and rerun the library evaluation after a parent-owned toolchain decision. D17 has now made that decision and the rerun is complete below. The proposed crate boundary and Rust API sketch remain applicable; the revised recommendation selects Lightyear/Replicon as the private implementation.

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

## Lightyear rerun

### Implementation and reproduction

Merged local `main` (fast-forward to `c44ac6b`) and used D17's Rust **1.99.1**, Cargo 1.99.1, LLVM **23.1.1**, on the same Windows/Ryzen/RAM hardware. All Cargo work used **four jobs**. Only the owned spike changed after that merge.

`src/ly.rs` and `src/ly/{server,client}.rs` are a sibling adapter. `--stack lightyear|renet` selects it in the shared process harness; Cargo features also allow **separate binaries with no dependency on the competing stack**. `Scene` now exposes the existing simulation through a caller-owned ECS world: Local owns a plain `World`; the native network authority owns that same scene inside a minimal `bevy_app::App`. There is no authoritative-world-to-replication-world mirror.

The four component and two intent declarations, validators, scene, scripts, state hash and replay format are shared. Gameplay imports no networking library. The input type gains conditional Bevy reflection; its backend-required `MapEntities` implementation is outside gameplay. The adapter validates the same Ara policy table before registering types upstream.

This is an actual library evaluation, not Lightyear carrying the old snapshot messages:

- Motor/door/inventory/item components and stable IDs are registered with Lightyear/Replicon. Replicon supplies baselines, changed-component replication and acknowledgement tracking. The final control message contains **only a digest**; a client can acknowledge it only when its replicated ECS components and native predicted motor converge. Clients never apply an expected-state artifact to repair themselves.
- Lightyear's native `InputPlugin<MoveInput>`, `ActionState`, input buffers, synchronized timeline, `PredictionManager`, `PredictionHistory`, confirmed histories, rollback detection and FixedUpdate replay are used. The custom Renet predictor is not compiled into a Lightyear-only binary.
- `PredictionTarget` selects the owner; `InterpolationTarget` selects the other recipients. Each final client is asserted to have **one predicted entity, one prediction history, three interpolated motors and one private inventory component**. Door/inventory do not participate in motor rollback.
- Owner-only inventory uses a Replicon `VisibilityFilter` with `SingleComponent<Inventory>` scope. This hides the private component without hiding the public character. Register the filter only on the server: a client-only App has no server `FilterRegistry`. A focused test constructs both roles and checks owner/other/missing-identity predicates.
- Listen uses the same direct in-process host endpoint as the Renet reference; remote clients use the actual library stack. Local constructs neither adapter nor App. Both stacks use the **same external seeded UDP conditioner**, below encrypted transport and reliability, rather than different message-level simulators.

From repository root:

```powershell
& .\spikes\s4-netcode\rerun.ps1
python .\spikes\s4-netcode\summarize.py
```

The first command runs gates, separate fresh-target builds, Local measurements and five process scenarios per stack. The second aggregates the raw results and asserts all six gates for both stacks. It uses Python's standard library only. Individual examples from `spikes/s4-netcode/`:

```powershell
$env:CARGO_BUILD_JOBS = '4'
cargo run --locked --release --no-default-features --features lightyear-stack -- harness dedicated4 impaired 21668 artifacts/rust199/lightyear-dedicated4-impaired.json --stack lightyear
cargo run --locked --release --no-default-features --features renet-stack -- harness dedicated4 impaired 21668 artifacts/rust199/renet-dedicated4-impaired.json --stack renet
cargo run --locked --release --no-default-features -- local
```

`rerun.ps1` records executable SHA-256 hashes and runs scenarios from those exact binaries. It captures build logs, bounds each build to ten minutes and samples available RAM. Lowest sampled availability was **2,320,656 KiB (2.21 GiB)** during the Lightyear build. Builds/scenarios were sequential, with no compiler competing with the measured Local loops or socket scenarios. The PC remains shared; this is not an isolated performance laboratory.

### Six gates, both stacks

| Gate | Lightyear + Replicon | Ara reference over Renet |
|---|---|---|
| Local / Listen / Dedicated mode parity | **PASS** | **PASS** |
| Late join after door and pickups changed | **PASS** | **PASS** |
| 100 ms nominal RTT, ±20 ms one-way jitter, 5% directional loss; convergence after flush | **PASS** | **PASS** |
| Authoritative replay hash, all 600 tick checkpoints | **PASS** | **PASS** |
| Inventory privacy | **PASS** | **PASS** |
| Local zero serialization / network resources | **PASS** | **PASS** |

All primary four-player modes end at **`e013eb0e61ea0d88`**, matching Local. Dedicated-three matches the three-player Local script at **`65aa66aa3f856001`**. Both stacks consume all **840 non-neutral input ticks** in the primary four-player impaired run, accept five intents and reject the rapid second door toggle. Both replay the persisted trace independently, not just its in-memory copy.

Impaired late baseline: Lightyear dedicated tick **220**, listen **218**; Renet dedicated **198**, listen **197**. Every late baseline has an open door, exactly three collected pickups and only its recipient's inventory; player 3 subsequently receives its own pickup. Extra Lightyear connection/replication setup explains a different join tick; no join-latency target was specified.

`cargo check --all-features --all-targets`, Clippy with warnings denied (all features and network-free), **7 all-feature tests + 3 network-free tests**, and **10 socket/process scenarios** passed. The old compatibility probe now succeeds: [compiler record](artifacts/rust199/compatibility-check.txt).

Raw data and computed gates: [summary.json](artifacts/rust199/summary.json). All native counts, including selective prediction and rollback events, are embedded in the committed per-client reports; runtime logs/replays remain in the ignored per-run directories.

### Bandwidth and corrections

Same four players, 50 logical replicated entities, 60 Hz simulation, 20 Hz replication and authority seconds 5–9 measurement window. Units and header exclusions are the same as the original measurement: **offered encrypted UDP payload bytes/s**, including protocol/ACK overhead, before impairment; no IP/UDP/Ethernet headers. Forty-five pickups are static during this window. Renet sends full snapshots; Replicon uses change tracking, so this compares complete candidate stacks, not only raw transport efficiency.

| Client | Lightyear down / up, bytes/s | Renet down / up, bytes/s |
|---|---:|---:|
| 0 | 2,991.7 / 4,837.3 | 4,352.1 / 8,823.1 |
| 1 | 2,959.7 / 4,828.3 | 4,319.6 / 8,779.9 |
| 2 | 2,956.7 / 4,859.0 | 4,312.1 / 8,784.9 |
| 3 | 2,991.9 / 4,802.0 | 4,302.6 / 8,791.9 |
| Mean | **2,975.0 / 4,831.6** | **4,321.6 / 8,795.0** |

Lightyear used **31.2% less downstream and 45.1% less upstream traffic** in this workload. Native input sequence encoding and changed-component replication are functionality the Renet reference does not implement. This is not a fundamental bandwidth limitation of Renet. Actual whole-run losses were Lightyear **5.12% down / 5.20% up**, Renet **4.44% down / 5.41% up**. A seed chooses impairment samples, not identical packet counts across different protocols.

Primary correction magnitudes/frequency: **both zero** (Lightyear **685** checked owner checkpoints, Renet **715**). To avoid declaring prediction proved by an error-free trajectory, the extra Lightyear live-profile probe perturbs owner 0's prediction by **250 mm at tick 330**. Its native metrics show **one rollback, 12 replayed ticks**, and one same-tick **250 mm** correction among **677** checked checkpoints; the final authoritative hash still matches Local. Other owners had zero corrections. No custom restore or final snap was applied.

The Renet reference's six-tick stress profile records **224/707** nonzero corrections, mean **48.44 mm**, maximum **125 mm**; only **406/840** intended moving-input ticks arrive on time. Its final state differs from the ideal trajectory but converges and replays exactly (`f66b2ff29b644d38`). Lightyear's corresponding tighter-jitter-margin profile consumes all 840 and preserves the ideal result. These stress figures are **not an identical-timing-policy correction benchmark**: the reference uses fixed lead/neutral fallback, while native synchronization adapts its timeline and predicts missing input, and the native probe adds the deliberate prediction fault. The primary scenarios and conditioner are shared; these extra probes characterize and verify each predictor.

The legacy `prediction_lead_ticks` report field selects the harness's normal/tight profile. It is a fixed lead in the reference, **not a fixed Lightyear prediction depth**. Native settings are no input delay, jitter safety margin 6 ticks normally / 1 tick in the extra probe, with library RTT/jitter synchronization. The API allows input delay and prediction-window bounds independently. M2 must expose deliberate timing policy, not present these profile labels as measured lead.

### Clean build, size and Local cost

Fresh, previously nonexistent target directory per configuration; **offline cached registry/toolchain**, incremental compilation disabled, four jobs, identical release profile (`opt-level=3`, `debug=1`, no added LTO/stripping). No compiler wrapper or custom Rust flags were set. Wall time includes Cargo/process overhead. PDB files are reported separately and excluded from executable size.

| Build | Clean time | Executable bytes | PDB bytes | Normal dependency packages |
|---|---:|---:|---:|---:|
| Lightyear-only | **171.07 s** | **12,296,704** | 68,448,256 | 189 |
| Renet-only | **32.97 s** | **1,257,984** | 6,582,272 | 93 |
| Local-only, networking excluded | **27.85 s** | **422,912** | 3,534,848 | 63 |

[Build settings, hashes and RAM samples](artifacts/rust199/builds.json); adjacent `build-*.stderr.txt` files preserve compiler output. Lightyear's cold build is **5.19×** and its executable **9.78×** this small Renet reference. The reference implements much less general netcode; these costs are real but do not establish an equally capable custom stack would retain the reference's size.

Local reports in **all three binaries** have zero gameplay serialization, zero network resources, zero transport objects and zero history entries; ECS resources equal the fresh-World baseline of one. The network-free graph has neither `bevy`, `bevy_app`, Lightyear, Replicon nor Renet. Thus a Local-only export can exclude the networking build and binary cost as well as runtime facilities. A multiplayer-capable executable retains its larger linked binary even when running Local; do not claim otherwise.

Same Local microbenchmark as before (8 retained samples × 200,000 ticks):

| Binary | Bare motor loop, ns/tick | Identical direct simulation, ns/tick | Local, ns/tick | Local / direct |
|---|---:|---:|---:|---:|
| Local-only | 30.920 | 43.744 | **43.877** | **1.003×** |
| Renet-capable | 36.834 | 56.249 | 59.754 | 1.062× |
| Lightyear-capable | 31.692 | 47.123 | 46.432 | 0.985× |

Cross-build differences and the 6% reference-wrapper variation are shared-host/code-generation noise, not a claim that one linked library accelerates Local or that every wrapper is provably cycle-free. Structural absence of network work is the D4 guarantee; the bare loop also omits simulation validation and clock work.

### Fit and remaining gaps

**R09's simulation feature fits.** Ara declares typed `MoveInput`, complete restorable `Motor` and one `step_motor`; native systems call it in FixedUpdate on authority and predicting owner, and the upstream rollback pipeline calls it again during replay. Scope assertions and the injected-error recovery demonstrate selective owner prediction rather than prediction of the entire scene. Non-motor state stays server-authoritative. Lightyear does not prove the completeness/purity of arbitrary gameplay state: Ara must enforce declared dependencies, deterministic context and ECS-only script state.

**Local can avoid App/runtime machinery; a network build cannot avoid all transitive Bevy crates.** Our code imports selected crates, never umbrella `bevy`, and enables no DefaultPlugins, renderer, window or GPU integration. Replicon's published dependency still brings the minimal umbrella facade and `bevy_internal`, plus App/time/state/reflection, input, transform, math/shape/curve and diagnostics crates. Neither network graph includes `wgpu`, `bevy_render` or `bevy_winit`. Removing that facade entirely would require an upstream dependency change; it is not a runtime cost paid by `LocalSession`. The measured native client has **69 resources and 256 total ECS entities**, including observers/transport bookkeeping, with **50 game replicas**; Local has none of this setup.

**Delta and priority are present in these releases.** Do not repeat the old migration note as a current blanket feature gap. Published source exposes `replicate_diff` / `predict_diff`; diffs require explicit `EntityDiffExt::apply_diff` mutation tracking. This spike uses ordinary changed-component replication, not that diff writer path. Replicon also has actual entity/per-client mutation priorities (`ReplicatePriority`, `PriorityMap`), distinct from replication-rule precedence. Lightyear adds channel/message priorities and packet bandwidth admission. These APIs were inspected, not subjected to a congestion/starvation benchmark here; Ara still needs generated policy bindings and budget tests.

**Authority migration is not established.** Server authority and input ownership are separate. `ControlledBy`/prediction-target reassignment is not a tested trusted-authority handoff or host migration implementation. One explicit `ReplicationMode::Server` routing branch remains `unimplemented!` for server-scoped membership in the pinned source. Keep M5's authority-switching/host-migration work and tests; it is not a blocker for the proved single-authority M2 model.

Other M2 work remains: generated field codecs/IDs and protocol version compatibility, arbitrary initial-world replay snapshots, dynamic entity generations/lifecycle, reconnects, interest transitions, bounded adversarial decode/queues, physics/C# prediction and scale. Mixed public/private fields inside one logical Ara type need generated scoped wire components/projections; the proved visibility scope here is a whole inventory component. Upstream machinery does not eliminate Ara's schema, validation, session or replay responsibilities.

Source inspection is of the **published pinned crates**, not floating main. Lightyear package VCS commit: `8a926d6fece6a0cf04c2747de90c01258524fe0d`. The `.cargo_vcs_info.json` files identify these upstream source paths: [native inputs](https://github.com/cBournhonesque/lightyear/tree/8a926d6fece6a0cf04c2747de90c01258524fe0d/crates/inputs/inputs_native), [prediction configuration](https://github.com/cBournhonesque/lightyear/blob/8a926d6fece6a0cf04c2747de90c01258524fe0d/crates/replication/prediction/src/manager.rs), [diff registration](https://github.com/cBournhonesque/lightyear/blob/8a926d6fece6a0cf04c2747de90c01258524fe0d/crates/replication/replication/src/registry/replication.rs). See also [Replicon visibility/priority documentation](https://docs.rs/bevy_replicon/0.45.2/bevy_replicon/).

**Recommendation: adopt Lightyear + Replicon for M2.** It passes the same behavioral gates with the same declarations, provides the actual input/history/rollback/interpolation/lifecycle substrate, reduces traffic in the measured scene and keeps Local independent. The substantial cold-build/binary cost is isolated behind optional `ara-net`. Building an equally capable Ara layer on Renet/Quinn would take ownership of much more netcode than this reference proves. Keep the Renet spike for regression/comparison, not as a second shipping replicator or a growing generic prediction framework.

## Proposed decision

> **S4: adopt Lightyear 0.31 + Replicon 0.45.2 behind Ara-owned schemas for M2 Multiplayer v1, on D17's Rust 1.99.1.** Local/Listen/Dedicated character, door, private inventory, late join, adverse-link convergence, privacy and same-machine replay gates pass for both candidates. Use upstream native input buffering, selective owner prediction/rollback, interpolation and Replicon component replication; gameplay imports no backend types. Keep `ara-sim` independent and `ara-net` optional: ordinary Local allocates no network resources/history and serializes no gameplay state, while Local-only builds exclude the network/App dependency graph. Generate registrations, recipient filters and codecs from `ara-schema`, retaining server-side typed-intent validation. Accept the measured native cold-build/binary cost for the capability and reduced integration ownership. Diff/priority APIs exist but still need Ara bindings and scale tests; authority switching/host migration remain M5 work. Keep the Renet reference disposable and the same scenario/replay gates as the adapter contract.

This is proposed text for the parent agent. No locked decision, milestone, root README or AGENTS file was edited after merging `main`.
