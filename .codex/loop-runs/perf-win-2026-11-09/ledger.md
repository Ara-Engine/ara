# Measured wins over Unity continuation

Mode: work-packet. Branch t3/6f34c966; starts fdc447ee0.
Objective: continue implementation and matched timings until the representative suite beats Unity by a considerable margin, rather than stopping at correctness. Target: >=20% lower Ara process p50, no material p95 regression, equal algorithms/state/quality and declared worker ceiling. Every failing or uncertain row remains open. Never certify all arbitrary workloads/hardware from a finite suite.

Constraints: Mac only, Windows untouched; user authorized continuing on battery, no further power questions/changes. Current observed AC100%, 60% memory free, 200GiB disk free; no competing compilers/Unity. Cargo jobs2, nice10; sequential CPU/build/timing slot. Preserve D11/A12/A13, ECS authority, fixed semantics and full output gates. Commit and push each verified slice immediately.

Ownership:
- root: ara-sim native kernels/benchmarks, production runtime changes and integration/evidence.
- ecs_research: benchmarks/iteration/unity/ control harness/evidence only. Source-only until root CPU grant. Original S8 C#/MacBatch frozen, use explicit new guarded fixture family for fresh comparisons; no marker spoofing.
- batch_queries: C# kernel example/helpers and direct_kernel_reload test only; source-only outside explicit shared CPU grants.
- allocation_pipeline_review: target/feature-buffer-candidate prototype only; measured feature fixture sources remain frozen until baseline completion or explicit handover.
- current_comparison_review and script_executor_review: read-only independent evidence/code review, no heavy work during another timing grant.

## Slices

### W1/W2/W3/W4 matched performance
Status: implementing
Acceptance: fresh reversed engine-order rounds at serial/parallel workloads/counts with exact output gates and declared margins. Original baseline/evidence preserved. Benchmark-only speedups must be labeled separately from production gains.
Attempts: existing cached executable W2 phase profile; next native SIMD neighbour screening and dispatch tuning, then matched Unity execution via durable recovery controls.
Blockers: none currently.

### Feature timings beyond independent rows
Status: implementing
Acceptance: strict timed query/change/structural/dependency/buffer lanes after the completed50 independent-oracle correctness pairs. Any observed losses drive implementation, not a completion claim.
Progress: 12/36R1pairs valid,6targetwins6misses, full100k420independentoracles. Remaining24pendingafterUnitySIGBUS; exactknownsettings recovered, oldcache/failedattempt preserved. Freshownedcache-isolation diagnostic proposed, noautomaticbenchmarkretry.

### Actual C# and data/runtime integration
Status: implementing
Acceptance: profile and reduce real dispatcher/serialization/buffer/compile-to-first-tick costs, preserve state/reload/authoring, measure actual runtime benefits separately from native fixture changes.

### Checkpoints
Status: active
Commit/push verified improvements as they pass. Do not stop simply because a fixture is correct or a task plan is checked off while target rows still fail.

## Progress
- User correctly challenged prior stop before performance target. Goal stays active; correctness alone is not completion. Current measured competitive/losing rows remain open until fresh comparable timings and >=20% lower p50/no materialp95 regression.
- W2 current cached profile20k snapshot.016ms/grid.076ms/neighbour+dispatch.648ms;50k .038/.166/2.527ms. Retained under target/perf-win-2026-11-09.
- Four exact-output native experiments rejected rather than adopted: four-neighbour SIMD screening, conservative cell-bound pruning (rounding-safe widened cells), spatial scratch solve+publication, same-cell four-boid SIMD lanes. All passed reference bit gates but none reliably improved20k. Source/binaries/reversed raw timings retained in target rejected-* and *-probe folders. Working native kernels restored; only stronger exact static-ID/component assertions retained.
- Stable native baseline39b51adede901aa9f80dc19398a9accd5d8f5c1b3ae5ca1d7a01315bc9451c82 has complete source/root lock/compiler/build provenance; all4 scalar/tuned/SIMD validation gates and focusedbenchClippy pass. Source frozen during current matched processes.
- New explicit s8-current-v1 harness preserves original numerical kernels/MacBatch and extends durable recovery controls. Independent review found transient midprocess power switch acceptance and a shared feature build-record TOCTOU; root fixed both with targeted tests. Current11+features38 mocked tests pass; old evidence not rewritten.
- Actual fresh Unity C# bootstrap imported/compiled; metadata drift correctly rejected/unaccepted, re-prepared. Four paired smokesW1/W2/W3count1003/W4exact100k (2warm+3steps), exact IDs/components and complete numeric hashes agree with native baseline. Every known option/preference restored, no unresolved guard. Full20 timed pairs starting with W3batch8192 as measured configuration, two reversed rounds; no conclusion until complete.
- Real C# W1/W3 ScriptHost span/Vector128 prototype source frozen in new ara-script example/helpers, independent scalar per-field oracle and Unity-compatible48B dumps. Build/actual execution slot pending current Unity timings. No production/runtime performance gain claimed from a fixture alone.

