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
- Batch C# span API worker added: owns .NET generator/query docs+new tests, no Rust system edits. Existing[Query] may accept Span<Component>/ReadOnlySpan<Component>, one callback per native run, same descriptors/ABI; RNG ambiguity rejected. Source/tests ready; SDK checks later passed (see checkpoint).
- Scene worker source/tests prepared: decoded scene ownership and moved records/UI, explicit take_workspace, retained authoring mirror/retry bytes/CBOR unchanged. CPU probe10k/100k and player check pending.
- W2/W4 baseline6pairs R1 pass numeric+livecount+flags/restoration. Ara/Unityp50:20kboids7.106/7.763ms serial,.747/.737parallel;50k30.814/36.781serial,2.754/3.003parallel;W4.609/2.272serial,.817/10.513parallel. W4 includes different command representations/bookkeeping; not API-identical10x claim. Stable IDs not exported/asserted in legacy dump, disclosed.
- R2 paused twice by actual unrelated t3-4359603b Cargo/clippy/editor-test linking. No foreign job killed or paused. External PIDs exited; remaining R2 remains held while source correctness gates run.
- User answered continue on battery; no more power requests/global changes. Record per-process conditions, continue while safely executable; no invented AC prerequisite. User explicitly reconfirmed continue on battery. Later AC charging observed without any action/settings change by us; keep power profiles separate.
- All workers held heavy commands during the completed baseline. Current source-gate queue is sparse then batch then scene/player; timing stays held until root release.

### Harness checkpoint
- Feature inventory committed/pushed f08396d37. W2/W4 harness and R1 evidence independently reviewed: no must-fix findings; all12 retained raw hashes checked. Clarified clustered dirty selection, W4 logical-slot ordering, and parallel-only Rayon probe. Partial R1 remains battery-only; current machine reports AC charging, so future runs must identify the changed power profile.
- Private SDK generator28 and full compiler124 tests pass, audit enabled; matching Host/CompileServer fixtures built. Independent batch API review found no must-fix, Rust execution still queued.
- Cargo slot now sparse correctness, then batch integration, then scene/player gates. All Unity timing remains held. Sparse summary ticks are prohibited by Bevy descriptor invariants; table summary stamps retained and sparse per-entity Changed coverage added.
- New query_filter real-engine fixtures are source-only work; contracts/oracle are not claimed as completed Unity/Bevy coverage.

### Source correctness checkpoint
- Sparse implementation:17lib tests, actual20k-row CLR sparse fixture and all-target ara-script clippy pass. Independent unsafe/storage/order review: no must-fix. Dense no-regression measurement pending.
- Batch API:29generator tests including JIT/AOT entry generation,124compiler tests,2actual CLR integration tests and focused clippy pass. NativeAOT execution is not established by generator compilation. Optimized benchmark built; timing pending.
- Scene transfer:8ara-play tests including3new transfer/restore/replay cases and all-target clippy pass. Independent ownership/caller/benchmark review: no must-fix. Player check currently fills uncached Mac/Vulkan dependencies, followed by optimized CPU probe.

### Scene transfer measured checkpoint
- Two reversed fresh-process rounds at10k/100k, AC profile; all resource guards passed. Initial/changed/portable/changed-portable hashes agree across borrowed/owned at each size. Full source mirror, live handles and restore checks pass.
-100k workspace mean47.1–48.9ms→11.8–12.1ms; total p50 baseline1331.9/1412.6ms vsowned1321.2/1328.4ms. Projection remains~1.1–1.2s and dominates; do not promote workspace gain as whole-Play gain. Rust allocations39,828,431→37,703,392 perpreparation.
- Actual player cargo check and all-target clippy passed (unchanged vendored C++ warnings only);109-package ara-dev formatting passed. Evidence scene_prepare_mac_2026-11-09.json.
- Root rebuilt benchmark without first preserving legacya1f12binary. Historical R1 samples/source hashes remain intact, but original R2 cannot run that exact binary. Reconstructed20e8source baseline isa1892e… (different bytes), candidate7a42c8…; restored sources verified. Future comparative runs use a new profile and content-addressed executable snapshots, never call reconstruction the original binary.

