# Measured wins over Unity continuation

Mode: work-packet. Branch t3/6f34c966; starts fdc447ee0.
Objective: continue implementation and matched timings until the representative suite beats Unity by a considerable margin, rather than stopping at correctness. Target: >=20% lower Ara process p50, no material p95 regression, equal algorithms/state/quality and declared worker ceiling. Every failing or uncertain row remains open. Never certify all arbitrary workloads/hardware from a finite suite.

Constraints: Mac only, Windows untouched; user authorized continuing on battery, no further power questions/changes. Current observed AC100%, 60% memory free, 200GiB disk free; no competing compilers/Unity. Cargo jobs2, nice10; sequential CPU/build/timing slot. Preserve D11/A12/A13, ECS authority, fixed semantics and full output gates. Commit and push each verified slice immediately.

Ownership:
- root: ara-sim native kernels/benchmarks, production runtime changes and integration/evidence.
- ecs_research: benchmarks/iteration/unity/ control harness/evidence only. Source-only until root CPU grant. Original S8 C#/MacBatch frozen, use explicit new guarded fixture family for fresh comparisons; no marker spoofing.

## Slices

### W1/W2/W3/W4 matched performance
Status: implementing
Acceptance: fresh reversed engine-order rounds at serial/parallel workloads/counts with exact output gates and declared margins. Original baseline/evidence preserved. Benchmark-only speedups must be labeled separately from production gains.
Attempts: existing cached executable W2 phase profile; next native SIMD neighbour screening and dispatch tuning, then matched Unity execution via durable recovery controls.
Blockers: none currently.

### Feature timings beyond independent rows
Status: planned
Acceptance: strict timed query/change/structural/dependency/buffer lanes after the completed50 independent-oracle correctness pairs. Any observed losses drive implementation, not a completion claim.

### Actual C# and data/runtime integration
Status: planned
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
- Fresh direct actualCLRvsUnity comparisons underway, not extrapolated fromnative Rust. Remain active until all measured gaps addressed; do not stop at this checkpoint.