### Verified checkpoints while target remains active
- Pushed68ca5b732: exact current native IDs/control harness+four actual smokes. Full20 fresh current S8 pairs now pass quality/controls; native W3batch8192 meets20%/+5%tail target in both budgets/rounds. Native W1 and W2batch4096 remain below target; oneW1parallel p95+12.6% retained. W2default4096 gives onlyfive20k tasks, unlike earlier best128, so no claim this is bestW2. Fresh128two-round followup queued. W4 faster as legacy workflow, not identical API/bookkeeping.
- Production ScriptHost change verified: one-system outer caller dispatch avoids needless worker relay; multiple systems retain parallel scheduling, pool0 uses sequential outer path. Independent review corrected a false premise: Bevy already drains global tasks forpool0, so no deadlock fix claimed. All20smallCLRsmokes and15focused integrationtests pass including finalCommands/queryrefresh/reload1->2->1/zero-pool/host/modules/batch/sparse; newexampleClippy passes. Pairedbuild inputs differ onlyhost/mod.rs, guarded source restoration verified.
- Actual1mC#SIMD before/after executor measured8 fresh processes in two reversed rounds with identical48B/ID state. Small~1-5%p50 changes, variable tails, no20%enginegain claim. Evidence csharp_executor_mac_2026-11-09.json. RealC#S8 spans/intrinsics example is a usable public API workload; optimized andhot/partial/bounce correctness covered.
- Direct actualCLRvsUnity completed8fresh pairs, two reversed engine/case rounds,1mrows,300samples, exact48MB outputs plusIDs/components/scalarreplay. All8meet20%p50/+5%p95 target: meanp50 W1serial1.365vs3.735ms,parallel.326vs.473; W3serial.868vs4.529,parallel.239vs.550. Release/CoreCLR10.0.14 observedbeforeinitialization; HotDebugcorrectnessonly, nottimed. Actualcandidateall-targetClippy passes. Remain active: boids/nativeW1/featuretimings/Hot/otherAPIs unresolved.
- Pushedd9b13e841: production callerdispatch, actualCLRtests, publicS8example and8matchedReleaseCLRvsUnitywins. Independent evidence review recomputedpercentiles, comparedall48MBdumps/IDs, verified28fileproofs/receipts/runtime/builds; no mustfix. This is vectorizedpublicAraC#versusfrozenUnityBurstkernels, not isolatedCLRcompiler superiority.
- Native ARM64 exact transpose/hardware-sqrt experiment built b87de1945057bded57f29def61523cd9dcc72fd4ce161a1f2e679971fb1e4201; all4workloads pass scalar/tuned/SIMD bit gates at1003/12steps forserial+14workers. All16freshreversed before/afterprocesses haveidenticalfullstatehashes andacceptedresource/powerguards: W1serial3.916/3.921->1.295/1.297ms,parallel.436/.432->.349/.349; W3serial2.892/2.887->1.094/1.126,parallel.319/.320->.286/.286. BenchClippy/fmtandindependentunsafe/mathreviewpass. Benchmark-only change, no production claim. Fresh8matchedUnitypairsunderway; boids andfeaturetimingsremainopen.
- Pushed81cf10ad5 nativeexactSIMD and60f1f9214 fresh8nativeW1/W3wins, alltargetmet/allp95lower, allfullstatehashes/IDs/controlsagree. Independent evidence audit passes; original20lossesremainvisible. Nativehelper changesbenchmarkkernels, notproductionsimulation.
- Merged adjacentXcellspans retainexactneighbourorder, reducing27cellloops to9contiguousspans. E2AE candidate all4bitgates at1003/20k/50k;8validparallel beforeafterprocesses improve20k .746/.743->.634/.654ms,50k2.763/2.813->2.606/2.632. Longserialholdoutstwice rejected for unrelatededitorcompilation; shorterfirstorderpasses20k7.104->5.663ms and50k31.147->28.264,reverse interrupted/rejected. Allattemptsretained, nofullserial/crossUnitywinclaim. CombinedSIMDscreen slower anddiscarded; batchsweep64/128/256/512/1024 confirms128competitive. AraSimtests,all-targetClippy andworkspacefmt109packagespass. Furtherboidsmargin stillopen.
- Next: representativefeaturetimings, realHotDebugdirectintrinsics experiment under EnC, sampleMovementSpan actualNativeAOT gate. Source-only.NET10review rejects DebuggableAttribute/AggressiveOptimization bypass as unsupported; noheat/qualitycontractchanged.
- Pushed8652e54d6 mergedXspans. C004cached12bitkeys+conservativey/zslabbounds independentreviewpasses, adversarialplanes+1003/20k/50kall4bitgates pass. All8freshparallelbeforeafterfull420stephashes/guardsagree;20k .605/.607->.594/.594ms,50k2.378/2.378->2.323/2.315. AlltargetClippypass. Furtherforceinlineexperiment no gainandrejected; source/binary/proofretained. MatchedW2stillpending, no20%leadclaimfromoldUnityruns.
- Hotdirectintrinsics candidatepassedactualL1/partialtailbitproofanddisasmconfirmsEnC/MinOpts, butreviewcaughtalignmentcontractgap. Fixedbytebases+ReadUnaligned/WriteUnaligned sourceclosesP2; newactualgatesrunning, oldcandidateproofimmutable. NoHotperformanceclaimyet; nativeW2/featuresremainactive.
- Pushed55956bd14 featureR1timings+SIGBUS/recoveryevidence;6missesstayvisible. First64wordUnitycompilationcrash-10 hasunknowncause, sameBufferJobas4words; exactcheckpoint2ad1... recovery03ad... restoredallknownflags/prefs, noacceptedPhaseBpair/nobenchmarkretry. Narrowtempderivation preservesuf-t/ufx-t andisolatesnewufi-t;40mocktestspass. Originalkernel/template mathunchanged.
- ProductionC#dependencyplanner nowuseslastwriter/activereaders+laststructural insteadallpairs; totalchains usecaller, forksparallel. 1024mixedgraphreachabilitychecks+10kwriter/structuralcasespass, independentreviewno mustfix. All15CLRgatespass; strengthenedforkfreshCounterStampvisibility+zero-pool2reruns pass, alltargetClippyandworkspacefmt109pass. Opt-in graphprobe acceptedAC/quiet/sourcefrozen:512jobBevyinitialize79.638->11.075ms;2048nodeplan4.020->.126ms. Oneprocessalternating6/20samples, test/devprofile, notRoslyn/fullPlaylatency. InitialteststdoutJSONprefixparserfixedbyreparsingpreservedrawonly; no measurementrerun/modification.
- Nextsourcecandidatesremainunverified: cached9neighbourspansperoccupiedcell+diagonalradiusvalidation; native-ISAHotdirectC#hardwaremathqualitygates; contiguous/NEON4SmallVecprototypeundertarget; actualSpanMovementNativeAOTexecution. Keepworkingonlossesafterthisgraphcheckpoint.
- Pushed9c7a42779productiongraphfix anddb8a5589cprivatefixturetempisolation. VerifiedHotdirectdffcandidateall16fresh1m+3rowprocesses/8pairs accepted, identical48,000,144bytedumps/perfield/IDs. NativeISA+unalignedbyteI/O+inlinefloattranspose improvesHotSIMD1.80-1.97xwithbettertails; stillfarabovecoldlane, notUnitywin. Actualquality12smokes/L1enabled+HWdisabled/CoreCLR10.0.14 traces; perfidentityhash-boundonly. Quad24/32indirectcallsremainunresolved, nocall-freeclaim. Independentstage/evidenceauditpasses. CommitVERIFIEDdff4filesfromimmutablepacketindex; workingkernel-moduleexperimenteditsretaineduntouched.
- Cachednine-spanplan09passesqualitybut20kneutral/50k~2%gainwithextra288KB; simplerlookup-only666candidateusesalreadycached12bitcellkey, noextraallocation,20k .596/.597->.587/.586ms,50kroughlyneutral. All8internalparallelpairs andfullstate/IDs/plane+newdiagonalradiusgatespass; sourcegates/checkpointpending. AvoidoverengineeringrangeplanbeforefreshUnityW2.
- Optionaltwo-moduleHotGame/coldKernelprototype sourceprepared(publicborrowed-spanBatchonceperchunk, existingmathunchanged, GameactualL1mustremain); notcompiledyet. Bufferprototypebaselinequalitypassed, contiguous/SIMDgatesrunningundertargetonly. Unitynew-ownedcache-isolationdiagnosticpreparedbutnotlaunched;24remainingfeaturetimingspending. FurtherunusedCmdBufelisionauditprovespurequeriesdon'temitops, buthas_deferredMUSTstay(unrelatedstructuralvisibility); noimplementationyet.


