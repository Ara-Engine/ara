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
