# Physics performance pass — Windows, 2026-10-23

The absolute targets and native overhead results below apply to this shared-host
run. They do not establish that Ara is faster than every engine. Machine-wide
load varied substantially; every comparison alternates A/B, and the JSON retains
the load endpoints rather than treating S6's historical 31.4 ms as today's baseline.

## Targets and before/after

Uninstrumented, 600 ticks, 8 threads, precise FP. Bowl rows are single-world
ABBA runs of the retained pre-pass executable versus the current scale executable.
Both use **15 velocity / 2 position iterations**; before uses S6's 64 MiB arena,
after uses `game_default`'s 256 MiB arena. Listener collection is off throughout.
Ranges are separate run p95s, not a pooled percentile. Outpost alternates the
original Jolt character scan and Ara's tree every tick with identical settings.

| Workload | Before p95 ms | After p95 ms | Target ms | Remaining gap |
|---|---:|---:|---:|---|
| 5k bowl | 5.806–5.869 | 5.959–6.124 | 4 | +1.959–2.124 ms, 49.0–53.1%; no improvement established |
| 20k bowl | 34.419–35.624 | 29.682–29.975 | 15 | +14.682–14.975 ms, 97.9–99.8% |
| 1,000 virtual characters + 2,000 props | 94.350 | 7.038 | 3 | +4.038 ms, 134.6% |

The 20k single-world p95 improves about 13–17%, depending on the ABBA pair;
5k regresses slightly in this run. Outpost mean falls from **75.364 to 5.349 ms**
(14.1×), and both character paths finish with identical state. This is 1,000
moving capsules on a flat ground with 2,000 falling props, not an Outpost game
capture with animation, navigation and rendering. None of the three absolute
targets is met. Solver changes are not credited as same-workload improvements.

The native comparison below puts almost all bowl update cost inside Jolt, with
mean boundary deltas below 3%. Removing that boundary cannot close a roughly
15 ms 20k gap. The velocity solver dominates the instrumented worker scopes;
the arena removes one large temporary fallback per tick, but leaves the contact
and solver workload. Even the explicitly lower-quality `stress` preset misses
the p95 targets. These numbers do not support the brief's fastest-engine claim.

One independent `game_default` thread sweep records:

| Bodies | 1 thread p50 / p95 ms | 4 threads p50 / p95 ms | 8 threads p50 / p95 ms |
|---|---:|---:|---:|
| 5k | 18.582 / 27.201 | 6.518 / 8.415 | 5.040 / 6.143 |
| 20k | 106.602 / 133.538 | 33.360 / 40.369 | 23.834 / 29.041 |

## Matched native update boundary

Two 600-tick runs per row, 8 threads, listeners off. Means are averages of the two
run means; p95 columns give the range of the two independent p95s. Every pair uses
identical settings and finishes with bit-identical bodies and zero escapes.

| FP build | Preset | Bodies | Ara mean ms | Direct Jolt mean ms | Ara p95 range ms | Mean delta |
|---|---|---:|---:|---:|---:|---:|
| Precise | S6, 15/2 | 5k | 4.649 | 4.710 | 6.191–6.370 | −1.31% |
| Precise | S6, 15/2 | 20k | 27.646 | 27.660 | 34.612–36.171 | −0.05% |
| Precise | game_default, 15/2 | 5k | 5.059 | 5.036 | 6.119–7.252 | +0.45% |
| Precise | game_default, 15/2 | 20k | 27.306 | 27.331 | 32.459–37.419 | −0.09% |
| Precise | stress, 8/2 | 5k | 3.024 | 3.015 | 5.382–5.681 | +0.31% |
| Precise | stress, 8/2 | 20k | 24.345 | 24.818 | 30.475–31.844 | −1.91% |
| Precise | deterministic_network, 15/2 | 5k | 4.738 | 4.800 | 5.853–6.347 | −1.30% |
| Precise | deterministic_network, 15/2 | 20k | 26.069 | 25.687 | 31.267–39.113 | +1.49% |
| Local fast | S6, 15/2 | 5k | 5.373 | 5.380 | 6.283–9.715 | −0.14% |
| Local fast | S6, 15/2 | 20k | 36.231 | 36.271 | 30.364–85.693 | −0.11% |
| Local fast | game_default, 15/2 | 5k | 8.682 | 8.578 | 6.993–25.624 | +1.22% |
| Local fast | game_default, 15/2 | 20k | 31.714 | 31.837 | 29.442–67.785 | −0.39% |
| Local fast | stress, 8/2 | 5k | 4.707 | 4.674 | 6.148–8.722 | +0.71% |
| Local fast | stress, 8/2 | 20k | 25.528 | 25.101 | 32.061–43.581 | +1.70% |
| Local fast | deterministic_network diagnostic, 15/2 | 5k | 4.796 | 4.845 | 5.747–7.151 | −1.01% |
| Local fast | deterministic_network diagnostic, 15/2 | 20k | 26.346 | 26.693 | 32.789–35.093 | −1.30% |

