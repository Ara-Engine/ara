# Decision log

Locked decisions for Ara. To change one, add a new entry that supersedes it; don't quietly diverge in code. The evidence lives in [`research/`](research/00-synthesis.md).

## Owner decisions

| ID | Decision | Date |
|---|---|---|
| D1 | **Core in Rust.** Performance must be top-notch. | 2026-10-22 |
| D2 | **AI agents build the engine; agents and humans use it to make games.** Both the codebase and the product are agent-first. | 2026-10-22 |
| D3 | **Targets:** Windows, Linux, macOS first (PC is the top priority); Android and iOS second. No consoles for now. | 2026-10-22 |
| D4 | **Multiplayer is native from day one.** Single-player stays first-class and pays nothing for it. | 2026-10-23 |
| D5 | **Game UI is native**, in the style of Unity's UI Toolkit (web-shaped, not a browser). One system covers screen UI and world UI. | 2026-10-23 |
| D6 | **The editor UI is native**, built on that same UI system. | 2026-10-23 |
| D7 | **No scope cuts.** Plans express build order only. | 2026-10-23 |
| D8 | **2D is supported but secondary.** No Unity project import for now. | 2026-10-23 |
| D9 | **No CI for now.** Gates run locally until this is revisited. | 2026-10-23 |
| D10 | **Reference code:** Flax, Unity and Unreal may be studied for ideas; we write our own implementations. Godot and O3DE may be copied with attribution. | 2026-10-23 |
| D11 | **C# is the single scripting language**: gameplay, UI logic, editor extensions and agent scripts. C# 14 / .NET 10 LTS. It stays unless the S2 reload gates or the S7 AOT gates fail (see R10). | 2026-10-23 |
| D12 | **GPU: own native backends from the start.** `ara-gpu` with Vulkan 1.3 (Windows, Linux, Android) and Metal 4 (macOS, iOS). No wgpu. D3D12 only on evidence. | 2026-10-23 |
| D13 | **Team:** Islam plus agents. | 2026-10-23 |
| D14 | **License not decided.** The repo is private until it is. | 2026-10-23 |
| D15 | **Apple:** Apple silicon only; Intel Macs dropped for now. A MacBook Pro M5 (Apple10: hardware RT and mesh shading) is the Metal dev and Mac/iOS build machine. | 2026-10-23 |
| D16 | **Dogfood game: Outpost**, a co-op third-person salvage run built on the engine as it grows (see [milestones](plan/milestones.md)). | 2026-10-23 |
| D17 | **Toolchain: Rust 1.99.1, the latest stable**, pinned in `rust-toolchain.toml`. Track stable deliberately: bump when a dependency needs it or roughly every release cycle, in one commit that updates the pin and `rust-version` together. (The original 1.96 pin was just the version installed on the machine; Bevy 0.20 and Lightyear need ≥1.97.1.) | 2026-10-23 |

## Architecture (adopted from research)

These are working decisions. A spike result or a new entry here can change them.

