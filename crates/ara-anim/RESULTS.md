# Animation v2 results — 2026-10-23

Windows 11, Ryzen 7 7800X3D, RTX 4070 SUPER, Rust 1.99.1, release profile,
`CARGO_BUILD_JOBS=2`. Other agents shared the CPU. Animation crowds use eight
Rayon workers and chunks of 16; powered physics uses eight Jolt workers.

The recorded matrix is [benchmarks/v2-results.txt](benchmarks/v2-results.txt).
Three sequential A/B rounds interleave the preserved original crowd binary,
current crowd, pre/post inertialization implementations and pre/post descendant-FK
solvers. Each crowd/search run has 100 measured frames after warm-up. Procedural
runs have 35 measured batches of 200 solves. Numbers below are the median of the
three per-run p50s and the median of the three per-run p95s, not pooled percentiles.
No compiler from this task ran during that matrix. Shared-machine contention still
produced visible tails; the raw rounds are retained.

## Motion search and crowd

The synthetic eight-clip set contains walk, run, left/right strafe, left/right
turns, back and idle: 1,920 sampled frames, 27 feature dimensions, 215,040 bytes
of padded SIMD feature payload (210 KiB). This is a reproducible functional and
performance fixture, not a production mocap dataset or a visual-quality comparison.

- Exact exhaustive SIMD search: **9.407 µs p50 / 13.909 µs p95 per character**.
  Per-run p50 range 9.220–9.927 µs; p95 range 13.833–14.256 µs. Meets the 20 µs
  target for this database. This timer searches already-normalized query vectors.
- Explicit 512-candidate budget: **2.625 / 4.420 µs**. The result reports
  `exact=false`; this is a bounded approximation, not an exact-search claim.
- Exact search for 1,000 queries on eight workers: **1.537 / 2.231 ms**.
- Existing 1,000 × 65-bone animation + palette workload: original
  **1.155 / 1.307 ms**, current **1.161 / 1.400 ms**. Compression remains 11.932x
  and resident test-clip bytes remain 45,059. No baseline speedup is claimed.
- Motion-controller crowd, 1,000 × 65 bones, exact search every tick, including
  normalization, sampling, inertialization, snapshot copying and palettes:
  **2.550 / 3.064 ms**. Per-run p50 range 2.222–2.810 ms. This is within the
  measured existing workload plus the measured parallel search cost
  (p50: 1.161 + 1.537 = 2.698 ms).
- The controller crowd uses preassigned query parameters. Feature extraction,
  trajectory prediction, contact IK, physics and GPU skinning are outside that
  crowd timer. The separate scripted demo exercises predictor → pose/trajectory
  features → typed parameters → graph node for 600 fixed ticks. Its repeatable
  root-distance checksum is 4.701455 m; this is a checksum, not path-following error.
- Pre/post inertialization crowd medians: 2.757 → 2.550 ms p50. One paired round
  regressed under contention; do not treat this small difference as an isolated
  proven speedup. The implementation shares the decay calculation and skips zero
  offsets; all pose/replay/allocation checks pass.

## Procedural solvers and retargeting

These timers include resetting/reading 65-bone poses and solver FK. Individual
chain targets use the default 24-iteration cap. Full-body uses three goals and
four ordered sweeps; ground placement uses two feet and a deterministic flat query.

- FABRIK: **5.393 / 6.843 µs**, tip residual 0.1440 mm. Before descendant-only FK:
  24.437 / 33.353 µs p50/p95. Median p50 improvement about **4.5x**.
- CCD: **4.456 / 5.274 µs**, tip residual 0.1864 mm. Before: 23.129 / 26.580 µs.
  Median p50 improvement about **5.2x**.
- Two-foot placement: **6.168 / 8.425 µs**, excluding native scene-query traversal.
- Full-body, three contacts: **52.924 / 65.279 µs**. Before: 284.389 / 322.165 µs.
  Median p50 improvement about **5.4x**. Ordered projections expose residuals;
  this is not a global constrained-optimization guarantee.
- Runtime retarget, 65 bones, 1.3x rig, IK endpoint correction:
  **18.618 / 28.906 µs**. Bind-foot height residual 0.00004 mm in this fixture.
- Moving-pelvis planted-foot test, 60 poses, 1.4x target proportions:
  **maximum positional error 0.000348 mm**. This idealized analytic fixture is
  tested against a 0.2 mm threshold; it does not establish real-mocap accuracy.
  Offline baking also produces a valid re-compressed clip in the source timebase.