Across these 32 pairs, individual run-mean deltas are **−2.67% to +2.26%**, within
the 3% update-boundary budget in this experiment. This includes wrapper/tick work,
not ECS synchronization/extraction/writeback. It is an observed bound under these
conditions, not a universal worst-case guarantee. Endpoint CPU load spans **8–99%**;
large local-build tail spikes track changing host load and cannot be called FP cost.
The fast network diagnostic rows do not describe a supported cross-platform peer
configuration. Network and game rows share physical settings with listeners off;
their timing differences show the run-to-run contention envelope.

The eight additional same-settings pairs in the FP-only ABBA experiment bring
the uninstrumented count to **40**. Their combined run-mean range is −4.06%
to +2.26%; the maximum positive delta is unchanged. Six instrumented native
pairs are retained for phase analysis, not for the overhead budget or targets.

## Controlled tuning experiments

600 ticks per side, 8 threads, default precise FP. A/B alternates every tick.
These rows intentionally vary the named setting and are not shim-overhead tests.

| Experiment | Bodies | Ara setting mean ms | Reference setting mean ms | Delta | Final bits equal |
|---|---:|---:|---:|---:|---|
| Arena 256 vs 64 MiB, 15/2 | 5k | 5.626 | 5.564 | +1.12% | yes |
| Arena 256 vs 64 MiB, 15/2 | 20k | 24.638 | 26.304 | −6.33% | yes |
| Shared vs dedicated workers, 256 MiB, 15/2 | 5k | 4.966 | 4.898 | +1.38% | yes |
| Shared vs dedicated workers, 256 MiB, 15/2 | 20k | 27.488 | 27.193 | +1.09% | yes |
| Splitter on vs off, 256 MiB, 15/2 | 5k | 5.234 | 12.655 | −58.64% | no |
| Splitter on vs off, 256 MiB, 15/2 | 20k | 28.307 | 73.635 | −61.56% | no |
| Deterministic ordering on vs off, precise FP, 256 MiB, 15/2 | 5k | 4.776 | 3.367 | +41.86% | no |
| Deterministic ordering on vs off, precise FP, 256 MiB, 15/2 | 20k | 24.139 | 19.705 | +22.50% | no |

The 5k temporary peak is about 39.6 MB. The 20k peak with a sufficient arena is
about 157.6 MB: 64 MiB and the earlier 128 MiB sizing probe each fall back once
per tick, while 256 MiB has no fallback. The controlled 20k arena experiment
removes 600 heap fallbacks and saves 6.33% mean update time; 5k gains nothing.

Dedicated workers remain the default: their measured means are slightly lower
and they avoid Bevy scope-task allocations. The 1.1–1.4% pool difference is small
and does not establish a general winner under mixed Ara compute work. The custom
host JobSystem ships as an option on Ara's initialized ComputeTaskPool, with no
second native pool in that mode. Test the integrated runtime's concurrent workload
before changing that choice. The large-island splitter was already enabled; its
large benefit is Jolt's existing behavior, not a new Ara optimization. Changing
splitter partition/order changes final float bits, as recorded.

Runtime deterministic ordering and compiler cross-platform FP are separate costs.
The ordering experiment above keeps precise FP and changes only ordering; its
faster opt-out is not the network workload. A further whole-executable ABBA run
holds `game_default`, 15/2, 256 MiB, sleeping, splitting and ordering constant:

| Bodies | Precise FP mean ms | Local fast FP mean ms | Ara mean cost | Native mean cost |
|---|---:|---:|---:|---:|
| 5k | 5.089 | 4.635 | +9.80% | +10.53% |
| 20k | 24.275 | 23.639 | +2.69% | +1.20% |