### Sparse native checkpoint
- Actual nativeSparseSet binding no longer refused; checked-layout proof retained, exact membership/coalescing/row-balanced packets and serial command order verified.17lib tests+20kactualCLRfixture+all-targetclippy+independentunsafe review passed; formatted.
- Two reversed AC rounds:50k selected aligned~0.449/0.454ms, interleaved100klive/50kselected~0.880/0.873ms. These are new capability numbers, not a before/after speedup.
- Dense holdouts completed across guarded quiet windows using immutable binaries. All8valid state/tick results agree; interrupted reverse attempt retained/excluded. Coldp50+0.6–0.7%, hotp50-1.8–3.3%; reversecoldp95+22%(.320→.391ms) remains unresolved tail variability. Do not claim zero regression or universal superiority.
- Follow-up justified by measured sparse setup: streaming per-packet gather proposal can remove O(selectedrows) pointer/call metadata and parallelize it; preserve callback boundaries/state/ticks/order. Not yet implemented.
- Projection allocation follow-up source ready/reviewed: reusableValidator paths, owned-ID transfer, clone-once materialization. No compile or new timing yet.

### Projection allocation checkpoint
- Full tests and all-targetclippy for ara-schema/ara-doc/ara-sim/ara-play pass; independent source review no mustfix. BorrowedCow roots/one recursivepathbuffer preserve exact23nesteddiagnostics; ownedIDserde/parser reuse strings; materialization clonesone tree then fillsdefaults/normalizes in place, preservingunknowns/order/floatbits.
- CPU sample (excludedfromtimings) found validation path formatting in resolve; optimized actual100kprep then measured in2fresh reversed ACrounds. Valid p50 before1213.3/1239.8ms→985.4/998.6ms,~18.8–19.5%less. Rustallocationrequests37,703,392→27,878,359 and cumulativebytes2,345,064,586→1,856,286,837. NotpeakRAM/wholePlay/Unityclaim.
- All initial/changed/portable hashes match at10k/100k, livehandles/source mirrors/restore pass. Compiler-overlap attempt retained/excluded. Allocation-only separate run agrees; latency inthatprofile intentionallyunaccepted.
- BatchC#9validforward cases show smallscalarspanchanges and useful explicitSIMD gains; reverseandremainingparallelhot pending, interruptedcase retained. Runtime/source generationverified; noNativeAOTexecutionclaim.
- Real query_filter harness provenance reviews: fullinputmanifests fixed; root also binds resultJSONbytes tolaunchhash and6Pythoncontracts pass. Actualcompilation/oracle smoke pending; correctness lane will remain distinctfromacceptedtimings underload.

### Batch query checkpoint
-24valid fresh-process cases now complete in2reversed AC rounds, plus1interruptedattempt retained/excluded. Samefrozenbinary, all1,000,003rows bitvalidated; privateCLR10.0.14 tieredcompilationoff.
- Scalar spans alone give small/mixed changes; explicitVector<float>cold14workers+caller~.100ms vsrow.143/.151ms (~30–34%less); cold1worker+caller~.174/.179ms vsrow.545/.559ms (~68%less). Poolworker1 is not serial. HotDebug results separate. NotUnity/NativeAOT/framecomparison.
-29generator/124compiler/2actualCLR integration+focusedclippy+independentAPIreview pass againstcommittedd05677f2c runtime. Code/bench/docsreadytocommit; streamingruntime remainsaseparateuncommittedpacket.

### Real query-filter fixture checkpoint
- Actual serial1/parallel15 correctness smokes: retainedUnityEntityQuery/Burstjobs versusnativeBevy0.20.0 retainedqueries, all36132oraclebytesagree (1003IDs,64archetypes,1255updates). Full ID/tag/value/update audits, inputmanifests andBurst/preferencesrestoration pass. Smokes neverpromotedtotimings, evenwhenquiet.
- Newstandalonelock alignedall10Bevy0.20familyentries toroot0.20.0; setup-only0.20.1binaryretainedunused. Python10contracts, ownRustfmt/releaseClippyalltargets pass. Build/executable/source/lock/actualpackageproofsavedinquery_filter_smoke_mac_2026-11-09.json.
- Bothprovenancefindingsfixed: completeprepared/templateinventories before/afterlaunch; exactresultbytesboundtolaunchhash. Firstimportmetadata/bootstraprunsunaccepteduntilreprepare. NativefixturecoverageisnotpublicAraC#buffer/jobgraphavailability.
- Remaining5caseimplementationplanapprovednextafterthischeckpoint; source-onlywhileothergates/timingsactive.
- Streamingfallback23tests/clippy pass;50kfragmentedretainedscratch8832B at4workers/16packets. Newoptimizedbinariesbuilt, before/afterlatencyqueued.
- Local-scene-op source ready: cachedrecordcontributions withmembership/hierarchy/live-mappingguards, commonpreflight/publication andglobalmappingcleanup; fallbackfullforeveryuncertainty. Independentreview/tests/measurementpending.