## Main integration, 2026-11-09

User reported substantial newly pushed main work. Fetched `origin/main` at
`1e04fe5dd9d04ede28592597dace9c0c06a15f9a`: 277 commits beyond the common
base, including host ABI 5, graphics preferences and published watched-assembly
reuse at Play start. Merge this history; do not cherry-pick or discard it.

- Pushed `b474d043c`: cached boid cell lookup, exact diagonal boundary gates;
  all-target Clippy and focused correctness pass. Fresh Unity W2 comparisons remain open.
- Pushed `e15734bba`: optional explicit cold Kernel + editable Game wrapper example;
  same borrowed ECS columns/math, no SDK flags or reload-policy bypass. Independent
  review passes; 32 small actual-CLR cases across span/SIMD/direct and real Game L1
  tests pass. Editing Kernel still follows its cold-module reload policy.
- Two timing attempts stopped before launch because unrelated compilers appeared.
  A third completed 9/16 accepted internal cases before explicit cancellation for
  main integration. Incomplete reversed rounds, no Unity claim. Frozen raw proof:
  `target/ecs-hot-kernel-module-20261109-cbe087-r03/partial-summary.json`.
- Buffer prototype: 12 accepted internal fixed4 processes, original baseline retained,
  full 420-step outputs and capacity/spill equal the independent oracle. Contiguous
  and exact NEON candidates improve both budgets; tick cells checked separately over
  six serial steps only. No fresh Unity comparison. Publication packet under
  `target/feature-buffer-candidate/proof/buffer_fixed4_internal_mac_2026-11-09.json`.