These are averages of two run means per FP mode. The 5k precise runs themselves
span 4.382–5.796 ms, versus 4.622–4.648 ms local; endpoint CPU samples span
23–82% across this experiment. Consequently this is a noisy cost estimate, not
a universal constant or confirmation of S6's historical 7–12% on every scene.
The full matrix's larger fast-mode tail spikes are not attributed to FP flags.

## Phase breakdown

20k bowl, 600 ticks, 8 threads, precise FP, listeners off. These are **exclusive
summed worker-elapsed milliseconds per tick**, not additive wall-clock time.
The table isolates a controlled 64→256 MiB comparison and also records each
solver preset. Both the arena control and preset profiles are instrumented.
The 64 MiB control uses direct native Update and the 256 MiB control uses Ara;
preset phase columns use Ara's worker scopes, with native wall means explicitly
labeled in the final row. Neither table is a pre-pass whole-engine phase trace.

| Phase | 64 MiB control before, 15/2 | 256 MiB control after, 15/2 | S6, 15/2 | game_default, 15/2 | stress, 8/2 |
|---|---:|---:|---:|---:|---:|
| Broadphase | 24.907 | 24.868 | 24.544 | 24.687 | 25.462 |
| Narrowphase | 28.356 | 28.340 | 28.024 | 29.024 | 37.216 |
| Constraint setup | 32.122 | 22.903 | 32.885 | 22.941 | 26.706 |
| Velocity solver | 120.444 | 117.967 | 118.462 | 121.907 | 106.931 |
| Position solver | 16.099 | 15.521 | 14.953 | 14.296 | 25.098 |
| Integration / CCD | 2.174 | 2.182 | 2.100 | 2.133 | 2.223 |
| Island building | 0.224 | 0.219 | 0.225 | 0.237 | 0.241 |
| Named bookkeeping scopes | 0.089 | 0.070 | 0.080 | 0.072 | 0.083 |
| Update wall mean, separately measured | 31.253 | 29.216 | 30.892 native | 29.848 native | 30.809 native |

The controlled arena experiment drops constraint setup by 9.219 summed worker
ms/tick. The separate uninstrumented arena pair is the reliable wall improvement
(6.33%). `deterministic_network` with the listener disabled has exactly the game
profile's physical settings; no independent native network phase profile is
claimed. Subscribed network effects are costed below. Reducing velocity iterations
changes later contacts/positions too, so `stress` does not scale each phase linearly.

AoS versus direct SoA extraction in the paired `game_default` bowl, measured
outside Update with symmetric reads on both worlds, averages **0.101→0.060 ms
at 5k** and **0.779→0.416 ms at 20k**. This is extraction into reusable Rust
buffers, before ECS lookup/change detection/writeback. The wrapper/native update
delta is in the native table; it must not be added to the worker-scope table.

## ECS synchronization and writeback

The matched flat-grid ABBA control restores the three old per-row binding
lookups and the pre-linear lifecycle implementation. Both executables use the
same 600-tick tracker-maintenance harness, precise FP, 15/2, 256 MiB, 8 threads,
20k dynamic boxes and one ground. The final native-body hash is
`d3fd0412679e5d75` in all four runs. Averages of two run means:

| Wall phase | Before ms | After ms |
|---|---:|---:|
| Lifecycle/configuration synchronization | 3.195 | 1.125 |
| ECS inputs | 1.355 | 0.442 |
| Physics step | 0.559 | 0.536 |
| Native column extraction | 0.458 | 0.244 |
| ECS change-only writeback | 1.135 | 0.682 |
| Tracker maintenance | 0.0000945 | 0.0000653 |
| Complete boundary | **6.706** | **3.031** |
| Independent run p95 range | **11.907–12.426** | **8.281–8.316** |

The combined implementation reduces this boundary's mean by 54.8%. This is not
an isolated attribution to the zip walk: stable-ID lifecycle checks also became
linear, and changes to cache locality affect extraction. Endpoint CPU samples
span 39–77%. The scene mostly sleeps by the end, so p50 physics step is about
0.001–0.003 ms while early settling drives its p95. Full ECS work is clearly
larger than 3% of native Update; the 3% result is restricted to the update shim.