The substantive hot-path optimization preserves model transforms outside the
rotated joint's precomputed descendant list. The existing deterministic replay,
length, convergence and angular-limit tests pass after the change.

## Physical animation and determinism

- 32 powered ragdolls × seven bodies, eight Jolt workers, drive + fixed physics
  step + post-physics pose blend: **0.174 ms p50 / 0.206 ms p95**, 160 measured
  ticks after 20 warm-up ticks. This overhead fixture has no ground/contact scene;
  contact/death/get-up behavior is tested separately and captured visually.
- Actual Jolt hit → death/ragdoll → get-up runs reproduce all body poses,
  velocities, blend state and output poses bit for bit at **1, 4 and 8 workers**.
  Both same-world and structurally identical fresh-world snapshot restore pass.
- Existing Jolt cross-process/thread-count/rollback and native ABI tests pass
  with the additive closest-ray helper.
- Allocation instrumentation records **zero Rust allocations, reallocations or
  deallocations** in warmed chain/full-body/retarget/motion-node/snapshot loops
  and Jolt probe/drive/step/blend loops. Setup, cooking and snapshot construction
  allocate outside ticks. The allocator test does not count C++ heap allocations;
  the new native closest-ray collector itself uses one stack-owned result.
- Drives use bounded PD force/torque commands with real native constraints.
  Native constraint-motor setters are absent from the existing backend API.
  Hinge constraints map directly; cone profiles use conservative six-DOF swing
  intervals. See [V2.md](V2.md) for collision policy and get-up root reconciliation.

## Authoring, captures and gates

The four canonical documents in [examples/documents](examples/documents) cover
constrained IK, source/target rig maps, a complete motion database/controller with
source clips, and a powered-ragdoll profile. They are generated exclusively through
typed `AnimationCommand` intents, attributed kernel transactions and the canonical
writer. A focused test reads them byte-exactly and cooks every setup, including an
executable motion controller. Text example clips are decimated for readability;
the timed synthetic set retains every 60 Hz key.

[The marked Vulkan pose strip](captures/v2/pose-strip.marks.png) was visually
inspected: walk, hand IK, proportion retargeting, selected locomotion and real-Jolt
collapse/recovery are visible. All 30 pose entities have visible marks. Capture
JSON records the device/driver, shader cohort, fixed request and scene hash.
Vulkan validation completed cleanly. This is bone geometry, not a skinned-art demo.

Passed:

```text
cargo test -p ara-anim -p ara-anim-physics -p ara-anim-capture
cargo test -p ara-jolt-sys -p ara-physics-jolt
cargo clippy -p ara-anim -p ara-anim-physics -p ara-anim-capture -p ara-jolt-sys -p ara-physics-jolt --all-targets -- -D warnings
cargo fmt -p ara-anim -p ara-anim-physics -p ara-anim-capture -p ara-jolt-sys -p ara-physics-jolt -- --check
```

Runtime ownership and checkpoint integration are documented in [V2.md](V2.md).
No ara-sim/runtime/player files or root manifest are changed. New crates are picked
up by the existing workspace glob; the parent must refresh Cargo.lock when integrating
them because root-file ownership was reserved by the task.

## Published comparison and limits

Epic documents normalized pose/trajectory schemas, brute-force search and optimized
PCA KD-tree search with full candidate rescoring. Ara's delivered option is exact
SIMD brute force plus an explicit candidate budget; there is no claim that the
small fixture characterizes large production Pose Search databases.
[Epic Motion Matching documentation](https://dev.epicgames.com/documentation/en-us/unreal-engine/motion-matching-in-unreal-engine).

A published first-hand UE 5.7 Unreal Insights analysis reports approximately
**0.25 ms/character for GASP locomotion selection**, including pose search,
trajectory, choosers and BlendStack, and 0.50 ms/character for worker evaluation.
It uses different assets, workload and unspecified hardware, and measures a broader
pipeline; these are contextual published numbers, not a valid Ara speedup ratio.
[Subressor, May 10 2026](https://forums.unrealengine.com/t/state-machine-vs-motion-matching-demo-performance-analysis-share/2721152).

Unity's Kinematica manual describes near-constant lookup and lists the 0.8 preview
package. It supplies no per-character timing in that page. No equivalent vendor
microbenchmark was found in the cited vendor material, so no Kinematica timing or
UE/Unity fidelity superiority is asserted here.
[Unity Kinematica manual](https://docs.unity.cn/2020.3/Documentation/Manual/com.unity.kinematica.html).
