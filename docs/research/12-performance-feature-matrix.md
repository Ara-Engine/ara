# Performance coverage beyond transforms

Research checked **2026-11-08**. Code observations use Ara **`20e8ccdbc`**, before
the current follow-up work. Concurrent implementation changes must update their
own evidence; an item below is not automatically a remaining defect in a later
commit. This inventory adds feature and correctness coverage to
[the scaling investigation](11-performance-scaling-comparison.md). It does not
contain new timing runs; subsequent measurements are linked below.

The present evidence does **not** establish that Ara beats Unity across ECS,
Jobs, Burst, editor iteration or complete games. The controlled Mac comparison
covers two kernels, transforms/quaternions and particles, each single-threaded
and parallel. Ara leads three of those four p50 cases; Unity leads parallel
particles. That is useful evidence about those exact workloads, not four
independent feature families. No executable Unreal comparison exists here.

The useful target is a growing set of representative, reproducible workloads
with a stated margin, equal state/quality gates and visible losses. A universal
win across arbitrary programs, hardware and third-party packages cannot be
demonstrated by a finite suite. The locked direction remains C# gameplay,
explicit SIMD/native hot paths and the existing reload architecture: D11,
A12/A13 and S8 in [the decisions](../decisions.md).

## Follow-up evidence, November 9

The baseline inventory below remains a snapshot of `20e8ccdbc`. These subsequent
changes have executable evidence:

- Native SparseSet views now pass actual CLR mixed-storage, filter, tick, churn,
  command-order and undo tests. The [sparse results](../../benchmarks/iteration/sparse_native_mac_2026-11-09.json)
  retain fragmented-query costs and a cold dense-tail increase; they do not establish
  zero regression. The subsequent [streaming fallback](../../benchmarks/iteration/sparse_streaming_mac_2026-11-09.json)
  reduces measured sparse p50 by 52–71%, retaining exact callbacks/ticks and the
  dense holdout results, including tail variability.
- C# `[Query]` methods can receive component spans and readonly entity spans while
  retaining the same access declarations and ABI. The [batch results](../../benchmarks/iteration/batch_queries_mac_2026-11-09.json)
  cover 24 valid fresh-process cases, exact row bits, scalar and explicit SIMD
  variants, Release and hot Debug. Scalar spans alone give small or mixed changes;
  the explicit SIMD kernel benefits. NativeAOT entry generation compiles, while
  NativeAOT execution remains unmeasured.
- [Owned scene transfer](../../benchmarks/iteration/scene_prepare_mac_2026-11-09.json)
  removes decoded JSON-tree copies. Subsequent [validation/materialization work](../../benchmarks/iteration/pipeline_allocations_mac_2026-11-09.json)
  reduces complete CPU preparation by about 19% at 100k records and allocation
  requests by 26%, with matching complete state and portable hashes. Asset, GPU
  and first-correct-tick readiness are outside these timings.
- A [10,240-script full-edit probe](../../benchmarks/iteration/compiler_10240_mac_2026-11-09.json)
  completes full project requests at 13.9–14.5 seconds warm, with 1.93 GB peak
  compile-server RSS. This is one process with generated independent modules and
  three warm samples, on a newer CLR than the older 5,120-script fixture. It does
  not measure a commercial project, live reload or a matched Unity graph.
- [Local scene Set batches](../../benchmarks/iteration/projection_local_mac_2026-11-09.json)
  resolve touched source records with guarded full fallbacks. At 100k plain rows,
  one edit drops from 215–226 ms to 0.84–0.90 ms with matching source/state/snapshot
  hashes. A large prefab instance still resolves in full; broad edits do not gain
  the same benefit. The initial full-wave slowdown and follow-up overhead remain
  visible in the evidence. This measures CPU op application, not editor latency.
