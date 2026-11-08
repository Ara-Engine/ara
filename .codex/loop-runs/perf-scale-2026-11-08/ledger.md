# Loop Run: Ara performance and project scalability

Run id: perf-scale-2026-11-08
Mode: work-packet (commits/pushes, no PR requested)
Repo: /Users/islam/.t3/worktrees/ara/t3-6f34c966
Branch: t3/6f34c966

## Objective and constraints
- Improve C# compilation, large edits, Play preparation, data and editor APIs, and CPU execution. User explicitly expanded to continued experiments and Unity ECS/Unreal comparison.
- Mac only: Windows is busy. No Windows/GPU work or global SDK install. Cargo jobs=2; focused gates, no integration gate.
- Measure before/after hot changes; canonical authoring, ECS authority, deterministic scheduling, typed commands, last-good atomic reloads and undo stay intact.
- Commit AND push each verified slice immediately. Workers own disjoint files; root integrates. No PR/merge requested.
- No universal optimality or engine-superiority claim. Name measured lane and limits.

## Committed and pushed

### Prior pass
- 0b079b881 overlap C# analysis/emission.
- ce24048ce reuse validated prefab rows.
- 3b83c37b6 retain compilation/generator state and batch analysis.
- 0260cd275 reusable dense pose resolution. Prior 85 compiler +104 Rust tests; raw Mac phase evidence committed.

### Expanded pass
- fd682b6f6 strict streaming document headers (~14.3→3.3ms for3MB/10k records). 37 doc +39 focused host tests, clippy/fmt/review.
- df8ab4078 bounded parallel document preparation, ordered insertion (~108–114→51–52ms large open/load). 99 host tests, clippy/fmt/review.
- 471f1da30 codec lookup/buffer reuse and depth-scoped A→B→A-safe callback scratch. 23 runtime tests and fresh review; mock8192 codec accesses ~139→13.5ms.
- f4bb8cc6e lazy immutable editor indices. 5001-entity hierarchy298.21→6.84ms including ingress/snapshot;10 focused tests and full suite, independent review.
- ac821daf6 portable snapshot hex without per-byte allocations. 5 ara-play tests/clippy/fmt; dev8MiB175→11ms conversion only.
- 631ee46a1 active project cache retention, one source/inventory capture, source-membership vs owner boundaries, current dependent sources and final atomic checks; explicit committed Full/heat intent preserved. 107 tests +fresh reviews. 1024-component warm1481→1349ms; p95 did not improve; RSS251→329MB.
- 1fe55ddb4 index path reuse +parse-once UI identity. 100 host tests/clippy/fmt/review. Large open~18% less; legacy UI~41%; pinned UI~19% in A/B/B/A runs.
- fb57c9813 five checked native physics views, stable Transform-only ABI prefix/indices. Supplement commands keep codec validation/read-your-writes. Invalid JSON/schema preimages rejected before writes or queued Add/Remove/Despawn. 17 final CLR-backed tests,20 physics/schema,6 generated tests, clippy/fresh review. Mock8192 access10.22→0.060ms,589824→0 bytes; not full tick.
- dfbdaeaf2 bounded two-worker dependency-frontier project compilation with explicit diagnostic order and concurrent per-output cache. Exact PE/PDB/schema/fingerprint tests and full119 tests/review. Paired warm1336.7→1169.7ms,p951356.7→1202.3ms,RSS327→363/384MB. Four workers only~3.5% beyond two in same-binary probe.
- b4a75c6eb related independent-root edit waves and standalone Delta/None publication guards. Fallback only failed current-source attempts; related dirty closures only, disconnected quiet agent edits untouched even with common unchanged ancestor. Final cancellation check after rescans. Full123 tests; root inspected policy fixes after independent review.
- eaa862755 reproducible5120-script full-edit stress. 64modules×80 small generated component/query files; first3requests excluded,6samples. One change/module warm6.239s; ALL5120 files changed warm6.626s,p956.796s,initial7.231s,RSS1.064GB. Source writes outside timer. No commercial-project or cross-engine compile guarantee.

## Toolchain and gates
- Global SDK10.0.203/CLR10.0.7 selected vulnerable ILCompiler10.0.7; full solution audit warning-as-error blocked. Audit not disabled.
- Private SDK10.0.403/CLR10.0.14 installed under target/toolchains/dotnet, global unchanged. Per-command PATH/DOTNET_ROOT plus DOTNET_CLI_HOME under target.
- Full private solution tests passed earlier: Compiler103,Runtime24,Analyzer30,Generator14,RunWorker9. Final focused compiler suite123 passed after later changes.
- Solution tests do not build all fixture outputs. Focused Host, TestModule, Outpost.Sample prerequisites prepared under private SDK; CLR Rust tests use ARA_DOTNET_PREBUILT=1.
- Latest Rust checkpoint: ara-dev fmt passed109packages; touched clippy/tests pass as above. No full integration gate/CI.