- Unverified tracked buffer promotion and AOT span sample safely saved in stash
  `4adca42d2c99e20c9ec92e7dd15cad563fb5227d`. Apply after the merge; do not drop it.
- Conflict resolutions retain both `dependencies` and `graphics` modules; player
  initializes graphics revision and moves scene UI. Incoming shader sources/cooked
  cohorts remain byte-identical to main; no shader-source conflict or mixed cohort.
  Mac is not a canonical DXIL cook host; Windows remains untouched.
- Canonical generation: all 6 ABI/schema tests pass with no generated-file changes.
  Fresh .NET solution build passes. Runtime codec test exposed three new component
  recipes missing from the older example set (Decal, LocalFogVolume, VertexPaint);
  added them without weakening coverage. All 28 Runtime and 124 CompileServer tests
  pass. All 22 real-host tests across 11 suites pass, including every-component
  write/undo and module L1. Simulation/Play/runtime library gates and all-target
  Clippy for scripting, simulation, Play, runtime and player pass. All 55 selected
  library tests pass. Formatter passes for 114 packages; four inherited main Rust
  files needed formatting only. Shader sources and cooked cohorts remain unchanged.
- Preserved the pre-merge .NET output and hashes under
  `target/perf-main-integration-20261109/pre-merge-dotnet-bin`. Earlier timings remain
  bound to their immutable revisions/binaries; rebuild before combined-engine claims.

The performance objective remains active: remaining feature losses, actual fresh
boids and editable-wrapper Unity comparisons, AOT span validation and the fresh-cache
Unity diagnostic are unfinished. Main integration is not a completion claim.

- Main integration committed and pushed as `673c82153`; ancestry includes the fetched
  main history. Restored the preserved buffer/AOT work; backup stash remains retained.
- AOT sample Movement now uses borrowed component spans with identical scalar math.
  Both actual NativeAOT tests pass, including 1,003 partial-batch rows with per-row
  movement assertions, separate-world shutdown/isolation and commands/undo. Fresh
  solution build and all 8 JIT host tests pass with the same sample; focused Clippy
  and 114-package formatter pass. This validates shipping API support, not an AOT
  speedup measurement. Raw logs: `target/perf-main-integration-20261109/`.

- Fresh ABI5 kernel packet `target/ecs-kernel-module-abi5-20261109T111347Z` passes
  actual Game L1 plus cold Kernel L3/dependent relink at one boundary, both workloads,
  full fields/native identities/tails and unload. Independent review found a cumulative
  report-proof gap; the test now checks every boundary's report count and both module
  heat/generation transitions. All 32 small mode/topology/budget cases and focused
  Clippy/build/disassembly gates pass; no fresh throughput/Unity claim yet.
  Executable SHA `577f998f61e9db2f5c35974c1b0219def5428cb741b49275b098e012e5fb81e5`.
  One target-only controller filename error and one packaging syntax error are retained;
  only the missing direct-mode checks resumed against the correct frozen SIMD controls.
  No simulations were silently accepted from failed controllers or repeated wholesale.
- Buffer harness now explicitly selects and isolates baseline/contiguous/SIMD variants;
  both launch and offline verifier bind command, flags, architecture and archived build
  bytes. All 49 focused Python contracts pass. Integrated Rust variant builds/tick gates
  remain pending; prototype timings are not relabeled as combined-main measurements.

- Integrated native buffer variants now pass all three actual harness builds, release
  tests and Clippy. Both candidate variants verify serial and parallel Added/Changed
  cells, values and SmallVec storage transitions. All 42 actual native cases agree
  with fresh original scalar oracle bytes and old immutable baseline/storage; one
  harness smoke is reused, not rerun. Actual snapshot/label and offline build binding
  pass. Independent review recomputed small dump bytes/receipts with no must-fix.
  Public integrated quality summary and pre-integration internal fixed4 timings are
  separate artifacts; no fresh Unity or combined-main timing claim.
- ABI5 internal r01 rejected for unrelated compilation; r02 accepted its first eight
  cases/four pairs, then cooperatively stopped for a newly found startup issue. All
  raw proofs/source guards retained. Reversed round unfinished; no market claim.
- Startup audit found prebuilt watched modules lack compiler dependency state on first
  edits. Prioritize actual Core->Game leaf/root edit repro and correctness fix before
  extending the new-main performance captures. Nothing is silently called complete.


### Published watched graph regression (verified)

- Actual old-SDK first-leaf repro fails ARA0904: Game references Core, which has
  not built. First-root repro changes Core generation but Game keeps value11
  instead of12; only Core Initial/cascade1 is reported. Raw before logs retained
  under `target/perf-main-integration-20261109/prebuilt-*-before.log`.
- Stronger Rust fixture now covers Core->Game plus an independent broken Unrelated
  module, both edit directions and cold/hot modes, zero-compile published startup,
  atomic L3/heat/generation/native identities, subsequent L1 and failure/recovery.
  Its old-SDK leaf repro reproduces the same dependency failure.
- batch_queries owns the bounded C# fix: explicit published-loaded PE/source snapshot
  separate from compiled Roslyn ModuleState, exact loaded-image/source identity,
  first-edit L3/new baseline and dependent inclusion. Do not fake a compilation or
  invoke all-graph RecoverAll (unrelated saves and cold heating would be wrong).
  Public Host ABI5 stays; the internal Host/compiler IPC can version its payload.
  Root owns Rust fixture and all test/build grants. Source implementation in progress.
