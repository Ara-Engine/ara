# Ara: research synthesis and decision record (v2)

*2026-10-23. Inputs: ten research reports, R01–R10 (Claude Opus 5.5 and GPT-6.1 Sol), plus the owner's decisions made during the research. The reports are the evidence; this file records what we decided and why. Where the reports disagree, §5 gives the resolution. v2 adds the owner's answers: C# confirmed, own GPU backends, team, Apple hardware, dogfood game, and the DOTS-class performance tiers. The live decision log is [`../decisions.md`](../decisions.md); the build plan is [`../plan/milestones.md`](../plan/milestones.md).*

*Naming: the engine is **Ara** (CLI `ara`). During research it had the working name "snengine", and reports R01–R10 still use that name and `sn` for the CLI.*

---

## 1. Owner decisions (locked)

| # | Decision |
|---|---|
| D1 | **The core is written in Rust.** Performance must be top-notch. |
| D2 | **AI agents build the engine, and agents plus humans use it to make games.** Both the codebase and the product must work well for agents. |
| D3 | **Targets:** Windows, Linux and macOS first, with PC the top priority. Android and iOS come second. No consoles for now. |
| D4 | **Multiplayer is native from day one.** Single-player stays first-class and pays nothing for it. |
| D5 | **Game UI is native**, in the style of Unity's UI Toolkit: it looks and behaves like the web but is not a browser. One system covers screen UI and world UI. |
| D6 | **The editor UI is native too**, built on that same UI system. |
| D7 | **No scope cuts.** Roadmaps set build order only. |
| D8 | **2D is supported but secondary.** No Unity project import for now. |
| D9 | **No CI for now** (we'll revisit). Quality gates run locally (`ara test`, `ara check`). |
| D10 | **Agents may study Flax, Unity and Unreal code for ideas** but write our own implementations. Godot and O3DE code may be copied with attribution. |
| D11 | **C# is the one scripting language** (§4.7). It stays unless the S2 or S7 gates fail. |
| D12 | **Own native GPU backends from the start** (Vulkan 1.3 + Metal 4). No wgpu. |
| D13 | **Team:** Islam plus agents. |
| D14 | **License not decided**; the repo stays private. |
| D15 | **Apple silicon only** (Intel Macs dropped for now). A MacBook Pro M5 is the Metal and Mac/iOS build machine. |
| D16 | **Dogfood game: Outpost** (§8). |

---

## 2. What Ara is

Ara is a Rust engine with a typed **schema, command and query system** at its centre, over a world stored as **text**.

- The native editor, the `ara` CLI, C# gameplay code, AI agents (through MCP) and the netcode are all clients of that same core.
- Agents get no separate "AI API": every edit is a typed command, undoable, attributed to whoever made it, and reviewable.
- Every asset is diffable text with stable IDs.
- The simulation is fixed-tick and deterministic enough to test, replay and network.
- Rendering runs on its own thin Vulkan and Metal 4 layer, built for Unreal-class fidelity.
- One native UI system draws the editor, game HUDs and world UI.

That is Unity's layering (native core, UI Toolkit, C#), rebuilt in Rust and designed around agents from the start.

---

## 3. Architecture at a glance

```
┌─────────────────────────────────── Ara core (Rust) ───────────────────────────────────┐
│ Schema DB (reflection) · Document store + op log · Command / Query / Job registries   │
│ bevy_ecs worlds · fixed-tick sim · scheduler · sessions (local/listen/dedicated)      │
│ Assets: pure importers → content-addressed cache · cooker · packs                     │
│ Renderer: render graph → ara-gpu (Vulkan 1.3 | Metal 4) · Slang shaders · materials   │
│ UI runtime (Taffy layout, Parley text, native display lists) · physics · audio · anim │
│ Netcode (replication, prediction) · C# host (CoreCLR dev / NativeAOT ship)            │
│ Diagnostics bus · capture / replay · platform layer (SDL3)                            │
└──────────────────────▲──────────────────────────────────────────▲─────────────────────┘
                       │ Engine Protocol: JSON-RPC 2.0, WebSocket/stdio, localhost + token
   ┌───────────────┬───┴───────────┬───────────────┬──────────────┴──┬────────────────┐
 ara editor      ara CLI         MCP adapter     ara_run worker    ara-player        ara-bridge
 (native UI,     (headless +     (~12 generated  (C# agent         (play / client /  (in dev builds
 same process)   offline ops)    tools)          scripts, out of   server, child     on devices)
                                                 process)          process)
```

---

## 4. Decisions by area

### 4.1 Object model and identity (R01, R10)
- **Entities in a hierarchy carry plain reflected components, stored in `bevy_ecs` archetype tables.** Two authoring tiers sit on top:
  - **Behaviours** are per-entity C# objects that feel like MonoBehaviour but hold no authoritative state.
  - **Systems** are batched queries that declare what they read and write, so the scheduler can run them in parallel automatically.
- The hierarchy is a `ChildOf` relationship plus an `order` key. Transform is an ordinary component. There are no deep class hierarchies.
- Prefabs are asset subtrees. An instance is a patch layer of overrides addressed by ID, never by array index. Prefabs can nest and have variants, and list items that need their own overrides carry IDs.
- Events are typed, with observers. There is one messaging system.
- Identity comes in three kinds, and they are never mixed:
  - runtime: 64-bit generational handles;
  - files: short prefixed IDs, unique per file (e.g. `e_8b21`), plus asset IDs unique per project;
  - network: session-scoped IDs with an epoch.

### 4.2 Schema DB: one source of truth (R01, R03, R07, R10)
- **Declaration:** in Rust, derive macros (`bevy_reflect` for runtime reflection; serde + schemars for wire format and JSON Schema). In C#, attributes and source generators register into the same database. The runtime registry also accepts dynamic, data-only types defined live.
- **Metadata:** units, ranges, enums, defaults, examples, `since`/`deprecated`, replication policy and editor hints. **Doc comments are required.**
- **What it generates:**
  - serialization
  - the inspector UI
  - undo (through ops)
  - C# bindings
  - JSON Schema
  - the MCP command catalog
  - docs and `llms.txt`
  - network codecs
  - graph nodes
- Prototype `facet` on one isolated document type before ever considering it engine-wide.

### 4.3 Files (R01, R03)
- **Canonical strict JSON** with a `$schema` reference.
  - A deterministic writer omits default values.
  - Entities form a flat map keyed by ID, with `parent` and fractional `order` fields, so a reparent changes one line.
- **References** are written `@path#id`. The path is for readability and the ID for robustness. Whichever half breaks, the loader resolves through the other and repairs the file.
- Only *source media* (png, glb, wav) is binary, with an `.import.json` sidecar. Derived data lives in a content-addressed cache and is never committed.
- Tooling:
  - `ara fmt` and `ara merge`, a semantic git merge driver;
  - per-entity or per-cell files for large worlds;
  - a cooked binary format for the runtime and for fast editor loads.

### 4.4 Commands, ops, undo and review (R01, R03, R04, R09)
- **Every authoring mutation is a typed command:** validated, then turned into ops (`create`, `set`, `delete`, `move`) with automatic inverses.
- **Transactions** carry the author (human or agent), an optional `why`, revisions and an idempotency key. A slider drag commits as one gesture, and an agent turn commits as one transaction.
- **The registry distinguishes commands, queries and jobs.** Each entry generates its CLI form, command-palette and menu entries, MCP catalog entry and docs.
- **Undo applies inverses after revision checks.** An edit made against an outdated revision is rejected rather than overwriting.
- **Agent edits are applied but marked unreviewed.** The editor shows them:
  - outlined in the viewport and hierarchy;
  - with per-field before/after values;
  - with before/after captures;
  - with accept or revert per turn.
- **Gameplay network commands are a separate class.** They share the schema and validation, but a client sends an *intent* (`UseMedkit`), never an edit (`SetHealth`).

### 4.5 Engine Protocol and the AI-native layer (R01, R03)
- **Protocol:** JSON-RPC 2.0 over WebSocket or stdio, localhost only, with a per-project token and an approval prompt on first connection. The editor uses it too, so the protocol can never lag behind what humans can do.
- **MCP adapter**, generated from the registries, exposes about 12 tools:
  - `query`, `inspect`, `schema`, `apply`
  - `run`, `capture`, `play`, `test`
  - `logs`, `session`, `docs`, `jobs`

  Agents find commands by searching the schema. MCP is a thin layer over our own protocol because the MCP spec keeps changing.
- **Visual feedback** is a real API:
  - camera specs;
  - debug view modes;
  - set-of-marks entity labels with a legend that maps each mark to an entity;
  - deterministic capture and golden-image comparison;
  - `capture.ui`, which returns the computed layout rectangles.
- **Deterministic play** for testing behaviour:
  - fixed tick and seeded RNG streams;
  - input injected at the action level ("Jump", "Move");
  - `step`, `watch` and `until` controls, tick-level assertions, and record/replay.
- **Diagnostics** carry stable codes, structured locations and fixes the agent can apply directly. `ara check` validates a project; `ara why <entity>` answers questions such as "why isn't this visible?".
- **Docs, skills and AGENTS.md** are generated per engine version.
- **arabench**, a suite of 50–100 agent tasks, measures the differentiator. It runs locally while there is no CI.
- **Sessions carry capability scopes.** `ara_run` executes C# in a warm out-of-process worker whose only write path is the op log, so its edits are reviewable.
- **AI asset generation** is an optional, provider-agnostic job that records provenance.

### 4.6 Process model (R01, R03, R04)
- **One engine library, three binaries:**
  - `ara editor`: the engine plus the native editor UI, in one process;
  - `ara`: the CLI and headless daemon, same core;
  - `ara-player`: the runtime, as standalone, client or server.
- **One authority per project/worktree:** the editor if it is open, otherwise the daemon. The CLI and MCP connect to it, and the CLI can also work offline on files. Parallel agents each get their own git worktree and daemon.
- **Play mode runs in a child process.** That isolates crashes, guarantees clean state, and lets multi-client network testing reuse the same machinery.
  - Gate: process boot under 1 s, live patch under 50 ms.
  - Fallback if the gate fails: an in-process "simulate" mode.
- **Importers are pure functions** running in worker processes and feeding the content-addressed cache.

### 4.7 Scripting: C# (confirmed, D11) (R10)
- **C# 14 on .NET 10 LTS**, tracking each LTS. One language covers gameplay, UI logic, editor extensions and agent scripts. There is no JavaScript runtime.
- **Runtime:** CoreCLR with JIT in the editor and dev builds. NativeAOT ships Windows, Linux, macOS and iOS (supported since .NET 9). Android NativeAOT is experimental, so it is proven early, with a Mono/CoreCLR fallback.
- **Hosting:** `netcorehost`/hostfxr, a generated `#[repr(C)]` ABI in both directions, and chunk-batched spans over Rust component columns.
  - Measured on Islam's machine: 1.74 ns per entity in C# vs 1.09 in Rust, and 93 ms runtime start.
  - Calls are never made per entity.
- **Gameplay state lives only in ECS columns.** Reload, save, replication, prediction and agent inspection all rely on that.
- **Reload layers.** Modules are capped at about 5k lines. They start "cold" (optimized); the first edit makes a module "hot" (debug IL) so body edits can patch it live.

  | Layer | What it covers | Target (p50 / p95) |
  |---|---|---|
  | L0 | Data and UI edits | ≤30 ms |
  | L1 | Method-body edits in a hot module | ≤40 / ≤100 ms (measured: 4–25 ms emit + <3 ms apply) |
  | L2 | Additive edits (new members and types) | ≤60 / ≤150 ms |
  | L3 | Module swap | ≤200 / ≤450 ms |
  | L4 | State-preserving restart | 1–5 s |
- **Analyzers ARA0001–ARA0400**, with automatic fixes where possible:
  - Unity-isms (e.g. `GetComponent`, `void Update()`) and their Ara equivalents;
  - mutable statics;
  - non-deterministic code in simulation;
  - allocations on tick paths;
  - AOT-unsafe APIs.
- **Prior art:** s&box (MIT) for its compile pipeline, hot-reload classifier, IL whitelist and Razor UI.
- **Rust stays the tier for heavy systems.**
- **Reverse to TypeScript if any of these happen:**
  1. the reload targets are missed;
  2. NativeAOT cannot be linked into the app on iOS or Android;
  3. arabench shows C# more than 10 points behind a TS control;
  4. web export becomes first-tier.

### 4.8 UI: one native system for the game and the editor (R05, R04, R10, owner)
- **Runtime:** a retained element tree with:
  - **Taffy** layout (flex, grid, block);
  - **Parley** text, checked against R05's script list (Arabic/bidi, Indic, CJK, emoji, IME), falling back to HarfBuzz via FFI wherever coverage is missing;
  - engine-native display lists inside the render graph (batching, clipping, glyph caches, vector paths; evaluate Vello);
  - **AccessKit** for accessibility.
- **Authoring:** HTML-like markup plus a stylesheet that is a subset of CSS.
  - Design tokens and an optional, finite set of utility classes.
  - Typed bindings to C# view models; C# controllers.
  - The declarative subset guarantees lossless visual editing: stable node IDs, byte-identical saves when nothing changed, and edits that patch only what changed.
- **Input and focus** follow Unreal's CommonUI: screen stacks, gamepad navigation, focus restoration, per-player focus, and back/cancel routing.
- **World UI** is the same tree on a surface or texture, with ray-to-local input.
- **Tools:**
  - a UI Designer that edits the same files;
  - component stories;
  - structured inspection (computed boxes, overflow, focus graph, batch-break causes);
  - Testing Library-style semantic tests.
- **The editor is built on this system.** Its panels are written in the same markup, and Zed, Figma and Linear set the polish bar. Docking, virtualized trees and tables, the graph canvas, timelines and text input are the framework's first major workload.
- **Gate:** before building a wide widget catalog, prove a pause menu, a virtualized inventory, a world panel and the inspector panel, and compare against RmlUi.

### 4.9 Editor (R04, R01)
- **Unity's property model:**
  - inspectors generated from the schema;
  - custom drawers that emit standard commands;
  - multi-object editing;
  - an indicator showing where a value comes from (default, prefab, variant, instance, animation or runtime).
- **v1 panels:**
  - Scene viewport and Game viewport
  - Hierarchy and Inspector
  - Assets (with the import queue and dependencies)
  - Console/Problems
  - Agents/History (the review overlay)
  - Material graph
  - UI Designer
  - Profiler/Frame inspector
  - a command palette
- **Graph framework:** shared interaction plus graph documents stored as text (JSON, stable IDs, layout kept in a separate section), with a compiler per domain. Materials come first, then VFX module stacks, animation state machines, behaviour trees, dialogue and visual scripting.
- **Play mode is disposable.** "Keep changes" is explicit, with a preview.
- **Multiplayer play:** Local, Host + N clients, or Dedicated + N clients, with network-condition presets.

### 4.10 Rendering (R02, R08, R07)
- **RHI:** our own thin `ara-gpu`, with **Vulkan 1.3** (Windows, Linux, Android) and **Metal 4** (macOS, iOS) backends.
  - D3D12 is added only on evidence. The shader pipeline already emits DXIL, so only the backend would be new work.
  - **Decided (D12): own backends from the start.** Spike S1 validates the native path on Windows and on the M5.
  - Borrow from wgpu-hal and blade for swapchain and surface code.
- **Binding model:** SM6.6-style bindless heaps plus root constants. The render graph emits barriers explicitly, with async-compute and copy queues synchronized by timeline semaphores (`MTLSharedEvent` on Metal), and owns transient-resource aliasing.
- **Shaders:** Slang, single source.
  - Vulkan: SPIR-V.
  - Apple: DXIL → Metal Shader Converter → metallib. The converter runs on Windows.
  - Compiled offline per platform, tier and variant, with PSO manifests and persistent pipeline caches.
  - Hot reload swaps at a frame boundary and keeps the last good version on failure.
- **Materials:** one typed expression AST, with the same data shown as a text DSL and as a graph. Definitions are separate from instances, lighting is owned by the engine, and UI has its own material domain.
- **Tiers:** RT, Raster and Mobile, sharing one render graph and one material AST. Baked/probe lighting is mandatory as the fallback.
- **Fidelity sequence (nothing cut):**
  1. clustered deferred + forward, PBR/IBL, shadows, baked GI + probes, TAA, exposure and tonemapping;
  2. hardware-RT DDGI, RT reflections and shadows, denoising, DLSS/FSR/XeSS/MetalFX, a reference path tracer;
  3. a persistent GPU scene, GPU culling and indirect draws, texture streaming, Niagara-style VFX stacks, HDR output;
  4. virtualized geometry (meshlets, cluster LOD, streaming), virtual shadow maps, ReSTIR many-lights;
  5. layered materials, skin and hair, volumetrics and atmosphere, foliage, PCG, realtime path tracing, frame generation.
- **Verification:** deterministic captures, per-pass and per-resource dumps, GPU timing distributions, and golden scenes (energy-conservation furnace test, thin-wall light leakage, disocclusion, foliage, HDR UI).

### 4.11 Platforms and export (R08, R10)
- **Platform layer:** SDL3 via `sdl3-sys`, covering windows, input, gamepads, IME, touch, mobile lifecycle and audio devices.
- **Floors:**
  - Windows 10+ x64 (RT tier: RTX 20+, RX 6000+, Arc);
  - Linux x64 and Steam Deck (Steam Runtime);
  - macOS 26+ on Apple silicon (Metal 4; hardware RT from M3);
  - iOS 26+ on A14+;
  - Android 10+ arm64, Vulkan 1.1+ with descriptor indexing (coverage still to be measured).
- **Content cooking:**
  - textures: BCn on PC/Mac, ASTC on mobile;
  - audio: Opus;
  - shaders: per platform and tier;
  - packs: deterministic and content-addressed; patches are chunk diffs.
- **Export.** R08 designed a Godot-style "prebuilt runtime + pack" lane, which assumed an interpreted script runtime. **C# changes that: game assemblies need compiling per platform.**
  - Windows builds locally.
  - Linux via WSL2/container.
  - macOS and iOS through a Mac agent.
  - Desktop CoreCLR + ReadyToRun as an option that may cross-compile from Windows.
  - Verify all of this in the export spike (S7), because cross-OS NativeAOT is not supported out of the box.
- **Agent-friendly export:**
  - `ara doctor`, `ara export`, `ara run` and `ara device` emit NDJSON events with stable codes and runnable fixes.
  - **ara-bridge** in every dev build provides logs, screenshots, GPU timings, input injection and semantic queries on every platform.
- **Hardware:** the RTX dev box; the MacBook Pro M5 (Apple10) for Metal, macOS/iOS packaging and an iPhone; and 2–3 Android phones. These are on-demand build agents, not CI.

### 4.12 Multiplayer, native from day one (R09)
- **Default model:**
  - the server is authoritative;
  - remote entities are interpolated from snapshots;
  - the owner's own entity is predicted and reconciled only where it matters (e.g. movement).
- **Sessions:**
  - **Local:** one world, no serialization, no network code in solo exports.
  - **Listen:** a player hosts.
  - **Dedicated:** a renderer-free Linux server.
- **Schema policies:** replication audience, quantization, interpolation, rollback participation and private state. Network IDs are stable and carry epochs. Commands are intents, validated by the server and rate-limited.
- **Simulation features** for prediction: typed input, restorable state and one step function, run identically offline, on the server and on the predicting client.
- **Implementation:** evaluate Lightyear 0.31 + Replicon behind our own schemas. Acceptance test: a character, a door and an inventory behave identically offline, listen and dedicated, including late join, packet loss and replay.
- **Transport:** encrypted UDP by default, Steam Networking for PC friend-hosting, and hosting providers behind an allocator interface.
- **Tools:**
  - one-click server + N clients;
  - network-condition simulation with recorded seeds;
  - a bandwidth profiler per field;
  - replay that points at the first diverging field and tick;
  - headless bots for agents.
- **Built later, never cut:** lockstep, GGRS-style rollback, shared authority, host migration and multi-server worlds.

### 4.13 Other subsystems (reuse first)

| Area | Choice | Note |
|---|---|---|
| ECS / scheduler / reflection / tasks | Bevy 0.20 crates (`bevy_ecs`, `bevy_app`, `bevy_reflect`, tasks, math), pinned together | Do not fork Bevy; it restricts AI-written PRs, so we could never upstream |
| Physics 3D | **Jolt** via a thin binding we own (AAA-proven, deterministic mode); Rapier is the alternative | Decided in spike S6 |
| Physics 2D | Rapier 2D or Box2D v3 | Chosen with S6 |
| Audio | SDL3 device + Kira (mixer) + Steam Audio (spatialization) | FMOD/Wwise as plugins users license themselves |
| Import | gltf / ufbx / meshopt / KTX2 + Basis; OpenUSD import later | Importers run out of process |
| Animation | Own runtime graph; ozz-animation as reference; ACL compression later | |
| Navigation | Recast/Detour via bindings (or landmass) | |
| Profiling / crash reporting | `tracing` + Tracy + GPU timestamps; crash-handler + minidumper + Sentry | |
| Netcode | Lightyear + Replicon (evaluated in S4) | Fallback: Naia or Renet/Quinn |

### 4.14 Performance tiers: reaching Unity DOTS-class performance (R07, R10)

| Unity piece | What makes it fast | Ara equivalent |
|---|---|---|
| Entities | Component data in tight per-type arrays, in chunks; no managed objects | `bevy_ecs` tables use the same layout; plain struct components; built-in change ticks |
| Jobs | Work-stealing threads plus a dependency/safety system | Systems declare reads/writes and are scheduled in parallel. Safety is checked at compile time in Rust; C# systems get generated read/write sets. Chunk batches spread across cores. |
| Burst | LLVM optimization, SIMD, no-aliasing guarantees | **Rust is the "Burst tier":** the same LLVM backend, `&mut` gives the no-alias guarantee, `glam` provides SIMD math |
| No GC on hot paths | Burst code can't allocate managed objects | State lives only in ECS columns; C# systems use `ref` over spans; analyzers flag allocations |

- **Engine and hot gameplay systems written in Rust reach Burst-class code generation by construction.** There is also only one world: no GameObject/Entities split and no baking step.
- **C# systems match Entities + Jobs, but not Burst's code generation.**
  - Measured: 1.74 vs 1.09 ns/entity on a scalar loop (R10). The gap is expected to grow on loops LLVM can vectorize.
  - Day-one mitigation: explicit SIMD (`Vector128`/`Vector256`) and NativeAOT in shipping builds.
- **A Burst-like compiler for a C# subset through LLVM is in the build order (M4).** Whether to build it is decided by spike **S8**: Unity DOTS vs `bevy_ecs` vs C# JIT vs C# NativeAOT on four workloads.

### 4.15 Building the engine with agents (R06, R07)
- **Repo:** a Cargo workspace split along real dependency boundaries. A root `AGENTS.md`/`CLAUDE.md` lists commands and invariants; each crate has an `AGENTS.md` of 150 lines or fewer (purpose, API, how to test, gotchas). Architecture decisions are recorded as ADRs.
- **Agent loop:** `cargo check -p <crate>`, then focused clippy, then focused nextest. Insta snapshots cover schemas, commands and documents.
- **Performance contract:**
  - no `Arc<Mutex>` or `clone()` in per-frame code;
  - owned types and handles;
  - short-lived ECS borrows;
  - criterion/divan benchmarks and Tracy from day one.
- **Local gates (no CI):**
  - `ara test` (unit, golden-image and replay tests);
  - an RHI conformance suite with validation layers on;
  - an arabench subset.
- **Build speed:** rust-lld; Subsecond is optional; the reference clones in `.refs/` are for study (D10).

---

## 5. Conflicts resolved

| Topic | Positions | Resolution | Why |
|---|---|---|---|
| Core language | R06: C++20 · R07, owner: Rust | **Rust** | Owner decision. Consoles are excluded, which removes R06's strongest argument. Metal is needed. Derive-based reflection avoids a C++ header-scanning code generator. Memory safety matters when agents write the code. |
| GPU backend | R02: NVRHI · R07: wgpu + `ash` escape hatch · R08: own Vulkan + Metal 4 | **Own `ara-gpu`** (decided, D12) | NVRHI has no Metal and is C++. wgpu has one queue and no pipeline cache on Metal/DX12, and the Slang → converter → Metal path doesn't fit its bindings. Fidelity and performance come first. |
| Windows graphics API | R02: D3D12 · R08: Vulkan | **Vulkan first**, D3D12 only on evidence | One backend covers three OSes. The native editor removes the Electron-interop reason to prefer D3D12. |
| Scripting | R03, R06, R07: TypeScript/V8 · R10: C# | **C#** (confirmed, D11) | Native UI removes TypeScript's synergy. iOS gets full AOT. Value types map onto ECS columns. Owner preference. |
| Editor shell | R04: Electron + React · R07: Tauri + React · owner: native | **Native, on our own UI system** | Owner decision. Removes the viewport-embedding risk. One UI system everywhere. |
| UI authoring format | R05: restricted TSX | **HTML-like markup + CSS subset + C# bindings** | No TypeScript. LLMs know HTML/CSS well. The markup works with any scripting language. |
| Layout / text | R06: Yoga + HarfBuzz · R05: Taffy + HarfBuzz/ICU · R07: Taffy + Parley | **Taffy + Parley**, HarfBuzz via FFI where coverage is missing | Rust-native; Taffy supports grid. |
| ECS | R06: flecs · R07: bevy_ecs | **bevy_ecs** | Rust |
| Physics | R06: Jolt · R07: Rapier | **Lean Jolt**, decided in spike S6 | AAA-proven and fast; Rapier is the pure-Rust fallback. |
| Platform layer | winit (via Bevy) · R08: SDL3 | **SDL3** | winit and gilrs have mobile gaps. |
| When multiplayer arrives | R07: defer until a first game · R09, owner: day one | **Day-one contracts** | Owner requirement |
| Reference syntax | R01: `{id, path}` · R03: `@path#id` | **`@path#id`** | Compact, readable, self-repairing |
| JSON dialect | R03: JSONC · R01: strict | **Strict JSON** | Universal tooling; documentation belongs in the schema. |
| CI / license scanning | R03, R06, R08: CI, scanning, nightly runs | **None for now** | Owner decision; gates run locally. |
| Proprietary reference code | R06: keep it out of agent context | **Study allowed; write our own** | Owner decision |
| Roadmap framing | R06: "UE parity not realistic in 24 months" | **No cuts; build order only** | Owner decision |
| iOS NativeAOT status | R08: experimental · R10: supported since .NET 9 | **Supported** on iOS; Android experimental | R10 checked the .NET 9+ platform table |
| Export with C# | R08: no compiling at export · R10: NativeAOT per platform | **Per-platform compile of game code**, verified in S7 | Cross-OS NativeAOT is not supported out of the box. |

---

## 6. The decisions hardest to change later (merged across reports)

1. **Object model and identity:** the entity/component/behaviour/system semantics, and runtime vs file vs network IDs.
2. **The Schema DB:** its vocabulary and generation pipeline. Every binding, tool and file format derives from it.
3. **On-disk formats and addressing:** canonical JSON, keyed maps, `@path#id`, ID-path prefab overrides.
4. **The mutation model and Engine Protocol:** command → op → transaction → undo/review.
5. **The simulation contract:** fixed tick, input-driven gameplay, presentation kept separate, scoped determinism, restorable state. Multiplayer, replay and agent testing all sit on it.
6. **The Rust↔C# ABI, and gameplay state living only in ECS columns.**
7. **The RHI binding/sync model and the shader pipeline:** bindless heaps, explicit barriers, multi-queue, Slang → SPIR-V / DXIL → metallib.
8. **Material semantics:** one AST for both the text and the graph view.
9. **The UI runtime contract:** layout and style semantics, the text model, binding/input/focus, and one system for the game and the editor.
10. **The pack format, asset identity and platform floors.**

---

## 7. Spikes first (decision gates)

| # | Spike | Pass criteria | Decides |
|---|---|---|---|
| S1 | GPU: `ara-gpu` foundation, with a bindless material + RT-query pass from one Slang source on Vulkan (Windows) and Metal 4 (MacBook Pro M5, via the converter) | Same scene from the same shader source on both, async compute, PSO caching, validation clean | RHI binding/sync model, shader pipeline |
| S2 | C# host: `netcorehost` + generated ABI + systems over `bevy_ecs` chunks; L3 then L1 reload | L1 p95 ≤100 ms, L3 p95 ≤450 ms on Islam's machine | C# confirmed (or reversed) |
| S3 | Native UI: inspector panel + pause menu + virtualized inventory + world panel; text test list | Smooth at 120 fps, correct bidi/IME/emoji, lossless designer round-trip; compare with RmlUi | UI framework shape |
| S4 | Netcode: Lightyear + Replicon behind our schemas | Character/door/inventory consistent across local, listen and dedicated; late join; packet loss; replay | Netcode stack |
| S5 | Play in a child process | Boot <1 s, live patch <50 ms | Play-mode model |
| S6 | Physics: Jolt binding vs Rapier | Character controller, stacking, vehicles, determinism, performance | Physics stack |
| S7 | Export: C# NativeAOT on every target (iOS static link via the Mac agent, Android bionic), desktop R2R cross-compile from Windows | Signed builds that launch; ara-bridge reachable | Export lanes |
| S8 | DOTS-class benchmark: Unity DOTS (Entities + Burst) vs `bevy_ecs` (Rust) vs C# JIT vs C# NativeAOT on transform integration, boids, SIMD particles, spawn/despawn churn | Numbers recorded on Islam's machine | C# performance-tier plan (§4.14) |

---

## 8. Build order (nothing is cut; phases are dependency order)

0. **Spikes S1–S8.**
1. **Kernel:**
   - Schema DB; document store, ops and undo; Engine Protocol, CLI and MCP;
   - file formats with `ara fmt`/`ara merge`; asset pipeline and cache;
   - `bevy_ecs` world with a fixed tick and local sessions;
   - C# host with L3 then L1 reload;
   - diagnostics bus, capture API, deterministic play and replay;
   - the `ara-gpu` Vulkan backend with a baseline renderer.
2. **Editor v0 on the native UI, and multiplayer foundations:**
   - UI runtime, docking, hierarchy, inspector, viewport and gizmos, assets, console;
   - Agents panel with the review overlay;
   - play in a child process; Local / Host+N / Dedicated+N play;
   - replication and prediction;
   - physics, audio, input actions;
   - **Outpost v0, the dogfood game, built largely by agents.** Outpost is a 1–4 player co-op third-person salvage run: shared doors, private inventories, drones, and dusk-to-night lighting.
3. **Fidelity and authoring:**
   - full render graph; material AST + graph editor; DDGI/RT; upscalers; reference path tracer;
   - UI Designer, stories and semantic tests;
   - animation, navigation, VFX stacks;
   - Metal 4 backend and the macOS editor; Linux.
4. **Scale and reach:**
   - GPU-driven scene, virtualized geometry, virtual shadow maps, texture streaming;
   - world streaming, terrain and foliage, HDR output;
   - the 2D toolset (sprites, tilemaps, 2D physics);
   - full export to all five platforms with ara-bridge on devices; the Mobile tier;
   - the C# performance tier from S8 (the C#-subset LLVM compiler, if justified).
5. **UE-class and beyond:**
   - ReSTIR many-lights, layered materials, skin/hair, volumetrics, PCG, realtime path tracing, frame generation;
   - advanced netcode plugins (rollback, lockstep, shared authority, host migration);
   - visual scripting, a Sequencer-class timeline, multi-user editing.

arabench, generated docs and the local gates run continuously from phase 1.

---

## 9. Questions answered (v2)

| Question | Answer |
|---|---|
| Scripting language | **C# confirmed** (D11), with R10's reversal conditions checked at S2 and S7 |
| First dogfood game | **Outpost** (D16), chosen for the owner: co-op third-person salvage that exercises every pillar |
| Team | **Islam + agents** (D13) |
| Engine license | **Not decided**; private repo (D14) |
| Hardware floors | **Intel Macs dropped for now** (D15); the other floors in §4.11 stand until revisited |
| Mac hardware | **MacBook Pro M5** (Apple10: hardware RT + mesh shading) is the Metal and Mac/iOS machine |

---

## 10. Report index

| # | Report | Model |
|---|---|---|
| R01 | [Core architecture](01-core-architecture.md) | Opus 5.5 |
| R02 | [Rendering & fidelity](02-rendering-fidelity.md) | GPT-6.1 Sol |
| R03 | [AI-native design](03-ai-native-design.md) | Opus 5.5 |
| R04 | [Editor UX & tools](04-editor-ux-tools.md) | GPT-6.1 Sol |
| R05 | [Game UI framework](05-game-ui-framework.md) | GPT-6.1 Sol |
| R06 | [Stack, reuse & strategy](06-stack-reuse-strategy.md) | Opus 5.5 |
| R07 | [Rust path](07-rust-path.md) | GPT-6.1 Sol |
| R08 | [Platforms & export](08-platforms-export.md) | Opus 5.5 |
| R09 | [Multiplayer](09-multiplayer.md) | GPT-6.1 Sol |
| R10 | [C# scripting](10-csharp-scripting.md) | Opus 5.5 |

Reference clones (read-only) live in `.refs/`: Godot, Flax, O3DE and UnityCsReference.