| ID | Decision | Source |
|---|---|---|
| A1 | **Object model.** Entities in a hierarchy with plain reflected components in `bevy_ecs`. *Behaviours* are per-entity C# objects that hold no authoritative state; *Systems* are batched queries with declared reads/writes, scheduled in parallel. Prefab overrides are addressed by ID. | R01, R10 |
| A2 | **Schema DB is the single source of truth.** Rust derives and C# source generators feed it. It carries units, ranges, required docs and replication policy, and generates serialization, the inspector, bindings, JSON Schema, the MCP catalog, docs and net codecs. | R01, R03, R07 |
| A3 | **Files.** Canonical strict JSON; defaults omitted; a flat entity map keyed by short IDs; `@path#id` references; `ara fmt` and `ara merge`. Binary only for source media. Derived data lives in a content-addressed cache. | R01, R03 |
| A4 | **Mutation.** Typed command → validated ops with automatic inverses → transaction attributed to a human or an agent. Agent edits apply live but are shown as unreviewed until accepted or reverted. Network gameplay commands are intents, never edits. | R01, R03, R04, R09 |
| A5 | **Engine Protocol.** JSON-RPC 2.0 over WebSocket/stdio, localhost plus token; the editor uses it too. A generated MCP adapter exposes about 12 tools. | R01, R03 |
| A6 | **Processes.** One engine library; `ara editor`, `ara` (CLI/daemon) and `ara-player`. Play mode runs in a child process. Importers are pure functions in worker processes. | R01, R03, R04 |
| A7 | **UI stack.** Retained tree; Taffy layout; Parley text, with HarfBuzz via FFI wherever coverage is missing; engine display lists; AccessKit. HTML-like markup plus a CSS-subset stylesheet with typed C# bindings. Focus and input in the style of Unreal's CommonUI. | R05, R07, R10 |
| A8 | **Rendering.** SM6.6-style bindless; explicit render-graph barriers; multi-queue. Slang → SPIR-V, and Slang → DXIL → Metal Shader Converter → metallib. One material AST with a text view and a graph view. Tiers: RT, Raster, Mobile; baked/probe lighting is mandatory as the fallback. | R02, R08 |
| A9 | **Platform layer:** SDL3 via `sdl3-sys`. | R08 |
| A10 | **Multiplayer.** Server-authoritative snapshots, interpolation and selective owner prediction. Sessions: Local (zero serialization), Listen, Dedicated. Lightyear + Replicon evaluated behind our schemas. | R09 |
| A11 | **Reuse.** Bevy 0.20 crates (no fork). Kira and Steam Audio; gltf, ufbx, meshopt, KTX2; Recast; Tracy; Sentry minidumps. | R06, R07 |
| A12 | **Performance tiers (DOTS-class).** Engine systems and hot gameplay systems in Rust are the "Burst tier": the same LLVM backend, with `&mut` giving no-alias guarantees and SIMD via `glam`. C# systems run over chunk-batched spans with no per-entity calls, using explicit SIMD (`Vector128`/`Vector256`) and NativeAOT in shipping builds. A compiler for a C# subset through LLVM is in the build order, decided by the S8 benchmarks. | R07, R10 |

## Spike results (M0)