## Final verified packets

### Native query tick stamping
Status: committed/pushed (ffa79e966)
- Per-row changed ticks now stamp in disjoint chunk jobs before each C# callback. ABI and one-pointer task payload unchanged; persistent scratch reused.
- Readonly/added ticks, table summaries, fault/disabled results, missing resource and later-unmapped-column side effects preserved.
-13 lib +6 generated +8 host +1 native_views tests passed; clippy/fmt and independent review clear. Broad40-process matrix retains all small/single-worker variations and task-pool allocations.
- Original allocation-counted timing matrix is retained and labeled instrumented.
- Final matched holdouts use counters OFF during80 latency samples;8 separate untimed allocation ticks. Four paired cases/all8 processes pass exact full-row bits/ticks. Cold1m/15worker query0.384–0.387→0.311–0.315ms (18–19.5% less); hot0.726–0.728→0.672–0.674ms (~7.4% less). p95 improves in every pair; allocation probes match.
- Baseline system.rs swap/build was protected by try/finally; candidate restored exactly. Shared current ctx/cmdbuf and new harness fingerprints matched both builds. The follow-up harness/evidence/docs are included in this checkpoint.

### Controlled Unity ECS comparison
Status: committed/pushed (4ec027813)
- Installed Unity6000.4.5f1/Entities+Collections6.4/Burst1.8.29/Math1.3.3, isolated scratch; no Windows/engine install/license change.
- Optimized native Rust simd lane, nativeCPU/thinLTO/codegen1. Two fresh reversed rounds,1m rows,1runner or14workers+caller15 budget, nice10 both engines; warmup120 W1/3000 W3,300 measured steps.
- All8 pairwise48MB full-state BLAKE3 comparisons passed, all8 effective flags and preference restoration proofs passed.
- Controlled median p50 Ara/Unity ms: W1serial3.816/4.391; W1parallel0.436/0.470; W3serial2.879/4.525; W3parallel0.435/0.400. Ara leads3 tested p50 cases, Unity leads parallelW3. No general/whole-engine or C# gameplay win claim.
- On battery40→36%, ordinary desktop load; recorded guards/power/process/source/executable metadata. A transient cargo-fmt rustc probe caused one pair to be discarded/repeated; no contaminated retained pair.
- Correction documented: initial fixture set Burst options and could write EditorPrefs; initial preference value was unrecorded, no guessed restoration. Historical matrix remains exploratory. Final wrapper captures/restores four option values plus exact five known preference values/key presence, verifies finally success, and controls force-safety/debug=false before timing.
- Root moved non-batch guard before Exit so manual invocation cannot close interactive Editor. Numeric batch path unchanged; measured/current wrapper hashes both documented.
- Unreal unavailable; no fabricated timing. Rendering, whole Play/editor and shipping AOT/IL2CPP unmeasured.

### JSON-backed engine writes
Status: committed/pushed (1f9dcc0d9)
- Unjournaled comp.json replacement checks component presence, parses incoming Value once, uses same validated SimRegistry writer. Removes unused old serialization and redundant Value→bytes→Value.
- Reads, generic serde wire semantics, raw module writes and all journal/undo paths unchanged.
-16 fresh cold real-host runs cover Light, AudioSource, graph8/graph64; counters outside latency. Audio cases14–17% less time, Rust allocations14–20% less. All raw distributions retained.
-35 focused tests, perf all-target clippy -D warnings,109-package fmt check, exact callback equivalence and independent review passed.

## Final checkpoint and remaining limits
- User-facing results should link the checked-in evidence/research and interactive chart. No claim that no further optimization exists.
- Source/measurement fingerprints verified; final formatting check passes109 packages after the last harness update. No blind full integration gate.
- This checkpoint includes counter-disabled query holdouts/harness + research/ledger. Verify the final push and clean tree; no PR requested.
- No active Cargo/.NET/Unity measurement job remains; all workers finished. Mac only throughout.
- Architectural follow-ons: cooked scene projection/dependency closure, variable-sized read/bulk API, custom sparse-native data/filter traversal, broader editor extension surface, migration/save-to-first-tick measurements and remaining parallel particle gap. Keep these features on roadmap; they are not proven complete by this pass.
