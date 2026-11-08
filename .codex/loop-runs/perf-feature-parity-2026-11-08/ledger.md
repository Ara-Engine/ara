# Ara feature-wide performance continuation

Mode: work-packet; same t3/6f34c966 branch, starts at20e8ccdbc.
Objective: expand beyond W1/W3 to ECS, Jobs, Burst, structural/filter/variable-data/dependency workloads, scene preparation and iteration. Improve measured losses with matching algorithms/state/quality. Universal superiority in every scenario cannot be certified; never hide losses or lower correctness to claim it.

Constraints: Mac only; Windows remains busy. Cargo jobs2. No concurrent CPU measurements, including cargo/rustfmt probes. Private SDK target/toolchains/dotnet/CLR10.0.14 available; ARA_DOTNET_PREBUILT=1 for CLR tests. Commit AND push each verified slice, no PR requested. Preserve locked decisions, canonical authoring, ECS authority, deterministic state/structural order, last-good reloads and undo.

Ownership:
- feature_inventory: only docs/research/12-performance-feature-matrix.md; primary-source coverage/gaps/quality contracts. No builds.
- ecs_research: benchmarks/iteration/unity/ and new matched feature fixture files there; W2/W4 extension first, all original S8 math unchanged. No ara-sim source edits.
- native_views: ara-script ecs/system.rs, system_tests.rs and new sparse interoperability tests/examples; correct sparse data and filters while dense fast path unchanged.
- data_loading: ara-play +ara-player scene preparation seam, first inspect actual ownership/retry contracts and propose measured consuming/bulk path.
- root: ara-sim benchmarks/kernels and cross-cut integration, verification, comparisons and evidence.

Initial packets:
1. Feature inventory including NativeContainers, dynamic buffers, change/shared/enableable filters, jobs/dependencies/reductions, structural playback, hierarchy/prefabs, loading, compilation/reload/migration/editor and subsystem lanes.
2. Matched W2/W4 executable baseline and richer feature fixtures; all state/operation/order checks, actual worker budgets, versions, options, power/load provenance.
3. Sparse query correctness implementation, public native SparseSet component reproduction and dense no-regression probe.
4. Scene/Play preparation optimization with production caller use and authoring/state preservation.
5. Profile remaining execution gaps (parallel particle, boids, dispatch, memory) and implement defensible improvements; distinguish benchmark kernels from engine runtime benefits.
6. Independent review and focused gates, immediate commits/pushes, transparent full coverage/results report.

CPU/Cargo slot: unassigned initially. All children explicitly holding heavy work until root grants it. First timing grant planned ecs_research W2/W4 baseline after static extension; root can run bounded existing-executable probes before that grant.

## Current progress
- Research feature inventory complete, primary links/source checked; ready to commit. Unity buffer/shared/enable/change/containers/job graphs are separate coverage, not represented by W1/W3.
- Root static benchmark changes: optimized fixtures retain query plans; W4 post-timing checks assert every stable ID/component and unique logical slot. Not a production FPS claim. No compile yet.
- Existing executable particle batch sweep:4k~0.454–0.464ms,8k~0.324–0.332ms, identical numeric hashes. Exploratory configuration only, all samples retained under target.
- Sparse baseline actual red: ARA0934 explicitly rejects canonical-layout SparseSet Transform at startup. Worker authorized minimal binding guard removal only with correct fallback; do not describe baseline as silent public misfiltering.
- Batch C# span API worker added: owns .NET generator/query docs+new tests, no Rust system edits. Existing[Query] may accept Span<Component>/ReadOnlySpan<Component>, one callback per native run, same descriptors/ABI; RNG ambiguity rejected. Source/tests ready, SDK check pending.
- Scene worker source/tests prepared: decoded scene ownership and moved records/UI, explicit take_workspace, retained authoring mirror/retry bytes/CBOR unchanged. CPU probe10k/100k and player check pending.
- W2/W4 baseline6pairs R1 pass numeric+livecount+flags/restoration. Ara/Unityp50:20kboids7.106/7.763ms serial,.747/.737parallel;50k30.814/36.781serial,2.754/3.003parallel;W4.609/2.272serial,.817/10.513parallel. W4 includes different command representations/bookkeeping; not API-identical10x claim. Stable IDs not exported/asserted in legacy dump, disclosed.
- R2 paused twice by actual unrelated t3-4359603b Cargo/clippy/editor-test linking. No foreign job killed or paused. External PIDs exited; remaining R2 now granted exclusive CPU again.
- User answered continue on battery; no more power requests/global changes. Record per-process conditions, continue while safely executable; no invented AC prerequisite. Latest battery14%.
- All own source workers hold Cargo/.NET/format/metadata/timing during baseline. After release: private SDK batch generator/compiler tests first, then focused Rust correctness/player checks. Keep measurement slots exclusive.