- Frozen ABI5 throughput r02 has8accepted round0 cases/4pairs, then a clean STOP
  before reversed round; all full48B states/source/resource guards pass. Evidence
  remains partial; no fresh Unity/market claim. Compiler fix takes priority.

### Further bounded experiments retained

- `target/perf-feature-batching-candidate/`: source-only control plus fixed32768
  query batching, safe contiguous DAG columns, and combined variant. Four real
  schedule stages/dependencies stay; dirty Changed filters/selected epoch stamping
  stay unchanged. ONE combined quality configuration granted, no timings/Unity yet.
- Compile audit found repeated publication artifact reads/hashes in ownership scans
  and reparsing unchanged consumer trees after dependency/configuration changes.
  Address after published-state correctness. Preserve public inventory hydration,
  fresh-build-equivalent bytes, dependency error/recovery and corrupted-cache rejection.
  Project compilation already has two independent-module workers; no speculative
  live-reload concurrency redesign is authorized by this finding.

- Published-loaded implementation verified: exact PE/PDB/source/dependency snapshot,
  no synthetic compilation or baseline, first edit L3/new baseline, atomic published
  dependents, unchanged saves skip parsing. Explicit unwatched images remain exact
  references when saved source is invalid; untrusted source bodies stay unknown.
  Internal IPC v3; public Host ABI5 unchanged. Missing/corrupt source snapshot or
  complete PDB-byte mismatch forces a real compile, without blessing modified caches.
- Independent source review has no remaining must-fix. Fresh solution build has zero
  warnings/errors; all156 CompileServer tests pass. Four actual published-runtime
  families pass in cold/hot modes, plus13 reload/lifecycle/recovery/multi-world tests.
  Native identities, first-edit generations/heats, atomic cascades and later L1 verified.
  Errors from an unrelated module remain active during edits and repairs.
- Initial new-fixture failures retained: diagnostic reports were incorrectly counted
  as applied code, then queued historical error rows were treated as current failure.
  Assertions now inspect actual reload reports and latest structured completion scopes.
  Shutdown cancellation no longer escapes the compiler worker. All-target scripting
  Clippy passes after one borrowed-slice fixture correction; formatter114 packages passes.
  Raw logs: target/perf-main-integration-20261109/published-*.log and
  prebuilt-watch-scope-recovery-fixed.log. No runtime throughput/Unity claim from this fix.


### Dependency/configuration syntax reuse (verified; limited timing scope)

- Committed/pushed published-module fix as bf3a04b3a. Performance target remains active.
- ProjectCompilations now keeps identical path/text syntax trees across dependency,
  heat and profile changes while creating fresh semantic compilation/generator state.
  Changed-reference errors, recovery, heat, fresh PE/PDB/generated output equivalence
  pass; full157 compiler tests and solution build pass. Independent source review clear.
- Added compiler_scale --dependency-fanout: one shared constant edit rebuilds8unchanged
  256-script modules plusCore. One clean guarded baseline->candidate pair accepted,
  3 discarded requests +4 measured per process: p50 5021.96->3569.09ms, p95(max4)
  5376.52->4401.40ms. All7request schemas agree. Actual CoreCLR10.0.14 observed.
  Public packet benchmarks/iteration/compiler_dependency_reuse_mac_2026-11-09.json.
- Baseline SDK was built from source later committed bf3a while HEAD4b245; candidate
  embeds bf3a. All changed SDK method bodies audited: only CompileServer.Generate
  differs; other changed managed SDK methods/signatures/IL/local/EH bodies equal.
  Complete hashes/version caveat/raw method audit retained, no source-map ambiguity.
- Retained initial fixture schema-file mistake and unaccepted competing-build run.
  Reverse guarded02 attempt conservatively rejected its own just-started child due
  ps ownership sampling race; guard narrowed to exact argv+live parent and rejected
  run stays unaccepted. Guarded03 reverse attempt rejected a new active Unity
  compiler. One order only, four samples per process, no stable-tail/Unity/Play claim.
- DAG prototype sixteen internal cases passed full original420step output guards.
  Parallel DAG improved, dirty inconsistent, serial chain tail regressed: no blanket
  promotion or fresh Unity claim. Contiguous-only isolation quality build/120frames/
  240actual observers/fourfull dumps passed. Timings await clean CPU slot.
- Ownership-only scan change source frozen by batch_queries; public/default/full
  publication hydration and excluded editor results preserved. Actual .NET build,
  dedicated test and runtime gates still pending; not part of measured SDK above.


### Ownership discovery without publication hydration (verified)

- ModuleInputs.Scan now explicitly skips derived publication reads, while the one
  shared discovery implementation and public/default/full ProjectCompiler scans
  preserve their behavior, including excluded editor publication schema/fingerprint.
  No new cache or graph semantics. Actual publication/corruption/unknown-reference
  test passes; full158 compiler tests and fresh0-warning/error solution build pass.
  Seven actual host tests pass: first published leaf/root edits, mixed unwatched
  images, module cascades and project lifecycle. Source review: the hydration flag
  gates only the existing derived publication branch, not discovery/diagnostics.
