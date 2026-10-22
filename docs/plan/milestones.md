# Milestones

Milestones are **build order, not scope**: nothing in the research gets dropped, only sequenced. Each milestone ends with exit criteria an agent can check locally (`cargo test`, `ara test`, captures, replays). There is no CI for now (D9). Tasks are tracked in Snoze (Ara › Tasks).

**Dogfood game: Outpost.** A 1–4 player co-op, third-person salvage run through a derelict outpost at dusk. Players open shared doors, loot items into their own inventories, fend off drones and extract before night falls. It grows alongside the engine and exercises every pillar:
- multiplayer with late join;
- native UI (HUD, inventory, world-space labels);
- physics and animation;
- dynamic lighting from dusk to night;
- export to all five platforms.

Agents build most of it, using the engine's own tools.

---

## M0: Foundations and spikes

**Goal:** prove the risky foundations before building on them. Each spike lives in `spikes/<id>-<name>/` (outside the workspace, throwaway) and ends with `RESULT.md` plus an entry in [`decisions.md`](../decisions.md).

| # | Spike | Pass criteria | Decides |
|---|---|---|---|
| S1 | **ara-gpu foundation.** Vulkan 1.3 on Windows and Metal 4 on the MacBook Pro M5. A bindless material plus a ray-query pass from one Slang source (SPIR-V; DXIL → Metal Shader Converter). An async compute queue and a persistent pipeline cache. | The same scene renders on both backends from the same shader source, with validation layers clean | RHI binding/sync model, shader pipeline |
| S2 | **C# host.** `netcorehost` plus a generated ABI; C# systems over `bevy_ecs` chunks; L3 module swap, then L1 body-edit reload | L1 p95 ≤100 ms, L3 p95 ≤450 ms on Islam's machine; chunk loop within 2× of Rust | Keeps or reverses D11 |
| S3 | **Native UI.** Inspector panel, pause menu, virtualized inventory and world panel; markup + stylesheet → Taffy + Parley → engine display lists | 120 fps; correct bidi, IME and emoji; a designer round-trip that leaves the file byte-identical; compared against RmlUi | UI runtime shape, text stack |
| S4 | **Netcode.** Lightyear + Replicon behind our schemas: a character, a door and an inventory | Consistent in local, listen and dedicated modes; late join, packet loss and replay all work | Netcode stack |
| S5 | **Play in a child process.** | Boot <1 s, live patch <50 ms | Play-mode model |
| S6 | **Physics.** A Jolt binding vs Rapier | Character controller, stacking, vehicles, determinism and performance compared | Physics stack |
| S7 | **Export and AOT.** C# NativeAOT per target: Windows locally, Linux via WSL, macOS and iOS on the MacBook, Android bionic; ReadyToRun cross-compile from Windows | Signed builds launch on every target | Export lanes |
| S8 | **DOTS-class benchmark.** Unity DOTS (Entities + Burst) vs `bevy_ecs` (Rust) vs C# JIT vs C# NativeAOT on four workloads: transform integration, boids neighbour search, SIMD particles, spawn/despawn churn | Numbers recorded on Islam's machine | C# performance-tier plan (A12) |

Also in M0: the repo, workspace and `AGENTS.md` (done at init).

**Mac sessions are paused (2026-10-23).** The Apple halves are sequenced later, not dropped:
- the S1 Metal 4 half (checklist in `spikes/s1-gpu/RESULT.md`) opens M3's Metal 4 backend work;
- the S7 macOS/iOS export half is part of M4's export pipeline.

M0 exits on the Windows/Linux/Android results.

**Exit:** S1–S8 have written results and decisions; `decisions.md` is updated; M1 tasks are refined from the spike findings.

---

## M1: Kernel, a headless engine an agent can drive

- **Schema DB:** Rust derive registration; metadata (units, ranges, docs, replication); JSON Schema and C# generation; `reflection.json`.
- **Documents:**
  - canonical JSON scene and prefab formats;
  - `@path#id` references;
  - ID-addressed prefab overrides;
  - `ara fmt` and `ara merge`.
- **Mutation:**
  - command, query and job registries;
  - ops with inverses;
  - transactions carrying author and `why`;
  - undo and redo with revision checks.
- **Engine Protocol:** JSON-RPC over WebSocket/stdio with a token; the `ara` CLI; an MCP adapter with about 12 tools and schema search.
- **Simulation:**
  - `bevy_ecs` world with a fixed tick and Local sessions;
  - seeded RNG;
  - input injected as actions;
  - `step`, `watch` and `until`;
  - record and replay.
- **C# host:**
  - the generated ABI;
  - Systems over chunks, and Behaviours;
  - L3 then L1 reload;
  - the first analyzer set (ARA0001–ARA0400);
  - the `ara_run` worker.