### Streaming sparse execution checkpoint
-20lib+2batch+1realCLR sparse tests, alltargetclippy, formatting andindependentunsafe review pass. Separateprivate sparsemodule; densebody earlydispatch. CallbackID/len/packetorder/ticks/resource/fault/disabled/churn/undo semantics retained.
-2fresh reversed ACrounds, all16sparse+denseprocessguards pass.50kselected alignedp50.444/.452→.125/.130ms (~71%less); interleaved100klive/50kselected.861/.859→.325/.413ms (~52–62%less). Allselectedpositions/unselectedpose/ticks checks pass; variednativeintegrationfixtures separate.
- Untimed50kfragmentedmetadata8832B (4workers,16packets,64pendingIDcapacity,zeroChunkCall retention). Excludesworld/schema/taskallocations andnotuniversalRSSgain; alignedvectorscanretainlargercapacity.
- Densefollowup p50 largelyunchanged; allstatebits agree and73allocations/tick unchanged. Reversehotp95+.022ms/+3.2%, forwardimproves; retainvariability, noall-tailclaim. Evidence sparse_streaming_mac_2026-11-09.json.
- Localprojectionreview identifieddoubleinstanceexpansion; fixingrecordvalidation+singlecontributionresolution and addinglargeinstance/broadwaveprobesbeforegates. No correctness/rollbackfinding.

### Native fixture fairness/calibration checkpoint
- OptimizedS8fixtures nowretainQueryStateplans outsideframes; productionAraalreadyretainedthem, so thisisbenchmarkmodelcorrection, notengineFPSgain. Independentbaseline mathkept.
- Exact12-stepvalidation1003rowsinclSIMDtails and100kW4passes;20full freshprocessruns in2reversedACrounds have identicalnumericstatehashesperworkload. W4candidateassertsactualIDepochs/presence/uniqueslots.
- Newreconstructedbaselinea1892e differsfromhistoricala1f12; newprofileonly. W3batch8192.315/.320ms vs4096.433/.428ms; configstudyonly. W2batch512.774/.775ms loses to128.712/.713ms, so noadoption/hiding. Othercachedplanresultsretainedmixed/noise; W4kernelunchanged.
- Evidence ecs_fixture_cache_mac_2026-11-09.json; benchclippyandvalidationpassed. No newUnitytimingclaim.

### Local projection measured checkpoint
- Seven focused contracts compare the local path with real full-resolution edits, including nested/remapped prefabs, runtime divergence, inverse ops, typed rejection/stale preimage rollback, external writes, dead/recycled handles, fallbacks and queued snapshot/replay. 41 Sim/Play tests and all-target clippy pass; independent re-review found no must-fix after duplicate instance expansion was removed.
- Frozen paired builds restore candidate source exactly. Two reversed fresh-process rounds at 100k rows: one edit 215/226ms -> .844/.900ms; 1% 223/229ms -> 7.65/7.60ms; 10% 271/277ms -> 75.0/76.7ms. All source/state/portable hashes agree, all authored handles and preexisting runtime values retained.
- Large-instance name/override remains ~146–148ms because a 100k-row contribution must resolve fully. Full-wave original after931ms/before766ms (+21.5%) retained; reverse747/774ms. Two justified full-wave follow-ups after774/776 vs before769/766ms (+.7%/+1.4%). No zero-overhead/tail/whole-editor claim. All28 process guards pass, AC observed without changing power settings. Evidence projection_local_mac_2026-11-09.json.
- Unity extended feature correctness suite has 21+ valid native/Unity oracle pairs; a Burst LLVM native compiler crash (-11) during churn100 parallel bypassed finally. An identical retry passed before pause arrived. Known five preferences match the prior successful initial state on read-only comparison. No guessed preference restoration. New feature-only durable checkpoint/recovery is source-only until reviewed; Unity launches held. Original S8 fixture remains frozen.