- Guarded actual16-module/1024-script publication scan comparison accepted. One warm
  process,30samples per block, both orders; every normalized inventory equal.
  Full->ownership p50 6.36585->3.94190ms, reversed 6.33315->3.95715ms; both p95 lower.
  Median managed scan allocation2,918,936->1,140,152bytes. This is only scan cost,
  never total startup/Play or a market comparison. All source/binary/power/CPU
  guards pass. Public packet compiler_ownership_scan_mac_2026-11-09.json.
- Target-only preparation helper initially used invalid reflected names containing
  underscores (ARA0704); preserved errors/invalid source, corrected names and emitted
  the actual fixture. No preparation timings accepted. No production naming relaxation.
- Three-way DAG timing attempt stopped before its first process for an unrelated
  active ara-editor compiler; zeroaccepted. Native quality and prior limited internal
  gains remain retained, not promoted to a fresh Unity or combined-engine claim.


### Incremental changed-file parsing (candidate; checkpoint gates in progress)

- CurrentSDK kernel quality refreshed under
  target/ecs-kernel-module-current-20261109-20261109T130705Z:32actualsmall cases,
  realL1/coldKernelL3 atomic cascade and2disassemblies pass. Rust577f99 unchanged;
  SDKactualDLL hashes/new fixture source captured independently ofembeddeddb76
  version. No throughput/Unity run or freshly rebuilt SDK claim from this packet.
- Native three-way DAG isolation now completes18accepted processes with exact420step
  originaloracles/IDs/updates and source/exe/power/CPU guards. Contiguous-only helps;
  combination meets20%/+5% internalparallel target vsbaseline, but some incremental
  batching tails lose vscontiguous-only. Serialoneorder limited; older tail loss
  retained. No Unity/production claim. Packet dag-isolation-timing-02/summary.json.
- ModuleSession and ProjectCompilations fully parsed changed files even for a local
  body edit. Prototyped precise common UTF16prefix/suffix plusRoslyn incremental API.
  Primary sources: Microsoft Learn SyntaxTree.WithChangedText and dotnet/roslyn
  SourceText.cs/ChangedText.cs. No classifier/EnC safety flags changed.
- Ordinary SourceText.WithChanges prototype passed14freshtext/checksum/diagnostic/
  PE/PDB cases, guarded parser comparisons. Smallfilesregress; largefilesimprove.
  Repeatededit source history/backing storage needs care; oldmemoryincremental run
  rejected forunrelatedcargo, no throughput claim fromit. Rawtargetpacket retained.
- Flat change provider owns complete new text, weak immediate predecessor and exact
  range; ancestors/unrelated texts conservatively get full replacement. No strong
  source-history chain. Flatprototype parserguard passed; memoryfull process rejected
  forunrelatedcargo andflat1000edit trace captured asqualityonly, neverquietthroughput
  or boundedtotalcompiler memory. Roslyn/global compiler caches remainoutsideclaim.
- Production candidate uses flat provider at>=2048characters; tinyfiles/pathchanges
  parse fresh. Matchingbefore/tree invariant atbothcallsites reviewed independently.
  Above-thresholdUnicode/directive/error/recovery/PE/PDB tests,2047/48/49crossings,
  self/immediate/ancestor/unrelated ranges andgenerator+image equivalence allpass.
  Initial8test failures were ImmutableArray identity assertion; bytearrays corrected,
  exactchecksum equality remains enforced. Full168compiler tests and0warning/error
  buildpass. Actuallargefile levelsL1/L2/L3 andbothkernel/directreload testsallpass.
- Rustlevelsfixture paddedabove cutoff. Batch_queries owns only reload_bench.rs
  optional single-file/SDKpath/rawJSON sourceextension; Root waitingfreeze forfocused
  fmt/Clippy andactual full save-visible comparison. Do not commit/claimfinalspeed
  before remaining checkpoint/measurement decisions; user goal remainsactive.

- Incremental candidate checkpoint now verified: full168 compiler tests, reallarge
  levelsL1/L2/L3, direct+kernel reloads,114packagefmt andalltargetClippy pass.
  Benchmarkstylecollapsible-if fixedwithoutbehaviorchange;perfexamplebuild passes.
- reload_bench optional--single-file retainsallfillersacrosssaves, explicitprivate
  SDKpaths andrawJSON output; actual3edit smoke passesalllevels+unload. Private
  archivedbaselineapphost lackedexecutablemode; firstlaunchfailedbeforeedits. Fresh
  runnablecopyrestoresmodeonly,all46filecontentsbyteequal,oldarchiveuntouched.
- Guarded5000line/20kentity comparison acceptsR1baseline+candidate andR2candidate;
  finalR2baseline rejectedforunrelatedcargo. All4processescorrect,failedtimingnot
  accepted. R1actualsave-visiblemedians L1 127.56->115.34ms,L2 141.31->123.56ms,
  L3 222.68->204.10ms; allp95lowerinthislimitedpair. Parse/classify~9-10ms->~1ms.
  Eighteditsperlevel,p95=max8,onecompleteorderonly,notastabletail/Unityclaim.
  Publicpacket compiler_incremental_parse_mac_2026-11-09.json;rawcapturessaved.