The actual S6 **20k bowl through ECS** is separately measured by `ecs_bowl`,
600 ticks, 8 threads, precise FP. Native Update and complete ECS boundaries
alternate order every tick, with native column reads between updates. The same
mesh/grid generator feeds both paths; final dynamic-body hashes agree in every
pair. Cooked shapes are attached to ECS entities before setup. Each preset is
a separate run, so cross-preset timing differences are not a causal A/B.

| Mean wall phase ms | S6, 15/2, 64 MiB | game_default, 15/2 | stress, 8/2 | deterministic_network, 15/2, listeners off |
|---|---:|---:|---:|---:|
| Lifecycle/configuration | 2.063 | 2.311 | 2.513 | 2.513 |
| Inputs | 0.582 | 0.652 | 0.735 | 0.773 |
| Physics step | 28.178 | 28.715 | 29.365 | 31.537 |
| SoA extraction | 0.273 | 0.285 | 0.301 | 0.340 |
| ECS writeback | 1.576 | 1.735 | 1.887 | 1.841 |
| Tracker maintenance | 0.000206 | 0.000308 | 0.000316 | 0.000279 |
| Complete ECS boundary | **32.683** | **33.709** | **34.814** | **37.017** |
| Paired native Update | **27.515** | **28.620** | **29.095** | **34.452** |
| Complete-boundary mean delta | +18.78% | +17.78% | +19.66% | +7.45% |
| Complete-boundary p95 | 42.480 | 50.756 | 47.039 | 59.248 |

The full boundary therefore **does not meet a 3% end-to-end overhead target**.
These wall phases can be summed (up to timer/bookkeeping gaps), unlike the
native parallel worker-scope table. The network row's native p95 is 65.681 ms
versus 51.251 ms for its ECS step: fluctuating host contention makes its mean
delta an unreliable universal overhead estimate. It uses the same physical
settings as game-default, with subscriptions disabled; no speedup from the
network name is claimed. Per-launch CPU/memory endpoints are in
`ecs-bowl-provenance`. The separate flat-grid ABBA experiment above establishes
the before/after synchronization improvement without conflating these scenes.

## Queries and subscriptions

Single caller, warm scene after Outpost, five samples of 100k queries:

| Batch | Mean ms | p50 ms | p95 ms | Mean throughput | Returned hits |
|---|---:|---:|---:|---:|---:|
| 100k rays | 41.802 | 40.876 | 44.585 | 2.39 million queries/s | 144,934 |
| 100k radius-0.2 m sphere casts | 110.234 | 110.062 | 112.340 | 0.907 million queries/s | 239,505 |

The batch API returns **all hits**, sorted, and includes collector/FFI conversion
and allocations. It is not a closest-hit kernel, parallel batch or GPU result.

The 5k bowl contact experiment rotates three paths each tick for 600 ticks:

| Subscription mode | Mean ms | p95 ms |
|---|---:|---:|
| Disabled, no native listener | 5.437 | 7.411 |
| Native listener only, discard effects | 5.608 | 7.511 |
| Listener plus Rust effect decoding | 7.534 | 9.809 |

All finish with identical bodies; the decoded path produces 8,136,524 effects.
Native callbacks add about 3.15% mean versus disabled. Decoding adds another
1.926 ms versus listener-only; total mean cost is about 38.6% versus disabled.
The listener-only path uses the direct harness and excludes Rust tick bookkeeping;
the small wrapper cost remains in that comparison. Network users must budget
subscriptions explicitly rather than applying the listener-free table unchanged.

## Measurement contract

- Ryzen 7 7800X3D, 8 physical / 16 logical processors, Windows 11, RTX 4070 SUPER
  unused by these CPU tests; Rust 1.99.1, MSVC through `cc`.
- `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=2`. Main `180b29b` was merged as
  `a2c6ac5`, then main `ac00048` as `67e750a` before the final gates. Runtime code used by the core matrix binaries is
  through `370b0a3`; `f0a09d2` adds the allocation test only. The final symmetric
  extraction/cache-warming harness is `e60ba7e`; `20706e4` adds run manifests,
  launch provenance and ECS tracker maintenance. `9935ecf` adds linear stable-ID
  lifecycle checks and the matched standalone ECS comparison. `fe98ef3` adds the
  complete ECS bowl harness, reusing the same bowl and seeded grid generators.
  Earlier probes, including a detected shared-Cargo-cache ECS contamination,
  are excluded.