| Spike | Result | Decision | Evidence |
|---|---|---|---|
| S1, Vulkan half | **PASS** (2026-10-23) | **Adopt the ara-gpu binding/sync model**, as proven on Vulkan 1.3 (RTX 4070 SUPER), validation and sync validation clean.<br>• Binding: one global update-after-bind descriptor set (textures, buffers, sampler, TLAS) plus 32-byte push constants carrying SM6.6-style heap indices.<br>• Synchronization: timeline semaphores across a dedicated compute family, with explicit queue-family transfers.<br>• Shaders: offline Slang → SPIR-V + DXIL; no shader compiler at runtime.<br>• Persisted `VkPipelineCache`: 7.6 ms cold → 0.5 ms warm.<br>The Metal 4 half (MacBook Pro M5, via Metal Shader Converter) is still open. | [`spikes/s1-gpu/RESULT.md`](../spikes/s1-gpu/RESULT.md) |
| S3 | **PASS** (2026-10-23) | **Keep A7: one Ara-owned retained UI runtime** for the editor, HUDs and world surfaces, on Taffy 0.15 + Parley 0.12 behind our own boundaries.<br>• Measured (worst sample of 5,000): pause/HUD updates 0.33 / 0.41 ms; a 10k-item virtual inventory scroll 0.68 ms.<br>• Arabic, bidi, CJK and emoji render correctly; designer edits round-trip byte-exact; focus navigation and an AccessKit tree work.<br>• Rules: keep surviving virtual-list items and their shaped text; track restyle, layout/text and paint dirtiness separately; no HarfBuzz/ICU FFI until a coverage gap is shown.<br>• RmlUi rejected: the tested revision has no Grid, and it wouldn't remove our authoring and C# integration work.<br>Still open (M2/M3): GPU rendering at 120 Hz, OS IME and screen readers. | [`spikes/s3-native-ui/RESULT.md`](../spikes/s3-native-ui/RESULT.md) |
| S4 | **PASS on behaviour; library undecided** (2026-10-23) | **Keep D4/A10 and the schema boundary.** Gameplay declarations never import netcode types. Local / Listen / Dedicated character, shared door, private inventory, late join, 100 ms RTT with 5% loss, replay hash and inventory privacy all pass on a disposable `bevy_ecs` 0.20 + Renet 2.0 fallback. Local mode creates no network resources and serializes nothing (46.7 ns/tick, equal to direct simulation). Four players and 50 entities use 4.3 kB/s down and 8.9 kB/s up per client. Lightyear 0.31 + Replicon couldn't be built on the old 1.96 pin (they need 1.97.1); a rerun of the same gates on 1.99.1 decides the library. Don't grow the fallback predictor into a framework. | [`spikes/s4-netcode/RESULT.md`](../spikes/s4-netcode/RESULT.md) |
| S8 | **PASS** (2026-10-23) | **Keep A12, with "Burst tier" now a measured target rather than a claim.** All-core p50 on the 7800X3D: Unity IL2CPP+Burst vs Rust Bevy: transforms 0.53 vs 0.77 ms, 50k boids 4.77 vs 9.30 ms, particles 0.51 vs 0.73 ms, spawn/despawn churn 2.59 vs **1.17** ms. Single-threaded, Rust wins transforms, particles and churn. C# over Rust spans is close to Rust scalar, and **C# explicit `Vector256` particles hit 0.12 ms** (all-core). The gap is in **parallel ECS iteration and dispatch, not code generation**, so tuning ECS queries, layout and job dispatch becomes explicit work (M1 simulation core, then M2 profiling), aiming to match or beat DOTS on these workloads. The C#-subset LLVM compiler stays in the build order (M4) but only gets built if later profiling shows compiler-generated kernel gaps that explicit SIMD and Rust don't cover. | [`spikes/s8-dots-bench/RESULT.md`](../spikes/s8-dots-bench/RESULT.md) |
| S5 | **PASS** (2026-10-23) | **Keep A6 (play runs in a child process) and add a pool of pre-spawned, single-use players as the default.**<br>• Play → first tick: 3.5 / 4.3 ms p50/p95 at 5k entities and 30 / 38 ms at 50k.<br>• Live patch round trip: 8.9 / 16.3 ms p95, about one tick at 60 Hz.<br>• Snapshots go through shared memory as binary (474 KB at 5k, 4.8 MB at 50k); JSON is 13–19× slower.<br>• The host survives panics, access violations and hangs, with a structured error and a restart action.<br>• Keep changes works as one op transaction, with undo and redo.<br>• One server + 2 clients are running in 5 ms from the pool.<br>• In-process "simulate" stays a secondary mode (scene-view physics preview, batched headless tests, platforms that can't spawn processes).<br>• Cold costs to hide behind the pool: Vulkan device creation (~220 ms) and the first launch of a fresh binary (~150 ms). | [`spikes/s5-play-process/RESULT.md`](../spikes/s5-play-process/RESULT.md) |

## Open: decided by spikes (see [milestones](plan/milestones.md), M0)

- Physics: Jolt (via a binding we own) vs Rapier. **Lean: Jolt** (S6).
- Netcode library: Lightyear + Replicon behind our schemas (S4).
- ~~Play-mode process model (S5)~~: child process with a pre-spawned pool; in-process simulate is secondary (S5 PASS).
- Export lanes per platform with C# NativeAOT and ReadyToRun (S7).
- ~~C# performance-tier plan (S8)~~: explicit SIMD + Rust now; the compiler is built only on profiling evidence; ECS dispatch tuning is first (S8 PASS).
- ~~Whether Parley's text coverage is enough (S3)~~: yes for the tested corpus (S3 PASS).