- **Assets:** glTF and PNG importers as pure functions in worker processes; a content-addressed cache; hot reload.
- **Renderer baseline on ara-gpu Vulkan:**
  - render graph v0;
  - PBR, shadows, IBL, TAA, tonemapping;
  - headless capture with entity marks and a legend;
  - debug view modes and golden-image comparison.
- **Diagnostics:** a bus with stable codes and fixes; `ara check` and `ara why`.
- **Agent tooling:** per-crate `AGENTS.md`; arabench v0 (10–20 tasks).

**Exit:** using only MCP, an agent:
- builds a lit scene with a C# behaviour from text files;
- steps it deterministically;
- captures labelled screenshots;
- passes a tick-level assertion test;
- undoes its own changes.

`ara test` is green.

---

## M2: Editor v0, multiplayer and the Outpost gray-box

- **Native UI runtime:**
  - Taffy and Parley;
  - display lists;
  - focus and input in the style of CommonUI;
  - markup and stylesheets;
  - typed C# bindings.
- **Editor:**
  - docking;
  - scene and game viewports with gizmos;
  - hierarchy;
  - an inspector generated from the schema, with value provenance;
  - assets panel;
  - console/problems;
  - Agents panel (review overlay, accept/revert);
  - command palette.
- **Play mode:** runs in a child process, with a "Keep changes" flow.
- **Multiplayer:**
  - Local, Listen and Dedicated sessions;
  - replication policies;
  - owner prediction for movement;
  - network-condition simulation;
  - a bandwidth profiler;
  - divergence replay;
  - headless bots;
  - Host + N and Dedicated + N play from the editor.
- **Gameplay basics:** physics (the S6 choice); a character controller; audio (Kira + SDL3); input actions; basic animation playback.
- **Outpost v0 (gray-box):**
  - 1–4 players;
  - movement;
  - shared doors;
  - pickups into private inventories;
  - HUD and inventory screen;
  - late join.

  Agents build most of it.

**Exit:** Outpost v0 plays co-op in listen and dedicated modes on Windows; the arabench baseline is recorded; the editor is used daily.

---

## M3: Fidelity, authoring and macOS

- **Rendering:**
  - full render graph (async compute, aliasing);
  - material AST + text DSL + graph editor;
  - baked GI and probes;
  - hardware-RT DDGI, with RT reflections and shadows;
  - denoising;
  - DLSS, FSR, XeSS and MetalFX;
  - post stack;
  - reference path tracer;
  - a suite of golden scenes.
- **Platforms:** the Metal 4 backend completed, giving the editor and runtime on macOS (MacBook Pro M5); the editor and runtime on Linux.
- **UI authoring:** UI Designer with lossless round-trip; component stories; semantic UI tests; world-space UI.
- **Gameplay systems:**
  - animation (state machines, blend trees, root motion);
  - navigation (Recast);
  - GPU VFX module stacks;
  - one graph framework shared by materials, VFX and animation.
- **Outpost v1:**
  - art pass;
  - dynamic lighting from dusk to night;
  - drone enemies on a navmesh;
  - VFX and an audio pass.

**Exit:** Outpost v1 runs on Windows, Linux and macOS on the RT tier; golden scenes pass; the UI designer round-trips byte-exact.

---

## M4: Scale and reach

- **Rendering at scale:**
  - GPU-driven scene with culling, indirect draws and HZB;
  - virtualized geometry (meshlets, cluster LOD, streaming);
  - virtual shadow maps;
  - texture streaming;
  - HDR output.
- **World:** world streaming; terrain and foliage.
- **Export:**
  - `ara doctor`, `export`, `run` and `device`;
  - ara-bridge on devices;
  - packs for Windows, Linux, macOS, Android and iOS (iOS via the MacBook);
  - the Mobile tier.
- **2D toolset:** sprites, tilemaps, 2D physics and a 2D render path.
- **C# performance tier:** implement S8's decision, including the C#-subset LLVM compiler if the numbers justify it.

**Exit:** an agent exports Outpost to all five platforms and verifies it on each through ara-bridge; a large stress scene meets its frame budgets.

---

## M5: UE-class and beyond

- **Rendering:**
  - ReSTIR many-lights;
  - layered materials;
  - skin and hair;
  - volumetrics and atmosphere;
  - PCG;
  - realtime path tracing;
  - frame generation.
- **Netcode plugins:** rollback, lockstep, shared authority and host migration.
- **Authoring:**
  - visual scripting;
  - a Sequencer-class timeline;
  - multi-user editing;
  - generation jobs with provenance.

**Exit:** reference shots and motion sequences match UE-class quality on declared hardware; Outpost v2 shows it off.