- Audio-v2 main `e6d471d` was subsequently merged as `46570fe`. It changes no
  owned physics implementation. The full default tests, benchmark-enabled tests,
  strict default Clippy and formatting gates pass again on that integration.
- Release Rust benchmark binaries; native Jolt stays `/O2`, AVX2. Default build
  defines `JPH_CROSS_PLATFORM_DETERMINISTIC`, `/fp:precise`, no FMA; local build
  disables that feature and uses `/fp:fast` plus FMA. Runtime deterministic
  ordering is a separate setting.
- S6 bowl: 5,000 or 20,000 mixed boxes/spheres, mesh bowl radius 25 m / height
  12 m, seeded grid at Y=13 m, 50 bodies per row, seed 7; 600 fixed 1/60 s ticks.
  15 velocity / 2 position iterations unless a row explicitly says `stress` (8/2).
  Bodies retain sleeping and discrete motion unless CCD is individually flagged.
  Max bodies = count+16; pair/contact capacity = max bodies × 16.
- The scale benchmark runs one world. The native comparison keeps two identical
  worlds alive and alternates Ara/direct C++ order every tick. Both worlds receive
  identical AoS/SoA extraction outside the step timer, with alternating order, to
  avoid warming only Ara's body cache before the following Update. These different
  working sets make their absolute percentiles unsuitable for cross-table A/B.
  Two complete runs per build flavour execute in deterministic/local/local/
  deterministic order. Each run samples 600 ticks per scene/preset.
- Machine-wide `Win32_Processor.LoadPercentage`, free memory, process count and UTC
  are sampled before/after each case. These are endpoint samples, including recent
  benchmark work, not a continuous isolated measurement of other agents' CPU time.
- Percentiles use nearest rank. Tables give ranges of independent run percentiles
  or means of run means as labeled; averaging p95 values is not a pooled p95.
  Negative measured overhead denotes contention/timing variation, not negative FFI cost.
- Every same-settings native pair checks bit-exact final rigid-body state and zero
  escaped bodies. Solver/ordering/splitter experiments disclose changed settings;
  their final state is not required to match.

## Native reference and phase interpretation

`ara-jolt-sys/benches/native.cpp` is the brief's permitted tiny C++ harness,
compiled by `cc` from vendored Jolt 5.6.0 commit
`e77f175595e64cb44218cc9d9d56fc365ad0e36a`. It calls upstream
`PhysicsSystem::Update` directly on an equivalently constructed world. It bypasses
Ara's C update shim, Rust update/effect conversion and tick bookkeeping. Both sides
share shape construction, broadphase configuration, Jolt settings, the measured
TempAllocator wrapper and (unless explicitly varied) Jolt's native worker pool.
This measures update-boundary overhead; it does not measure asset cooking, ECS or
all C++ allocation wrappers against a separately configured upstream executable.

The `profile` feature instruments build-directory copies of PhysicsSystem and the
contact manager; the vendored sources remain unchanged. Counters sum exclusive
**worker elapsed scopes**, including descheduling. They are neither on-CPU time
nor additive critical-path milliseconds. Nested narrowphase/contact-setup scopes
are subtracted from their parent; parallel scopes can sum above Update wall time.
Instrumentation itself costs time, especially per-pair clocks; use uninstrumented
rows for targets and overhead. Bookkeeping covers named job construction/barrier/
removed-contact scopes; uninstrumented scheduling, waits and remaining work are
not silently attributed to a physics phase. Integration includes CCD handling.

## Presets and production choices

| Preset | Velocity / position | Temp arena | Contact effects | Trade-off |
|---|---:|---:|---|---|
| `game_default()` | 15 / 2 | 256 MiB | disabled until configured | Keeps S6 tower quality; avoids this bowl's large temporary heap fallback. |
| `stress()` | 8 / 2 | 256 MiB | disabled until configured | Reduces velocity work; the slender 20-box tower is not guaranteed. |
| `deterministic_network()` | 15 / 2 | 256 MiB | bounded 65,536 by default | Retains ordered updates/effects; peers must also retain the deterministic build feature. |
| Existing `Default` / S6 | 15 / 2 | 64 MiB | bounded 65,536 by default | Compatibility; throughput fixtures explicitly disable contact collection, as in S6. |