- Remainingbottleneck: analyzer/compiler validation dominates measuredL1serverwork.
  Inspect precisebody-scopedanalysisanddiagnosticcoverage beforechanginganything;
  do not weakenallowlist/state/determinism/AOT rules orclearunchangederrors.


### Whole-module analyzer coverage prerequisite (fix under final gates)

- Incremental parsing/benchmark slice committed and pushed4591ef7bf. Source clean
  before starting this next correctness prerequisite for an ordinary-method span prototype.
- Review found C#14 field-backed property edits can create new state diagnostics at
  another declaration, so accessors/properties cannot use a method-only span fastpath.
  No scoped analyzer optimization has been enabled in production.
- Actual pre-fix reproduction: partial A gains :Behaviour, unchanged B has a field
  without[Transient]. Existing server incorrectly sendsReload instead ofARA0101.
  Red log compiler-analysis-coverage-20261109/repro-before.log retained.
- Expanding all-user-tree Analyze fixed base/Simulation/overload cases but stillmissed
  partial-property backing fields: per-tree Roslyn requests omitted ARA0100 that a
  full GetAnalyzerDiagnosticsAsync request found. Actual symbol/full-project diagnosis
  retained in property-diagnosis.log. No StateAnalyzer rule relaxation/change.
- IAnalysis.AnalyzeModule defaults to full-tree custom-provider fallback; real
  AnalyzerSet uses existing full compilation AnalyzeProject. All image builds validate
  exact start.Trees, including committed-code rebuilds. Additive/nonordinary method/
  doc/directive/reference changes use module analysis; ordinarymethod L1 still uses
  existing full-changed-tree analysis. No span narrowing yet, no cached trust assumed.
- Four focused regression cases nowpass: Behaviourbasefield,Simulationattribute with
  untouched Stopwatch body, added overload causing dynamic dispatch in untouched
  caller, C#14partialproperty field storage. Preserve last-good and verify repairs.
  Full172compiler suite passes and fresh solution build haszero warnings/errors.
  Runtime levels/modules/nondisruption/lifecycle gates running, thencommit+push.
- One array.Length compile correction and a stale no-build focused attempt after
  failed build retained; stale result is not accepted. Matching rebuilt tests pass.
- Batch_queries owns only target/compiler-body-analysis-prototype-20261109/source
  preparation: clean baseline and full-vs-method-span differential/timing plan;
  no prototype build/run grant yet. Reviewer found ordinarymethod-only seam plausible
  with unchanged declarations/refs/no directives, but requires differential proof and
  full-module clean provenance. Target only; keep production checks conservative.

- Coverage fix final gates pass: fresh0warning/error solution build, full172 compiler
  tests andnine actualruntime tests acrosslevels/modules/nondisruption/compile+project
  lifecycle. Reference-only validation nowalsochoosesfullmodule; afterthat small
  changefull172andthetwo module/cascade runtimecases passagain. Reviewer foundno
  mustfix; AnalyzerSetsummary updated. No productionsemanticspan fastpath enabled.
- Body-analysis prototype source nowfrozen in target/compiler-body-analysis-prototype-
  20261109 (fourfilesplusplan), privateSDKonly, globallycleanbaseline, eight exact
  diagnostic differentials plusineligiblepropertycounterexample. Unbuilt/unrun; Root
  willgrant isolatedqualityafterthisverifiedcorrectnesssliceispushed.


### Active after7ddac7ecb checkpoint

- Coveragefix committed+pushed7ddac7ecb. Root treecleanbeforethisledgerupdate.
- Targetbodyanalysisquality initiallyfailed unqualifiedEnvironment (Ara/System); Root
  qualifiedSystem.Environment, preservedsource/logs. Nextfailure exposedduplicate
  cacheddiagnostics fromsuccessive CWA semanticspanrequests. Filtereachrequesttoits
  disjointfullmethodspan; exactfull-vs-scopedmultisetassertnotweakened. All8differentials
  +ineligiblepropertycounterexample nowpass in quality-scoped-filter-512.json.
  No productionnarrowing/no timings. Batch_queries owns target-onlyfinite24cell/
  288calltiming implementation SOURCEpreparation, NO CPU/build/run grant yet.
- CurrentUnitysource-onlyplan refreshed in feature-unity-recovery-current-20261109-
  140642Z. TenC#numeric/controlhashes matchold; ufipreparedextended-v1controls fresh,
  ufxandoldcacheunchanged, recoveryregistryclear. Rootgrantedecs_research CPU for
  ONLYONEcold64 bootstrap serial1003/5ticksmoke diagnostic. No acceptedtimings,
  noBevy/helperbuild/secondUnity/reprepare/manualglobalchanges/retry/recovery.
  Stoponfailure,verifydurablecheckpoint+normalfinallyreceipt,returnCPU. Root IOonly.

