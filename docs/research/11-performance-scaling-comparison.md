# Performance scaling: Ara, Unity Entities and Unreal Mass

Research checked **2026-11-08**. Ara observations below describe commit
`0260cd275`, before the subsequent scaling work; they are an audit baseline,
not a statement that later commits retain each problem. External documentation
identified itself as Unity **6000.6**, Entities **1.4.8**, and Unreal **5.8**.
Those are documentation versions, not binaries benchmarked here.

Ara already has the core architecture needed for high CPU throughput: table-based
ECS storage, generated batched C# entry points, declared access and parallel jobs.
The largest visible gaps are around that core: codec-backed engine access,
repeated project discovery and document materialization, reload application cost,
and editor query complexity. These are actionable findings from code inspection.
They do **not** establish a performance result against Unity or Unreal.

## What the comparable systems actually do

| Area | Unity | Unreal | Ara baseline |
|---|---|---|---|
| Bulk simulation | Entities groups equal component compositions into archetypes and tightly packed per-component arrays inside 16 KiB chunks. Queries can cache matching archetypes. | MassEntity uses data-only Fragments, archetypes, chunks and stateless Processors with batched EntityQueries. Mass is the relevant comparison for an ECS kernel; Actor-oriented gameplay is a separate ergonomic tier. | Bevy ECS tables plus generated C# chunk calls, parallel across safe access sets. |
| Native execution | Burst compiles a supported C# subset through LLVM; Editor Play uses JIT, Player builds use AOT. It supplements the normal scripting backend. | Mass processors are C++ code; this research found no basis for treating Blueprint or Actors as equivalent to a native Mass kernel. | Rust kernels and CoreCLR C# are distinct lanes; shipping also has NativeAOT. A shared LLVM backend alone does not prove identical vectorization, alias analysis or execution speed. |
| Structural writes | Moving between archetypes is a real cost; batching and the workload's structural-change policy must match in a benchmark. | Mass composition changes are deferred through a command buffer and applied after processing batches. | C# structural writes are buffered, with deterministic playback; systems that reserve entities or write shared resources take the serial path. |