All presets retain 2 cm speculative contacts, 0.5 s sleeping time, 0.03 m/s point
speed threshold, large-island splitting and deterministic runtime ordering.
Sleeping can be disabled per body; CCD is enabled only by each body's CCD flag.
The 256 MiB arena is per world, not a recommendation for small scenes: size both
arena and contact capacity from measured peak/fallbacks. Reducing max contacts
also reduces Jolt's worst-case temporary constraint reservation.

The named network bowl rows disable listeners to isolate solver/FP costs, exactly
like every other bowl row; use the separate subscription experiment for the cost
of native callbacks and Rust effect conversion. Selecting the network constructor
does not override Cargo features or prove cross-platform equality.

Body insertion uses Jolt's prepare/finalize batch API; removal is prevalidated and
batched. The ECS adapter preserves stable PhysicsId ordering across rigid bodies,
characters and vehicles, then optimizes the broadphase after structural changes.
Layer zero is nonmoving; the other application layers share the moving broadphase
tree, with the collision matrix filtering object layers. No gratuitous layer-tree
split was added to the already appropriate two-tree rigid-body configuration.

Column extraction writes native positions/rotations/velocities/awake flags directly
into reusable SoA buffers. ECS writes compare values before mutating change ticks;
missing mirror components are restored. Sorted rows and ordered bindings are walked
together. The host must clear Bevy frame trackers after consumers finish; otherwise
resource-scope removal history grows. Warm unsubscribed stepping/columns and quiet
ECS plus tracker maintenance perform zero Rust allocations/reallocations in the
dedicated allocation test. Structural edits, subscribed contact-map churn, the
generic all-hit query API and Bevy's scoped shared-job tasks can still allocate.

Characters use a refitted AABB candidate tree, then Jolt's original exact checks
in stable CharacterID order. The reference path is the original
`CharacterVsCharacterCollisionSimple`, with no tree rebuild/copy/refit overhead.
Sweeps honor the collector's padded fraction, including values above one. Cache
bounds conservatively retain padding at large coordinates; growing float ULPs
reduce selectivity and do not recover position precision near extreme coordinates.
The cache is rebuilt after snapshot restore and structure changes.

## Determinism and gates

Final default-mode tests pass for in-process replay, separate processes, 1/4/8
threads, snapshots into the same/fresh world, dedicated/shared worker equivalence
and restore across worker modes. These tests construct `deterministic_network`,
including its bounded contact listener. Local floating-point mode also passes
the same-platform replay and performance-contract tests; it is not a portable
network configuration. Cross-platform equality has not been tested on this PC.

Regression coverage includes failed body-batch retry, duplicate/stale removal,
quiet ECS change ticks/missing columns, 131,072 host jobs across queue wrap/reuse,
dense moving/crouching/jumping crowds with removal/restore, and the lost-padding
case at X=1,048,577. Independent static review rounds found and resolved creation
retry, contaminated legacy baseline, unsafe queue reuse and float-bound issues.
The final independent review found no actionable defects in the complete bowl
ECS harness, fixture refactor, manifests, provenance or reported figures. It was
static source/artifact review, not an independent rerun of the measurements.

```powershell
$env:CARGO_BUILD_JOBS='2'; $env:RUST_TEST_THREADS='2'
cargo test -p ara-jolt-sys -p ara-physics -p ara-physics-jolt -p ara-anim-physics --features ara-physics-jolt/bench
cargo test -p ara-jolt-sys -p ara-physics -p ara-physics-jolt -p ara-anim-physics
cargo clippy -p ara-jolt-sys -p ara-physics -p ara-physics-jolt -p ara-anim-physics --all-targets -- -D warnings
cargo clippy -p ara-jolt-sys -p ara-physics -p ara-physics-jolt -p ara-anim-physics --all-targets --features ara-physics-jolt/bench -- -D warnings
cargo fmt -p ara-jolt-sys -p ara-physics -p ara-physics-jolt -- --check
cargo test -p ara-physics-jolt --no-default-features --features bench --test determinism --test performance_contract
cargo test -p ara-physics-jolt --features bench --test steady_allocations
./crates/ara-physics-jolt/benches/run.ps1 -Rounds 1
# Retain a pre-pass executable outside the checkout for this historical A/B:
./crates/ara-physics-jolt/benches/compare.ps1 -IncludeMatrix -FpCost -BaselineBinary <pre-pass-scale.exe>
# Matched ECS control; see source/target isolation instructions below:
./crates/ara-physics-jolt/benches/ecs-compare.ps1 -ControlBinary <matched-control.exe> -CurrentBinary <current.exe>
./crates/ara-physics-jolt/benches/ecs-bowl.ps1
```