- Unity cold64 completed without crash. First-import metadata/settings delta caused
  expected rejection; exact normal restoration receipt passed, editor exited and
  recovery registry cleared. Root reviewed only33added metadata/settings files and
  ProjectVersion change, C#kernels unchanged; granted verified reprepare and two
  serial1003/5tick full-state pairs64/4, baseline-only, no accepted throughput.
  Current64 pair nowpasses both engines+independent scalar oracle and receipt.
  Length4 runsnext. Current verifier relinks BLAKE3 helper onceperverify; Root
  authorized two tiny helperlinks outsidecase, no engine build orsourceadapter.
- Version correction: direct oldSIGBUS checkpoint and current cold64 both record
  Unity6000.4.5f1 and identical package cohorts (Burst1.8.29, Entities/Collections6.4.0,
  Maths1.3.3). Root initially inferred .3.27f2 from unrelated observedVBCS; retract
  that inference. Checkpoint facts rule; no cache-only cause attribution eitherway.

- Fresh serial Unity/Bevy baseline fixed64 andfixed4 pairs complete. Bothcurrent
  full-state/ID/component/update/buffer dumps match independent originalscalar
  references at1003entities/5steps, exact current input/nativebuild bindings and
  normalrestorationreceipts. Guardclear/Editorclosed,ufx/cache/registryunchanged.
  No acceptedthroughput; no 100k retry orcachecause claim. Public correctness
  packet buffer_fresh_smokes_mac_2026-11-09.json retains88rawproof hashes/limits.
- Bodyrequesttiming sources frozen thenRoot requested guardonlychanges before
  execution: minimal nonsensitiveenvironment snapshot, stablepower withbattery
  allowed, unchangedcumulativeidleVBCS exception,100mssampling. Unbuilt/unrun.

### Continuing after 870d5b895

- Fetched origin/main again: HEAD includes all current main commits (55 ahead,
  zero behind). No integration changes required; origin unchanged.
- Body request timing attempt timing-01 rejected before the first cell by an
  unrelated active ara-editor PID 76926. Zero accepted timings. Subsequent CPU
  snapshots show cumulative CPU advancing; no blind retry, process interference
  or resource guard relaxation. Existing frozen prototype remains untouched.
- Extended quality packet target/compiler-body-analysis-quality-extended-20261109
  built cleanly and passed nine exact diagnostic differentials (including two
  simultaneously edited partial files) plus property and existing-warning
  counterexamples. Existing-warning case proves globally clean provenance must
  include warnings: method narrowing otherwise drops an unedited-method warning.
- Investigating precise process-managed analyzer request allocations in a separate
  versioned target copy while editor remains active. Finite four process plan:
  4096 methods, allowed edit, full/scoped then scoped/full, 3 warm +12 measured
  calls each. All exact diagnostics required. Elapsed times explicitly unaccepted;
  factory/model/preparation excluded from this allocation measurement.
- Batch_queries owns NEW target/compiler-body-analysis-provenance-20261109 only,
  source design of immutable baseline trust + conservative fallback, no CPU.
  Ecs_research owns fixed4 target plan controller/three-arm validator source only,
  no runs/builds/Unity grant. Root owns sole CPU slot.
- Production body analysis implemented with immutable default-false AnalyzerClean on
  ModuleState and exact built-in raw emptiness, adopted only at validated atomic
  Commit. Unknown/custom providers stay full-tree; declarations/accessors/docs/
  directives/generated/reference changes retain full-module validation. Warnings
  revoke trust, changed-tree emptiness cannot create it, rebind clears it, published
  images never import it. Exact no-op preserves state identity and existing trust.
- Independent review caught a real Roslyn failure channel: tree/span APIs omit
  nonlocal AD0001. AnalyzeBodies now captures onAnalyzerException into a per-request
  queue and appends raw failures. Actual throwing-analyzer test passes. No analyzer
  rule, math flag, EnC capability, ABI or persistent manifest relaxation.
- Actual production entry-point allocation experiment completed four fresh processes
  full/scoped then scoped/full,4096 querymethods over2partialfiles (2048 in the
  changed file),one method edit,3warm+12measuredcalls each. Medians52.89/52.09MB full
  versus9.13/9.13MB scoped; raw diagnostics match every call. No accepted timing,
  peak/RSS/wholecompile/startplay/Unity claim. Measured SDK predates only final
  ModuleSession identity fix; cached method audit confirms only Commit/Compile differ,
  AnalyzerSet methods/signatures/locals/EH unchanged from checked final SDK.
- Final gates pass: solution0warnings/errors,186compiler tests,15actualCLR tests
  across7suites,all-target ara-scriptClippy -D warnings,fmt114packages. Retained
  failed local builds, a stale no-build test attempt, real no-op identity failure
  (fixed) and existing editor allocation0/656 failure (same binary isolated+full
  repeat passed, no tolerance/editor changes). Independent re-review has no mustfix.
- Fresh fixed4 controller/three-arm source review passes;22mocked mismatch contracts
  nowpass. No engine/helper/timing execution grant yet; foreignUnity jobs active.
- Allocation_pipeline_review nowowns NEW target/compiler-source-read-reuse-20261109
  sourceonly prototype for exact streaming source comparison/reuse; NO product
  changes/build/run/CPU grant. Source byte reads remain mandatory, no mtime trust.