Sources: [Unity archetypes and chunks](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/concepts-archetypes.html),
[Burst compilation model](https://docs.unity.com/en-us/engine/6000.6/manual/programming-environment/script-compilation/burst/introduction-to-burst),
[MassEntity concepts](https://dev.epicgames.com/documentation/unreal-engine/overview-of-mass-entity-in-unreal-engine?lang=en-US),
and Ara's [script systems](../../crates/ara-script/src/ecs/system.rs),
[scheduler construction](../../crates/ara-script/src/host/mod.rs),
[shipping host](../../crates/ara-runtime-script/src/lib.rs).

Unity's `IJobChunk.ScheduleParallel` schedules chunks concurrently and needs
accurate read-only declarations for efficient scheduling. Unity's change filters
are conservative at chunk granularity: declaring write access can mark the chunk
as changed even when values were not modified. Unreal exposes parallel chunk
iteration and per-job command buffers. The implementation lesson is to measure
dispatch thresholds, access declarations, structural playback and false-positive
change work independently; a bare entity-loop benchmark hides those costs.
[Unity job scheduling](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/iterating-data-ijobchunk-implement.html),
[Unity change filters](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/systems-entityquery-filters.html),
[Mass parallel iteration](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/MassEntity/FMassEntityQuery/ParallelForEachEntityChunk),
[Mass parallel command buffers](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/MassEntity/FMassEntityQuery/SetParallelCommandBufferEnabled).

## Compilation and huge edits

Unity's default game assembly grows with unpartitioned game scripts; assembly
definitions and explicit references reduce unnecessary recompilation. The fair
large-project comparison therefore uses both a monolithic project and a realistic
dependency graph, not Ara modules against a deliberately monolithic Unity project.
[Unity assembly organization](https://docs.unity.com/en-us/engine/6000.6/manual/programming-environment/script-compilation/assembly-definition-files/assembly-definitions-intro).

UnrealBuildTool controls `.build.cs` module dependencies. Its documented options
include cached target/include discovery, parallel build actions and unity builds
that combine C++ files. These trade clean-build throughput, incremental work and
memory differently from C# assembly compilation. Compare the complete user action
and phase breakdown; do not label raw C++ and C# compiler milliseconds as identical
work. [UBT](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-build-tool-in-unreal-engine),
[module dependencies](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-modules),
[build configuration](https://dev.epicgames.com/documentation/en-us/unreal-engine/build-configuration-for-unreal-engine).

Ara already retains Roslyn state, emits L1/L2 deltas for supported edits, and
stages L3 assembly generations out of process. A dependency cascade is registered
as one swap, preserving last-good code if staging or registration fails. However,
the baseline project compiler walks module builds sequentially, rereads source
text when validating inputs, and retains only eight generated module states.
The live module path also repeatedly scans inventory. Large changes need tests
for a shared dependency plus all changed dependents in one candidate generation,
including a failed candidate followed by recovery. These are inspection findings,
not measured scaling curves. [Project compiler](../../dotnet/src/Ara.CompileServer/ProjectCompiler.cs),
[retained states](../../dotnet/src/Ara.CompileServer/ProjectCompilations.cs),
[live module path](../../dotnet/src/Ara.CompileServer/ModuleSession.cs),
[cascade coordinator](../../dotnet/src/Ara.CompileServer/Coordinator.cs),
[staged swaps](../../dotnet/src/Ara.Runtime/Hosting/Updates.cs).

An out-of-process compiler does not eliminate publication hitches. Ara's tick
boundary performs registration, schema/layout migration, table replacement and
schedule rebuilding; changed component layouts migrate affected entities one by
one. Report `compile`, `load/pre-JIT`, `register/migrate`, `boundary wait` and
`first correct tick/frame` separately. [Boundary application](../../crates/ara-script/src/host/mod.rs),
[component migration](../../crates/ara-script/src/host/register.rs),
[managed publication](../../dotnet/src/Ara.Runtime/Hosting/ScriptRuntime.cs).

Two comparisons need care:

- Current Unity 6000.6 documentation says entering Play reloads the scene by
  default but leaves domain reload off. Turning reloads off changes the state
  reset contract. An Ara claim must name Unity's actual settings and verify an
  equivalent clean state; it must not assume Unity always reloads its domain.
  [Unity Play configuration](https://docs.unity.com/en-us/engine/6000.6/manual/programming-environment/code-reload-serialization/code-reload/code-reloading-editor/configurable-enter-play-mode).
- Unreal Live Coding patches running C++ binaries. Structural changes use object
  reinstancing, and retained pointers/caches need care. Preloading more modules
  can improve iteration while increasing startup cost. This is a relevant
  architectural comparison, but no Unreal Live Coding benchmark or platform
  support claim was verified on the current Mac.
  [Epic Live Coding](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-live-coding-to-recompile-unreal-engine-applications-at-runtime).

## Data and public interfaces

Ara's generated engine component types do not all provide the same execution
path. Only the baseline built-in `Transform` is bound as a checked native column.
Other engine component accessors fetch JSON bytes, parse callback-local values,
and encode dirty values on flush. Raw system queries explicitly reject these
accessor-backed components. Behaviours run serially with exclusive world access.
The typed public surface is useful, but its presence does not establish a cheap
bulk path. Expand verified native layouts where appropriate, or design a typed
bulk transfer for data that cannot safely be viewed in place. Never infer Rust
layout safety from a schema's field types alone.
[Native binding list](../../crates/ara-script/src/host/bindings.rs),
[accessor implementation](../../dotnet/src/Ara.Runtime/EngineAccess.cs),
[query validation](../../crates/ara-script/src/host/register.rs),
[behaviour execution](../../crates/ara-script/src/ecs/behaviour.rs).

Ara has cooked asset generations, cache digests and background import workers.
Its Play snapshot is CBOR, but contains generic document records and reconstructs
a Workspace before projection. Binary encoding by itself does not remove that
materialization. The baseline `read_header` also parses and clones a full JSON
object, and project indexing/loading rereads documents. Preserve canonical JSON
as the authored source while moving repeated work into validated, versioned
derived data. [Asset generation reuse](../../crates/ara-runtime/src/assets.rs),
[Play snapshot](../../crates/ara-play/src/lib.rs),
[document header](../../crates/ara-doc/src/read.rs),
[project loading](../../crates/ara-host/src/project.rs).

Unity distinguishes source assets, import settings and cached artifacts. Its
6.4 upgrade notes narrow artifact dependency invalidation to changed import
results, while importers that also consume source bytes must declare that source
dependency. This is a useful model for avoiding false invalidation without
silently dropping real inputs. Supported asset categories can use separate import
worker processes; do not assume every Unity import is parallel.
[Artifact dependencies](https://docs.unity.com/en-us/engine/6000.6/manual/upgrade-guides/upgrade-guide-unity64),
[parallel import scope](https://docs.unity.com/en-us/engine/6000.6/manual/assets-and-media/import-assets/parallel-import).

Unreal separates disposable derived data from cooked shipping data. Zen Loader
preprocesses dependency metadata offline into package/container structures;
Zen cooked-output storage also addresses filesystem overhead from very many loose
files. The transferable principle is to prepare the runtime's required form and
dependency closure before launch, not merely choose a faster text parser.
[DDC](https://dev.epicgames.com/documentation/unreal-engine/using-derived-data-cache-in-unreal-engine?lang=en-US),
[runtime loader](https://dev.epicgames.com/documentation/en-us/unreal-engine/zen-loader-in-unreal-engine),
[cooked output storage](https://dev.epicgames.com/documentation/unreal-engine/using-zen-storage-server-as-cooked-output-store-for-unreal-engine).

Ara's C# editor interface currently supplies commands, menu registration, immutable
scene queries, selection and typed undoable mutations. That is not the whole
editor extension surface offered by Unity's custom windows/inspectors or Unreal's
Slate-backed detail customizations. Performance readiness and extension coverage
should be tracked separately. Repeated child/sibling/name queries in the baseline
also rescan snapshots, making full hierarchy traversal a useful scale test.
[Ara editor contract](../dev/csharp-editor-api.md),
[Ara scene queries](../../dotnet/src/Ara.Editor/Scene.cs),
[Unity Editor UI](https://docs.unity.com/en-us/engine/6000.6/manual/uitoolkits/uielements),
[Unreal details customization](https://dev.epicgames.com/documentation/en-us/unreal-engine/details-panel-customizations-in-unreal-engine).

## Work to prioritize

These priorities are inferences from the code paths and documented designs above,
not claims of measured speedups:

1. **Make large edits transactional and proportional to affected work.** Capture
   one source/module inventory per candidate, compile changed modules and their
   required dependency closure, keep source membership separate from ownership
   changes, and preserve atomic last-good publication. Use memory-aware retained
   compilations and bounded independent-node concurrency only after profiling.
2. **Remove JSON from numeric bulk gameplay access.** Cover frequently used native
   components with explicit ABI/layout proofs and test direct C# queries, command
   undo, snapshots and reload together. Variable-sized component access needs a
   different deliberate bulk interface; simply copying raw Rust objects is unsafe.
3. **Avoid repeated document materialization.** Make header indexing cheap, dedupe
   paths efficiently, reuse immutable input work safely, and investigate a cooked
   scene projection/dependency closure for launch while retaining canonical source
   documents for edits and diagnostics.
4. **Index immutable editor queries once.** Preserve deterministic sibling order
   and ambiguity semantics; avoid eagerly storing every full path in deep trees,
   which can trade quadratic query time for quadratic memory.
5. **Measure idle work, structural work and publication hitches.** Keep warmed
   buffers and cached query plans, vary dirty fractions, and measure migration and
   entity creation/destruction inside the timing window. Extend the C# editor
   interface intentionally; do not mistake a faster menu callback for API parity.

## Fair benchmark matrix

The following is a test specification, not measured cross-engine results.
Run Ara CPU cases on the Mac now; do not use the busy Windows machine. Add Unity
and Unreal rows only when matching installed/toolchain-compatible executables are
actually run. Leave unavailable rows explicitly unmeasured.

| Workload | Scales and variants | Required evidence |
|---|---|---|
| Script compilation | 1k/5k/10k files; 1/16/64 modules; chain, fan-out and independent graphs; body edit, shared signature edit, reflected layout edit, add/remove source, no-op, invalid→fixed | Cold and warm p50/p95; files read/parsed; modules emitted/relinked; generator/analyzer/emit time; peak memory; all-or-none state publication |
| Save to active gameplay | Same edit classes with 1k/100k component instances; warmed and newly loaded modules | Detection→compile→load/pre-JIT→migration→first correct tick; tick p95/p99/max during reload; state/checksum correctness; retired-generation memory |
| ECS kernels | 10k/100k/1M entities; one and multiple workers; movement, SIMD particles, defined-neighbour boids, spawn/despawn, sparse updates | Full work including gathers, grid builds and playback; exact field widths/layout, algorithm/order and FP mode; checksums; ns/entity and bytes/allocations |
| Gameplay interface | Direct system columns versus convenience behaviours and numeric/variable engine accessors at equal entity counts | Boundary calls, serialization bytes, allocations/GC, dispatch cost and total tick time; compare the equivalent Unity or Mass lane, not only GameObjects/Actors |
| Data/Play preparation | 1k/10k documents; 10k/100k records; deep/wide prefab trees; cold/warm cache; one changed document | Read bytes, parse/validate/resolve/project time, copies/allocations, time to first correct tick, peak memory and invalidation correctness |
| Editor operations | 10k/100k entities; deep and wide trees; full traversal, repeated id/name/path lookup, duplicates, selection edits | Total callback time and allocation, snapshot construction separately, exact ordering/ambiguity behavior, atomic undo |

Use the same physical machine, entity math, seeds, type widths, worker count,
optimized build intent and correctness checks. Name CoreCLR JIT, NativeAOT, Burst
and native C++/Rust lanes separately. Exclude debug IL and safety instrumentation
only when the matching rows have equivalent settings; report both development
and shipping-oriented modes. Alternate run order, capture machine load, warm up
deliberately, and retain raw distributions. A boids benchmark must include the
same neighbour construction and search order; an engine must not get a free
precomputed grid that another engine rebuilds inside timing.

At the initial audit, the existing
[Mac CPU probe](../../benchmarks/iteration/mac-cpu-2026-11-08.json) measured Ara
phase improvements only. The subsequent executable comparison below adds Unity
CPU kernels; it does not add Unreal, RTX rendering, full shipping-frame or
end-to-end Play startup measurements.

## Verified follow-up: 2026-11-08

The audit above remains the `0260cd275` baseline. Subsequent work adds bounded
independent-module compilation, larger retained project state, transactional bulk
edit publication, checked native physics columns, indexed immutable editor queries,
and less repeated document work. The evidence has distinct scopes:

- A 1,024-script/32-independent-module compilation fixture improved its median
  process p50 from **1,337 to 1,170 ms**; a 5,120-script/64-module fixture measured
  **6.24 s** warm p50 when editing one source in every module. These are generated
  fixtures and full compile requests, not live publication or Unity comparisons.
  A later run changing all 5,120 sources measured **6.626 s p50 / 6.796 s p95**
  with **1.064 GB** peak RSS; authored file writes remain outside its timer.
  [Bounded compilation evidence](../../benchmarks/iteration/compiler_parallel_mac_2026-11-08.json),
  [5,120-script scaling](../../benchmarks/iteration/compiler_5120_mac_2026-11-08.json).
- Checked native access now extends beyond Transform to supported physics
  layouts. The 8,192-access fixture measured **10.22 to 0.060 ms** and zero
  measured native-batch allocation, but uses a pinned C# callback fixture;
  Rust-host correctness is tested separately. This is not a whole-tick speedup.
  [Native access evidence](../../benchmarks/iteration/native_access_mac_2026-11-08.json),
  [checked bindings](../../crates/ara-script/src/host/bindings.rs),
  [real-host semantics](../../crates/ara-script/tests/native_views.rs).
- A real Rust-host/C# query over 1 million native rows now stamps change ticks
  inside parallel chunks. Two paired orders showed **15–19% less cold-query
  tick time** and **7–8% less hot-query time** with 15 pool workers plus caller.
  Small/single-worker results varied around noise; existing pool allocations remain.
  Exact component bits and change-tick semantics were checked. Cold optimized IL
  and hot EnC Debug IL are separate lanes, with no Unity comparison for this probe.
  [Real C# query evidence](../../benchmarks/iteration/query_tick_mac_2026-11-08.json).
- The editor's 5,001-entity hierarchy callback improved from **298 to 6.84 ms**
  including snapshot/IPC work. Document loading now has bounded preparation with
  sequential canonical insertion; separate probes retain exact workspace and
  diagnostic equivalence. Neither establishes editor FPS or complete Play startup.
  [Editor evidence](../../benchmarks/iteration/editor_scene_mac_2026-11-08.json),
  [project loading evidence](../../benchmarks/iteration/data_loading_project_mac_2026-11-08.json),
  [indexing evidence](../../benchmarks/iteration/data_loading_index_mac_2026-11-08.json).

A controlled executable comparison now covers **W1 transform/quaternion** and
**W3 particles**, each with 1 million entities. Ara uses the final Rust-native
`simd` variant, optimized for the local CPU with thin LTO. Unity uses installed
**6000.4.5f1 Editor + Burst 1.8.29**, Entities/Collections **6.4.0** and
Mathematics **1.3.3**. Effective compilation was enabled and safety checks,
force-safety and debug were disabled and captured **before timing** in every
Unity process. This is an Editor versus native CPU-kernel comparison, not a
shipping-player or C# gameplay comparison.
[Reproducible harness](../../benchmarks/iteration/unity/README.md),
[exact versions, hashes and measurements](../../benchmarks/iteration/unity_mac_2026-11-08.json).

Both engines ran at niceness 10 on the same Apple M5 Pro, on battery power
(40% to 36%). Each case used a fresh process in each of two rounds, reversing
engine and case order. W1 used 120 warmup steps, W3 used 3,000, then each measured
300 steps. The parallel budget was 14 workers plus the participating caller:
Ara argument `14`, Unity request `15` with actual job-worker count `14`. Every
one of eight case pairs matched all **48 MB of final state bit for bit** using
BLAKE3. Setup, imports, output and state hashing were outside kernel timing.
[Per-case provenance and correctness](../../benchmarks/iteration/unity_mac_2026-11-08.json).

| Kernel / maximum active runners | Ara p50 ms | Unity p50 ms | Ara p95 ms | Unity p95 ms |
|---|---:|---:|---:|---:|
| W1 / 1 | 3.816 | 4.391 | 3.928 | 4.881 |
| W1 / 15 | 0.436 | 0.470 | 0.551 | 0.546 |
| W3 / 1 | 2.879 | 4.525 | 2.940 | 4.750 |
| W3 / 15 | 0.435 | 0.400 | 0.469 | 0.557 |

Entries are medians of two process p50/p95 values, not pooled percentiles or
confidence intervals. Ara led serial W1 by about **1.15×**, parallel W1 by
**1.08×**, and serial W3 by **1.57×** in these samples. Unity led parallel W3
p50 by **1.09×**; its p95 was higher. Each p50 direction held in both rounds.
Ordinary desktop background load remained. A short formatting-related compiler
probe triggered a post-process guard; the affected pair was discarded and rerun.
Every retained engine process passed the before/after overlap guards. This does
not establish continuous absence of interference or general engine superiority.
[Individual rounds and load snapshots](../../benchmarks/iteration/unity_mac_2026-11-08.json).

Inspection of the resolved Burst package found that option changes can write
EditorPrefs/SessionState. The original wrapper did not restore its initial
compilation/safety options or capture force-safety/debug, so its earlier
[exploratory matrix](../../benchmarks/iteration/unity_mac_2026-11-08_exploratory.json)
is retained separately. The controlled rerun used the current wrapper, which
restores all changed option values and all five known preference-key
values/presence in `finally` before exit. Every measured process reported checked
restoration success. No unknown historical preference value was guessed or
restored. Numerical kernels and their timing window are unchanged.
[Wrapper and isolation limits](../../benchmarks/iteration/unity/README.md).

The next comparisons should target the remaining measured gaps: W3 parallel
dispatch/memory throughput, broader C# native-column gameplay, structural writes,
sparse changes, and edit-to-first-correct-tick publication. Extend the data tests
to dependency cooking and Play projection, not only JSON parsing. Match Unity's
assembly graph and reload settings before comparing iteration latency. Unreal
Mass, Windows/RTX, shipping AOT/IL2CPP, rendering and whole-editor/startup timings
remain **unmeasured**; no engine installation or Windows work was performed.