`run.ps1 -BuildOnly` / `-RunOnly` separate compilation from measurement. Optional
`CARGO_TARGET_DIR` relocates generated artifacts; no CMake/Python is required.
The aggregate is written by `ara-json`'s canonical writer to [RESULTS.json](RESULTS.json).
Raw logs/per-case files remain in ignored `benches/results/`; executable hashes,
settings, final-state equality, per-phase measurements and load endpoints are
preserved in the checked aggregate. The pre-pass executable itself is not shipped.
The aggregate reads only the explicit manifest, rejects incomplete/wrong-run cases,
and never treats arbitrary JSON files in the directory as new measurements.
Standalone `compare.ps1` starts a fresh manifest; `-IncludeMatrix` explicitly
extends the just-completed matrix. Per-launch receipts identify retained historical
binaries, whose original output schemas predate the run-ID field.

This published aggregate contains 36 explicitly named cases (including four
provenance objects) and 103 measurement records. All completed same-settings
native pairs match final body bits and retain every body in the bowl. The
four final ECS control/current records share a final hash, and each complete
ECS bowl record independently matches its native reference.

Each receipt separates the **launcher revision** from the binary's **build
revision** and SHA256. The first matrix's source mappings are explicitly labeled
retrospective; they identify equivalent compiled source at `20706e4`, not an
invented exact clean build commit. The retained pre-pass executable's source
revision is unknown and recorded null; its bytes are identified by SHA256
`2D48DF466C0F217ECCFC0D1067CAA364B6A33E282806ECED28850FB641AF336E`.
Settings and old schema are preserved, but this provenance limit prevents a
claim that the old executable can be rebuilt from one proven commit.

To rebuild the matched standalone ECS control, check out `20706e4`, apply
`lookup-control.patch`, then copy `performance.rs` and `performance/workloads.rs`
from `9935ecf`. Build it with `--features bench --bench performance` in a
**different CARGO_TARGET_DIR** from the current worktree. Record that base,
patch hash and both harness source hashes in the `.exe.build.json` sidecar as
shown in the aggregate. Both harnesses include `clear_trackers()` in total time.
Cargo targets shared between same-named packages in separate worktrees can reuse
the wrong local library: the final builds clean both physics packages in each
isolated target and verify the library source path and a control-only code marker.
The four final hashes, paired executable hashes and isolated build receipts are
in `ecs-provenance`. The earlier full-extras ECS comparison is retained as a
separately labeled experiment against the earlier zip-only implementation; the
standalone final comparison is the result reported here.

## Published engine context

These published results are context, not a same-settings engine ranking:

| Primary source | Published workload / result | Comparison limit |
|---|---|---|
| [Epic, 4 May 2022](https://www.unrealengine.com/tech-blog/chaos-scene-queries-and-rigid-body-engine-in-ue5?lang=en) | 512-convex tumbler: PhysX 3 3.17 ms, Chaos UE5.0 4.76 ms; 64 ragdolls: 5.30 / 8.18 ms; terrain + 100k static / 512 dynamic: 10.82 / 5.34 ms. | Means, different scenes/engine generations; hardware, iterations and thread budget are not matched here. |
| [NVIDIA PhysX 5.7 GPU simulation](https://nvidia-omniverse.github.io/PhysX/physx/5.7.0/docs/GPURigidBodies.html) | Documents separate CUDA broadphase and dynamics acceleration. | GPU throughput is not a CPU-Jolt comparison; no equivalent 20k CPU bowl/p95 result was found in these primary sources. |
| [Unity physics performance guide](https://unity.com/how-to/enhanced-physics-performance-smooth-gameplay) | Discusses PhysX, broadphase, solver iterations, sleeping, callbacks and batched queries. | No equivalent published 5k/20k bowl or Outpost timing/settings supplied. |

Do not relabel Jolt's older PhysX 4.1 scaling paper as PhysX 5, or extrapolate a
512-body tumbler into a 20k-body bowl. Establishing an engine ranking still requires
running equivalent scenes with matched solvers, callbacks, FP mode and hardware.