### Larger compiler scaling observation
- Existing release CompileServer/SDK10.0.403/CoreCLR10.0.14, all10240 files edited across16 modules of640 scripts. Six full requests, firstthree excluded; warm13.890/14.468/14.280s, peak server RSS1,934,458,880 bytes. No errors/noncompiled returned modules. Final separate audit confirms16nonempty assemblies/all10240 expected final authored revisions. Assembly/config hashes unchanged.
- Single-process observation, resource context captured after launch/after completion, no strict initial timing guard; no baseline/Unity/IL execution/complete reload claim. Earlier5120 fixture has different CLR/module shape and cannot support a pooled ratio. Evidence compiler_10240_mac_2026-11-09.json.
- Local projection checkpoint pushed0d1b593fb. Full109-package formatting check passed. Preference recovery source frozen with28mocked tests; fresh independent review active, no Unity launch or current preferences restoration performed.

### Feature crash recovery review/fix checkpoint
- Independent frozen-source review reproduced two medium failures with mocked children: pre-checkpoint wait timeout bypassed failure guard; recovery of an already normally restored checkpoint could fail while its old receipt still permitted future launches.
- Root fixed both narrowly: child wait/cleanup exceptions record failure, explicit recovery persists an atomic checkpoint-bound pending attempt before launch, C# receipts carry its fresh attempt ID and Python requires matching proof before clearing. Five new tests added; all33 mocked/source tests pass. Source/whitespace reviewed; no global state touched by mocked tests.
- Actual feature-only C# import/normal receipt/idempotent known-state recovery gate authorized next, then remaining bounded critical oracle cases. Original S8 wrapper frozen; no safety-flag weakening. Current known matching preference state requires no guessed restoration.

### Actual preference recovery gate
- New feature C# compiled; normal tiny checkpoint-before-mutation/finally receipt passed. First import metadata was correctly rejected as drift and re-prepared; fresh normal proof passed.
- Explicit recovery launch failed before executeMethod: Bee IPC socket exceeded macOS104-character limit because its child TMPDIR was too long. No recovery receipt; exact original checkpoint and pending attempt retained, all feature Unity launches blocked. No guessed restoration or guard deletion.
- Narrow fix authorized: reuse proven short scratch TMPDIR; fresh attempt-specific receipt/log paths, preserve failed attempt and require current attempt ID before clearing. Worker must freeze/tests/root source review before actual known-state retry.

### Extended feature quality packet complete
- All50 serial/parallel native Bevy/Unity pairs pass: dirty16, churn6, DAG4, fixed buffers8, resize16. Complete IDs/tags/epochs/A/B/C/update counts/lengths/words match the independent v1 integer oracle. Actual reuse/trim capacity and SmallVec spill behavior asserted, worker diagnostics separated from timing.
- Exactly one runtime-kernel source version and one immutable Bevy executable eb9ca9be… across50 pairs.22 earlier records retain pre-checkpoint controller proofs;28 resumed records have durable checkpoint/finally receipts. All timings remain unaccepted; native fixture coverage does not establish public Ara C# buffer/JobHandle availability or Unreal/player/frame superiority.
- Normal C# checkpoint gate and explicit known-state recovery gate passed with checkpoint95e50f0b… unchanged, attempt4093eb61… and matching receipt; original failed socket attempt1d5e… retained. Registry-wide guard clear, no Editor remains. Earlier Burst native crash and socket failure excluded, raw logs under target with published hashes.
- Source/recovery checks38 pass, standalone release build/Clippy/formatting pass, independent runtime and recovery reviews addressed. Root independently validated593 referenced file hashes,50 unique pairs, every complete engine state byte/hash and unchanged current Rust inputs against build proof. Evidence benchmarks/iteration/unity/features/extended_smoke_mac_2026-11-09.json.
- This bounded packet is complete. Further work is explicit: matched timings for these new cases, complete save-to-first-correct-tick/asset readiness, NativeAOT execution, integrated C# change/enable/shared/buffer/job surfaces and matched subsystem/Unreal lanes. No universal win or claim of exhausted optimization opportunities.