- The [expanded boids/structural baseline](../../benchmarks/iteration/unity/broad_mac_2026-11-08.json)
  is one battery round, with its missing-ID and bookkeeping limits recorded.
  A later build replaced the original binary; reconstructed-source runs require a
  separate profile. The [new feature fixtures](../../benchmarks/iteration/unity/features/README.md)
  use full ID/tag/update exports and distinguish correctness smokes from timings.
  The [extended suite](../../benchmarks/iteration/unity/features/extended_smoke_mac_2026-11-09.json)
  passes 50 serial/parallel pairs for real changed filters, structural command
  playback, dependency chains/forks and fixed/resizing buffers. Every full state
  matches the independent integer oracle. Runtime kernels/executable stayed fixed;
  controller phases and failed attempts are preserved. These are native API
  correctness results, with no accepted timings or new Ara C# buffer/job API claim.

These additions do not fill every API or comparison gap below. C# buffer,
enable/shared/change-filter and explicit job-graph surfaces, subsystem comparisons
and a measured Unreal lane still need their own evidence.

## Versions and evidence boundaries

The installed fixture is Unity **6000.4.5f1**, Burst **1.8.29**, Mathematics
**1.3.3**, and built-in Entities/Collections **6.4.0**, according to
`target/perf-scale/unity-s8/Packages/packages-lock.json`. Its installed official
package documentation was checked for the feature mechanisms below. The public
links currently identify Entities **1.4.8**, Collections **2.6.8** and Burst
**1.8.31**. These are documentation versions, not the benchmark binaries, and
must not replace the fixture manifest in a result. Public Epic pages identify
Unreal **5.8**; no Unreal installation or executable result was verified.
[Entities docs](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/index.html),
[Collections docs](https://docs.unity3d.com/Packages/com.unity.collections@2.6/manual/collection-types.html),
[Burst docs](https://docs.unity3d.com/Packages/com.unity.burst@1.8/manual/index.html),
[Epic Mass documentation](https://dev.epicgames.com/documentation/en-us/unreal-engine/mass-entity-in-unreal-engine).

| Evidence already checked in | What it establishes | What it does not establish |
|---|---|---|
| [Controlled Unity Mac JSON](../../benchmarks/iteration/unity_mac_2026-11-08.json), [fixture](../../benchmarks/iteration/unity/README.md) | Two reversed rounds of W1 and W3 at 1M entities; matched numeric-state hashes for eight cases; declared worker settings. | C# Ara gameplay performance, structural changes, neighbourhood search, fragmented queries, other subsystems or complete-frame superiority. Keep exploratory runs separate. |
| [Actual C# query tick](../../benchmarks/iteration/query_tick_uninstrumented_mac_2026-11-08.json) | Ara before/after with real query dispatch, 1M rows and a simpler kernel. | A direct comparison to W1: math, instrumentation and worker settings differ. The [native access fixture](../../benchmarks/iteration/native_access_mac_2026-11-08.json) uses pinned mock storage, not the complete ECS tick. |
| [Codec writes](../../benchmarks/iteration/codec_writes_mac_2026-11-08.json) | Real C# read/modify/flush of codec-backed engine data at 2,048 entities. | General variable-buffer throughput, audio DSP, or Unity/Unreal parity. |
| [5,120-file compiler fixture](../../benchmarks/iteration/compiler_5120_mac_2026-11-08.json), [parallel compiler fixture](../../benchmarks/iteration/compiler_parallel_mac_2026-11-08.json) | Ara generated-project compilation scaling and before/after effects. | Complete save-to-first-correct-tick latency or a matched Unity assembly graph. |
| [Project loading](../../benchmarks/iteration/data_loading_project_mac_2026-11-08.json), [indexing](../../benchmarks/iteration/data_loading_index_mac_2026-11-08.json), [editor scene](../../benchmarks/iteration/editor_scene_mac_2026-11-08.json) | Ara phase-specific improvements and 5,001-entity editor callback/snapshot costs. | Cooked scene/asset readiness, editor interaction FPS or a cross-engine startup result. |
| [Historical S8](../../spikes/s8-dots-bench/RESULT.md) and [locked M1 results](../decisions.md) | Earlier Windows boids, churn and math results with stated workloads. | Current Mac results, complete modern API coverage or permission to run the busy Windows machine. |

All new CPU experiments in this pass belong on the Mac. The Windows machine
remains untouched. Rows below marked **missing** have no matched current
cross-engine result, even when Ara has unit tests or an internal benchmark.

## Storage, queries and data access

Rust capability, engine integration and C# exposure are separate questions.
`ara-sim` publicly reexports Bevy ECS, but that alone does not prove a facility
participates in Ara reflection, snapshots, rollback, replication, reload or the
C# API. [Simulation public surface](../../crates/ara-sim/src/lib.rs),
[component registration](../../crates/ara-sim/src/registry.rs),
[runtime script integration](../../crates/ara-runtime-script/src/lib.rs).

| Feature / Unity mechanism | Ara baseline and concrete gap | Required matched workload and state gate |
|---|---|---|
| **Archetypes, chunks and cached matching.** Unity stores equal compositions together; query plans match archetypes. [Archetypes](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/concepts-archetypes.html) | Bevy tables and retained `QueryState<FilteredEntityMut>`; generated C# performs one foreign call per batch and loops rows within C#. Strong dense foundation. C# is not called across Rust once per entity. [Query dispatch](../../crates/ara-script/src/ecs/system.rs), [generator](../../dotnet/src/Ara.Generators/Emit.cs) | **Partially measured: W1/W3 dense kernels only.** Sweep 1/16/256/4,096 archetypes, component widths, chunk occupancy and empty queries at fixed entity/work counts. Include matching refresh after structural changes; exact visited IDs and full state. |
| **With/Without, optional and sparse data.** Presence filters change membership independently of arithmetic. [Queries](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/systems-entityquery-filters.html) | Public C# `[With<T>]`/`[Without<T>]` exist; extraction accepts declared component columns. No general optional-column C# query contract found. Baseline native binding resolution explicitly rejects SparseSet storage (ARA0934); the dispatcher assumes contiguous table columns. Supporting sparse-native columns and filters requires membership/traversal work, not just sparse tags whose archetypes share one dense table. [Attributes](../../dotnet/src/Ara.Runtime/Attributes.cs), [extraction](../../dotnet/src/Ara.Generators/Extract.cs), [dispatcher](../../crates/ara-script/src/ecs/system.rs) | **Missing.** Vary 0/1/10/50/100% matches, shared tables, sparse storage, archetype mutation and no-result queries. Compare exact entity sets and changed ticks; ensure no duplicate, omitted or incorrectly written row before timing. |
| **Changed filters.** Unity filters conservatively by chunk versions; write access can flag a chunk without an actual value change. [Filter semantics](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/systems-entityquery-filters.html) | Bevy change ticks exist and C# native writes must stamp them correctly. No public generated C# changed-filter attribute found. A general dirty boolean does not supply equivalent membership/history semantics. Added/Changed use in authoritative simulation must respect Ara restore rules. [Dispatcher](../../crates/ara-script/src/ecs/system.rs), [session contract](../../crates/ara-sim/src/session.rs) | **Missing.** Producer/consumer chain at 0/1/10/100% dirty, no-op write declarations, multiple ticks, wrap/restore and sparse data. Match the chosen observable semantics, record skipped work and false positives; never compare skipped updates to a full update as equal work. |
| **Enableable components.** Unity per-component enable bits affect queries without archetype migration. [Enableable introduction](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/components-enableable-intro.html) | No first-class equivalent C# enable-bit/query-mask surface found. Tags can express a gameplay state but adding/removing them has structural and change-observation implications. An `Enabled` field still scans unless the query supports masking. [Public query API](../../dotnet/src/Ara.Runtime/Attributes.cs) | **Missing feature lane.** Toggle masks at different densities and clustering; time toggles plus masked iteration. Check query membership, re-enable state preservation, scheduler dependencies and snapshot/replay behavior. |
| **Shared component values.** Unity groups entities with equal shared values; changing values moves entities. [Shared components](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/components-shared-introducing.html) | A C# Resource is one shared world value, not Unity's value-grouped component. No integrated shared-value grouping/filtering C# API found. [System registration](../../crates/ara-script/src/host/register.rs), [attributes](../../dotnet/src/Ara.Runtime/Attributes.cs) | **Missing feature lane.** Material/team/region groups with 1/16/1,024 distinct values, skew, reassignment and filtered iteration. Include regrouping, memory fragmentation and exact entity/value groups. |
| **Dynamic buffers.** Unity exposes per-entity unmanaged element arrays, with inline capacity and external spill. [Buffers](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/components-buffer.html) | Rust dynamic components have owned variable/nested representations. Module schema generator accepts blittable fields and rejects managed reference fields; codec-backed engine fields use copy/flush. Neither a Rust `Vec` nor a JSON list establishes cheap, safe C# `DynamicBuffer<T>` parity. [Dynamic layouts](../../crates/ara-sim/src/dynamic.rs), [schema generator](../../dotnet/src/Ara.Generators/Schema.cs), [engine access](../../dotnet/src/Ara.Runtime/EngineAccess.cs) | **Missing feature lane.** Inventory/path/contact buffers at 0/4/16/64/1,024 elements, growth/shrink/spill and parallel read/write. Include allocations, copies, serialization and playback; validate all elements, lifetimes, snapshots and reload ownership. |
| **Native containers and gather/scatter.** Unity Collections supplies native arrays/lists/maps/queues/streams and parallel variants. [Container catalogue](https://docs.unity3d.com/Packages/com.unity.collections@2.6/manual/collection-types.html) | C# gameplay allows spans, numerics, collections, `Unsafe`, `MemoryMarshal` and intrinsics. That does not automatically provide a job-aware native container library with allocation lifetimes and dependency safety. ECS state must still live in components. [API profiles](../../dotnet/src/Ara.Analyzers/ApiProfiles.cs) | **Missing.** Spatial hash construction, prefix sum, compaction, append queues and reductions, including clearing/disposal and output ordering. Match allocation policy and deterministic reduction/tree semantics; measure bytes and GC as well as time. |
| **Random access / lookup.** Unity `ComponentLookup` accesses another entity outside current iteration. [Lookups](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/systems-looking-up-data.html) | `World.Get/Read/Set` and `Query<T>(Span<Entity>)` exist. The latter returns IDs, not a span of components. Checked native layouts are fast candidates; other engine data takes codec paths. Baseline checked numeric native views cover Transform plus PhysicsId, PhysicsPose, Velocity, CharacterController and VehicleTelemetry. [World API](../../dotnet/src/Ara.Runtime/World.cs), [layout proofs](../../crates/ara-script/src/host/bindings.rs) | **Missing cross-engine comparison.** Parent/target lookups, random IDs, deleted handles, contiguous versus shuffled access and codec/native variants. Include index resolution and read/modify/flush; check stale-handle rejection and write visibility. |

## Jobs, generated code and structural work

| Feature / Unity mechanism | Ara baseline and concrete gap | Required matched workload and state gate |
|---|---|---|
| **Declared dependencies and job graphs.** Unity propagates read/write dependencies through job handles. [Dependencies](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/scheduling-jobs-dependencies.html) | Generated component/resource access feeds the script scheduler; independent systems can run in parallel, ordering uses `(Order, name)`. Structural systems and mutable-resource systems take a serial path. Rust session parallel systems reject access conflicts. The runtime integrates scripts through one exclusive gameplay slot, which is not arbitrary per-system Rust/C# overlap. [Script scheduling](../../crates/ara-script/src/host/mod.rs), [session scheduling](../../crates/ara-sim/src/session.rs), [integration](../../crates/ara-runtime-script/src/lib.rs) | **Missing.** Empty/tiny jobs, 1/8/64 systems, chains, forks/joins, two readers/one writer and cross-subsystem dependencies. Include scheduling and completion fences. Assert producer-before-consumer data and absence of races, plus deterministic output over worker counts. |
| **Batching and work distribution.** `IJobChunk` processes chunks in parallel with explicit access declarations. [Chunk jobs](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/iterating-data-ijobchunk-implement.html) | Script dispatcher has a 4,096-row parallel threshold, minimum 1,024-row chunks and a batch target based on pool threads. Reused scratch metadata does not prove zero tick allocation: task queues and scheduling remain costs. C# `[Query]` exposes per-row methods inside generated batch loops, not a general user-defined chunk-span API. [Dispatcher](../../crates/ara-script/src/ecs/system.rs), [extractor](../../dotnet/src/Ara.Generators/Extract.cs) | **Partial: dense parallel math.** Sweep 0–1M rows, fragmented tables, short/long/uneven kernels and caller/worker counts. Capture dispatch, stalls, allocations and ns/row. Tune thresholds using crossover curves, not one large case. |
| **SIMD, aliasing and code generation.** Burst compiles a supported C# subset; Editor JIT and Player AOT are different paths. [Burst](https://docs.unity3d.com/Packages/com.unity.burst@1.8/manual/index.html) | Rust/LLVM, optimized CoreCLR, reloadable debug IL and NativeAOT are distinct lanes. Explicit C# SIMD is permitted. A12's future subset compiler is conditional on evidence; sharing LLVM does not prove equal vectorization or code. [API profiles](../../dotnet/src/Ara.Analyzers/ApiProfiles.cs), [development/shipping host](../../crates/ara-runtime-script/src/lib.rs), [decisions](../decisions.md) | **Native W1/W3 measured; matched real C# and shipping AOT missing.** Use identical loop bodies/widths, scalar/SIMD, alignment and tail sizes. Inspect emitted code where a loss remains; include real ABI entry and scheduler. Publish each execution lane separately. |
| **Development compilation, safety and FP options.** Burst supports asynchronous Editor compilation and explicit safety/float settings. [Compilation](https://docs.unity3d.com/Packages/com.unity.burst@1.8/manual/compilation-synchronous.html), [options](https://docs.unity3d.com/Packages/com.unity.burst@1.8/manual/compilation-burstcompile.html) | Ara has analyzer profiles, layout proofs and checked host entry points; C# unsafe tools are allowed in gameplay. These are not the same contract as Unity native-container safety instrumentation. Preserve deterministic math and error containment rather than disabling checks silently. [Analyzer](../../dotnet/src/Ara.Analyzers/ApiProfiles.cs), [bindings](../../crates/ara-script/src/host/bindings.rs) | **Missing matched mode matrix.** Development checks on/off and shipping; warmed compiled kernels versus first invocation; publish FP/safety settings. Include invalid-access rejection separately from valid-path timing. Relaxed FP is a separate quality lane and must not invalidate exact-state claims. |
| **Structural command buffering.** Unity ECB records changes for later playback and supports temporary entity remapping. [ECB](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/systems-entity-command-buffers.html) | C# Commands exposes Spawn/Despawn/Add/Remove/SetParent with buffered application; reservation/structural execution constrains parallelism. Native Rust churn is not proof of C# command recording/playback throughput. [Commands](../../dotnet/src/Ara.Runtime/Commands.cs), [dispatch](../../crates/ara-script/src/ecs/system.rs) | **Historical native churn only; current real C# lane missing.** Empty/one/bulk commands, 1–100% churn, multi-component spawn, add/remove, same-entity conflicting commands and mixed reads. Include recording, allocation, remapping, playback and query refresh. Check final IDs/components, deterministic ordering, stale handles and failure atomicity. |
| **Neighbourhood workloads and reductions.** Jobs/containers support multi-stage algorithms, not just independent rows. [Containers](https://docs.unity3d.com/Packages/com.unity.collections@2.6/manual/collection-types.html) | Existing Rust boids work supplies historical evidence; broad C# query API coverage does not establish a fast spatial build/gather/reduction pipeline. Resource-write serialization may matter for accumulation. [S8](../../spikes/s8-dots-bench/RESULT.md), [scheduler](../../crates/ara-script/src/host/mod.rs) | **Historical W2 only; current matched pipeline missing.** Fixed-seed boids/crowd with identical neighbour grid, radius, traversal order and all build/query/integration stages timed. Also deterministic sums/histograms. Do not replace the algorithm or omit grid construction to win. |

## Hierarchies, data, compilation and authoring

| Feature / reference mechanism | Ara baseline and concrete gap | Required matched workload and state gate |
|---|---|---|
| **Transform hierarchies.** Unity's LocalTransform/LocalToWorld hierarchy work exceeds independent quaternion updates. [Transform concepts](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/transforms-concepts.html) | Parent/child APIs and hierarchy projection exist. W1 does not measure parent propagation, reparenting or authored overrides. [World hierarchy](../../dotnet/src/Ara.Runtime/World.cs), [simulation projection](../../crates/ara-sim/src/projection.rs) | **Missing.** Deep/wide/mixed trees, unchanged branches, 1–100% dirty parents and reparenting. Include dependency traversal; check every world matrix, child order and cycles/error behavior under identical transform semantics. |
| **Prefab baking and scene streaming.** Unity separates authoring/baking from entity-scene loading. [Baking](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/baking.html), [loading](https://docs.unity3d.com/Packages/com.unity.entities@1.4/manual/streaming-loading-scenes.html) | Ara has canonical authored docs, prefabs, asset generations and cooked world cells. Baseline Play CBOR still contains generic records and reconstructs a Workspace; binary transport alone does not remove clone/resolve/project cost. World-cell streaming already has bounded preparation/activation and generations. [Play snapshot](../../crates/ara-play/src/lib.rs), [world contract](../../crates/ara-world/AGENTS.md) | **Ara phases measured; matched full lane missing.** 1k/10k docs, deep prefab overrides, cold/warm/change-one inputs, 10k/100k/1M entities, stream-in/out. Time read→validate→resolve→project→first correct tick and asset-ready frame; exact references, overrides and stale-generation rejection. |
| **Huge script edits and dependency closure.** Unity assembly definitions partition recompilation. [Assembly organization](https://docs.unity.com/en-us/engine/6000.6/manual/programming-environment/script-compilation/assembly-definition-files/assembly-definitions-intro) | Ara retains Roslyn states, compiles module dependency graphs out of process, supports delta and collectible-generation swaps, and preserves last-good candidates. Recent fixtures cover large generated graphs; neither compiler time alone nor an invalid candidate proves live project readiness. [Project compiler](../../dotnet/src/Ara.CompileServer/ProjectCompiler.cs), [coordinator](../../dotnet/src/Ara.CompileServer/Coordinator.cs), [updates](../../dotnet/src/Ara.Runtime/Hosting/Updates.cs) | **Ara compiler measured; cross-engine and full-flow missing.** 1k/5k/10k files; realistic monolith and modular chain/fan-out/independent graphs; body/signature/schema edits, file movement/deletion, no-op and invalid→fixed. Match analyzers/generators and output intent; measure detection, parsing, generation, emit, load/JIT, migration and first correct tick. |
| **Reload/state migration and Play reset.** Unity Play settings change reload/reset behavior; Unreal Live Coding has reinstancing requirements. [Unity configuration](https://docs.unity.com/en-us/engine/6000.6/manual/programming-environment/code-reload-serialization/code-reload/code-reloading-editor/configurable-enter-play-mode), [Epic Live Coding](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-live-coding-to-recompile-unreal-engine-applications) | Ara swaps at boundaries and migrates changed layouts; off-process Roslyn does not eliminate registration/migration hitches. Preserve D11/A13, last-good code and authoritative ECS state. [Boundary application](../../crates/ara-script/src/host/mod.rs), [migration](../../crates/ara-script/src/host/register.rs) | **Historical S2 gates; current large live-edit lane missing.** Body/layout edits over 1k/100k instances and many dependent modules; repeated success/failure cycles. Record tick p95/p99/max, publication wait and retired-generation memory; checksum state, reject partial cascades and verify equivalent fresh-Play reset. |
| **Editor API and authoring data.** Unity has custom windows/inspectors and UI Toolkit; callback speed does not prove API coverage. [Editor UI](https://docs.unity.com/en-us/engine/6000.6/manual/uitoolkits/uielements) | C# API covers command/menu registration, immutable scene query, selection and typed undoable authoring commands. Scene indices improve repeated ID/name/path/hierarchy access. No complete Unity extension-surface parity demonstrated. [Contract](../dev/csharp-editor-api.md), [scene indices](../../dotnet/src/Ara.Editor/Scene.cs) | **Ara callback measured; API/interaction parity missing.** 10k/100k scenes, deep/wide hierarchy, duplicate names, multi-selection, commands and undo/redo. Separate snapshot ingress from query and paint; verify ordering/ambiguity, typed ops, undo and interaction responsiveness. |

Canonical JSON remains the authored format. A faster derived format must retain
schema validation, deterministic references, invalidation and diagnostics. Measure
the complete dependency closure and first usable scene, not only JSON parsing.
This follows Ara's existing [world cooking contract](../../crates/ara-world/AGENTS.md)
and [typed document reader](../../crates/ara-doc/src/read.rs).

## Whole-engine lanes: quality is part of the workload

These systems already contain substantial code and their own tests/benchmarks.
They need separate, matched tests; a faster ECS loop cannot certify their
throughput. Unity Physics is itself a separate package from Entities/Jobs/Burst.
[Unity Physics](https://docs.unity3d.com/Packages/com.unity.physics@1.4/manual/index.html).
The current Unity fixture does not establish installed/tested physics, navigation,
animation, audio or rendering parity.

| Lane and Ara evidence entry point | Missing matched workload | State/quality gate that cannot be traded away |
|---|---|---|
| **Physics/queries/characters/vehicles.** Jolt default and Rapier reference, engine benchmarks and bowl/scale scenarios. [Contract](../../crates/ara-physics/AGENTS.md), [benchmark support](../../crates/ara-physics-jolt/src/benchmark.rs) | Bodies/contacts/joints, sleeping, CCD, characters, vehicles, 100k rays/casts and snapshot/restore; integration/synchronization included. Compare appropriate Unity physics and Unreal Chaos lanes independently. | Equal shapes, broad-phase filters, timestep/substeps, solver configuration and scene. Report penetration, joint/contact error, tunneling, controller behavior and determinism requirements. Different solvers need explicit numerical/behavioral tolerances, not an unsupported universal bit-equality demand. |
| **Navigation, avoidance and crowd/AI.** Recast, budgeted replans, ORCA/Detour, flow fields and streaming. [Navigation](../../crates/ara-nav/AGENTS.md), [crowd benchmark](../../crates/ara-crowd/benches/crowd.rs), [AI benchmark](../../crates/ara-ai/benches/ai.rs) | Baking and runtime lanes separately; same mesh/obstacles/agents, path requests, moving obstacles, off-mesh links and full avoidance/integration. | Path validity/clearance/area costs, arrival rate, collision/overlap error, replanning budgets and stale paths. Fixed seeds and defined update frequency; do not silently reduce agent intelligence or update cadence. |
| **Animation.** Sampling, compression, blending, root motion, IK, retargeting and motion matching. [Animation contract](../../crates/ara-anim/AGENTS.md) | Identical clips, bone counts, blend/controller complexity and active crowds; CPU pose, skinning and renderer submission distinguished. | Same interpolation/events/root motion/IK targets and compression-error bounds. Match LOD/visibility policy; reduced bone counts or sampling rate are separate quality rows. |
| **Audio.** Mixing/streaming, voice virtualization and optional spatial DSP. [Audio contract](../../crates/ara-audio/AGENTS.md) | Same voices, sample rate/block size, decoded source, effects, resampler, HRTF and stream pressure; loading separate from real-time render. | Waveform/spectral and loudness tolerances, latency, underruns, voice audibility/virtualization policy and allocation-free real-time behavior. Codec field access is not mixer performance. |
| **World streaming and asset pipeline.** Cooked cells, background jobs and bounded activation. [World contract](../../crates/ara-world/AGENTS.md), [runtime assets](../../crates/ara-runtime/src/assets.rs) | Identical camera path/content, asset residency, change invalidation, worker limits and cold/warm caches; cell activation plus assets and subsystem hooks. | No missing objects, incorrect dependencies, stale content or extra visible pop-in. Capture hitches, memory/residency, read bytes and latency; offline cooking costs remain visible. |
| **Rendering, visibility, GPU skinning, terrain, foliage and VFX.** Own Vulkan/Metal pipeline and D26/D27 gates. [Decisions](../decisions.md), [renderer](../../crates/ara-render/AGENTS.md) | Same real-content camera sequence/output resolution and comparable feature settings against Unity HDRP/Unreal; whole CPU/GPU frame and startup, then pass breakdown. **Not run here; Windows is busy.** | FLIP/RMSE against `ara-pathtrace`, temporal stability, shadows/GI/reflections/transparency and identical visibility/LOD intent. No lower-quality preset, missing effects or missing draw work to produce a timing win. |
| **Replication/rollback/saves.** Optional net stack, reflected state and replay integration. [Simulation](../../crates/ara-sim/AGENTS.md), [network](../../crates/ara-net/AGENTS.md), [save](../../crates/ara-save/AGENTS.md) | Same entities/change rates, peers, packet loss/latency, snapshots, rollback windows and save/load state. Compare corresponding networking implementations, not bare ECS. | Convergence, replay hashes under Ara's supported determinism contract, bandwidth, privacy, restore completeness and failure recovery. Local-only remains an independently measured zero-network lane. |

## Unreal comparison scope

Mass is the relevant data-oriented CPU comparison: data-only Fragments,
archetypes, chunks and stateless Processors; EntityQuery returns batches.
Composition changes are deferred by MassCommandBuffer, and EntityView supports
outside-batch entity access. These map to the storage, structural and random
lookup rows above, rather than an Actor tick baseline.
[MassEntity overview](https://dev.epicgames.com/documentation/unreal-engine/overview-of-mass-entity-in-unreal-engine?lang=en-US).

Mass also has chunk/shared access, query requirements and processor dependency
machinery. MassGameplay adds representation, simulation/replication LOD, signals,
movement, SmartObject and StateTree integration. Those are additional behavior,
representation and scheduling lanes; they cannot be represented by W1/W3.
[Mass API](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/MassEntity),
[MassGameplay overview](https://dev.epicgames.com/documentation/unreal-engine/overview-of-mass-gameplay-in-unreal-engine?lang=en-US).

Ara's terrain, AI, animation, renderer and networking code provides places to
implement matching end-to-end scenarios, not evidence they already outperform
MassGameplay. Unreal C++ build/Live Coding and C# iteration need a shared user
action and equivalent state outcome; raw compiler milliseconds are different
work. Unreal timing rows stay **unmeasured** until compatible binaries and
fixtures actually run. No source from Unity/Unreal is copied by this study.

## Next coverage priorities

These priorities are code-based hypotheses and missing evidence, not measured
bottleneck rankings:

1. **Prove membership and lifetime correctness first.** Sparse columns/filters,
   changed ticks, native layout validation and structural refresh need exact
   entity/state tests. A wrong subset can look artificially fast.
2. **Restore current W2/W4 breadth.** Run identical boids/neighbour construction
   and complete churn playback, including real C# query and command paths. Keep
   historical Windows evidence separate from current Mac results.
3. **Measure scheduler overhead and fragmentation.** Tiny jobs, mixed workloads,
   archetype counts and dependency forks/joins can dominate many-script games.
   Reused pointer grids do not eliminate task queues, fences or serial resources.
4. **Design missing C# data surfaces deliberately.** Changed queries, enable
   masks, value grouping and dynamic buffers require ownership, deterministic
   snapshot/reload and safety contracts. Do not bolt raw Rust headers onto the
   ABI or label codec copies equivalent to native span access.
5. **Close the actual huge-edit loop.** Compile a realistic affected dependency
   closure, publish it atomically, migrate live ECS instances and measure the
   first correct tick, foreground hitches and retained-generation memory. Compare
   both realistic modular graphs and monoliths with Unity.
6. **Reduce work before optimizing parsers further.** Trace scene record clones,
   Workspace reconstruction, derived-data invalidation and activation. Validate
   any consuming/cooked fast path against original scene projection and undo.
7. **Open subsystem lanes with fixed quality.** Physics, navigation, animation,
   audio, assets and rendering need representative content and their own quality
   metrics. Match update/visibility/LOD policies; full-frame testing then catches
   integration and contention invisible to isolated kernels.

For every result retain raw samples, code/engine revisions, settings, power/load,
warm-up, run order and worker counts including the caller. Use the same physical
machine and alternate order. Report p50/p95/p99, peak memory, allocations/copies
and cold/warm startup where relevant. Declare the desired margin before running;
show losses and uncertain/noisy rows. Exact-state kernels need matching complete
state, not a convenient partial checksum. Quality-tolerant systems need declared
error bounds and reference output. Time all necessary gather/build/playback work
inside the relevant pipeline. Only a row that passes both its output gate and
its timing gate supports a performance claim for that row.
