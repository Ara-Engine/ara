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

## Open: decided by spikes (see [milestones](plan/milestones.md), M0)

- Physics: Jolt (via a binding we own) vs Rapier. **Lean: Jolt** (S6).
- Netcode library: Lightyear + Replicon behind our schemas (S4).
- Play-mode process model: child process vs an in-process simulate fallback (S5).
- Export lanes per platform with C# NativeAOT and ReadyToRun (S7).
- C# performance-tier plan: whether to build the C#-subset LLVM compiler (S8).
- Whether Parley's text coverage is enough (S3).
