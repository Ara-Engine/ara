# Mac Unity CPU comparison

This harness copies Ara's S8 fixture into `target/perf-scale/unity-s8` and runs
its existing numerical kernels in **Unity Editor + Burst**, headless and without
a graphics device. It does not modify the original S8 project or user projects.
It is not the recorded Windows IL2CPP/Burst 2.0 lane and does not benchmark the
whole Ara runtime bridge, renderer or editor.

The initial lane targets installed Apple-silicon Unity **6000.4.5f1**, built-in
Entities/Collections **6.4.0**, Burst **1.8.29**, and Mathematics **1.3.3**.
The executed Editor reports its actual registered package versions alongside
every result. Package resolution can download missing compatible dependencies;
`UPM_CACHE_ROOT` is scoped to scratch, with no user cache/config override.

The controlled [2026-11-08 result](../unity_mac_2026-11-08.json) records two reversed
rounds on an Apple M5 Pro, with all eight full-state comparisons and all eight
effective-option/restoration checks passing. Its per-case statistics,
source/executable hashes, package versions, load and battery-power provenance
define the scope of those measurements. The earlier
[exploratory matrix](../unity_mac_2026-11-08_exploratory.json) is retained separately.
The expanded [W2/W4 baseline](broad_mac_2026-11-08.json) is explicitly partial:
one forward round is complete, while the reversed round awaits a clear CPU slot.
It includes actual Rayon participation, frozen compiled-source hashes and the
structural bookkeeping differences. Its legacy dump verifies numeric fields and
live counts, and does not export/assert exact StableId values.

After the controlled W1/W3 measurements, `MacBatch.Run` gained an early non-batch
guard so manual invocation cannot exit a user's Editor. The recorded executed
wrapper hash remains `044c48250e0a2b7aa7b0d64cb310a0afc43384ea583867d3d1278c6109e41c30`.
This later guard changes only the non-batch path; historical executed hashes are
retained and were not replaced with the current source hash.

Prepare without starting Unity, then inspect an exact launch command:

```sh
/usr/bin/python3 benchmarks/iteration/unity/bench.py prepare
/usr/bin/python3 benchmarks/iteration/unity/bench.py command --workload w1 --threads 1
```

Run these only when compilation and other CPU measurements have stopped:

```sh
/usr/bin/python3 benchmarks/iteration/unity/bench.py run --workload w1 --threads 1 --run mac-01
/usr/bin/python3 benchmarks/iteration/unity/bench.py run --workload w1 --threads 15 --run mac-01
/usr/bin/python3 benchmarks/iteration/unity/bench.py run --workload w3 --threads 1 --run mac-01
/usr/bin/python3 benchmarks/iteration/unity/bench.py run --workload w3 --threads 15 --run mac-01
```

The unchanged S8 boids and structural workloads are also supported:

```sh
/usr/bin/python3 benchmarks/iteration/unity/bench.py run --workload w2 --count 20000 --threads 1 --run broad-01
/usr/bin/python3 benchmarks/iteration/unity/bench.py run --workload w2 --count 50000 --threads 15 --run broad-01
/usr/bin/python3 benchmarks/iteration/unity/bench.py run --workload w4 --threads 15 --run broad-01
```

W2 defaults to **20,000** entities, W4 to exactly **100,000**. W1/W3 retain
their 1-million defaults. W2/W4 use 120 warmup and 300 measured steps. W2 includes
snapshot, stable grid build and neighbour processing; use Ara batch **128**.
W4 replaces 10,000 entities per step and includes recording and deferred playback.
Unity also rebuilds its slot handles after ECB playback; Ara updates those handles
while recording its deferred spawn commands. Both bookkeeping paths remain timed.

The launcher refuses an already-running Unity Editor, runs only its own process
at niceness 10, and never changes licenses. A license error is a blocker to report.
Use a fresh `--run` for each repeat; timed runners must stay sequential. Reverse
case and engine order between rounds. Logs, results, raw state dumps and imported
project state stay under `target/`; checked-in files are never benchmark outputs.

Burst 1.8.29 synchronizes global options through `OptionsChanged`: its
`Runtime/Editor/BurstEditorOptions.cs` writes EditorPrefs and SessionState even
when a benchmark changes only safety checks. The wrapper captures the initial
compilation/safety/force-safety/debug values and disables force-safety/debug before
the benchmark. It restores those values and all five known Burst preference keys,
including their initial presence, in `finally` before exiting on handled failures.
Metadata records initial preferences, effective options captured before timing,
and checked restoration results. The launcher requires this controlled metadata.
This does not promise that preference keys were never written or that forced
termination can execute `finally`. The original 2026-11-08 timings used the
previous wrapper, which did not restore or capture initial/force-safety/debug
settings; those results remain exploratory and are superseded by the separate
controlled matrix above. The controlled setup smoke and every measured process
passed the current checks.

Build the optimized Ara counterpart only in a separate compilation slot:

```sh
CARGO_BUILD_JOBS=2 RUSTFLAGS='-C target-cpu=native' \
  CARGO_PROFILE_BENCH_LTO=thin CARGO_PROFILE_BENCH_CODEGEN_UNITS=1 \
  cargo bench -p ara-sim --bench ecs --no-run
```

Run the executable path Cargo reports directly, without invoking Cargo during
measurement. Record its SHA256, source revision, Rust version and build flags.
Its arguments are `WORKLOAD COUNT WORKERS WARMUP FRAMES VARIANT BATCH RESULT`.
Use **`simd`** for the final optimized Ara comparison: it dispatches the SIMD
slice kernels through the contiguous query path. `tuned` deliberately excludes
those kernels and is an optional intermediate baseline. Record the variant in
every comparison and validate the final `simd` state independently.

For example, after setting `ARA_ECS_BENCH` to Cargo's reported executable:

```sh
"$ARA_ECS_BENCH" w1 1000000 1 120 300 simd 4096 target/perf-scale/ara-w1-t1.json
"$ARA_ECS_BENCH" w1 1000000 14 120 300 simd 4096 target/perf-scale/ara-w1-t15.json
```

Here `t15` in the second output filename describes the maximum active runner
budget (14 pool workers plus caller), while the recorded Ara `threads` value is
14. Use W3's 3000 warmup steps for its corresponding cases.

`threads` is the original S8 concurrency request. The fixture sets Unity job
workers to `min(threads - 1, JobWorkerMaximumCount)`, or zero for a request of one;
the caller joins jobs. The CLI cap of 14 is only a ceiling and can be limited by
Unity. Always inspect `actual_job_workers` and `job_worker_maximum`, then match
Ara's configured concurrency rather than assuming a 15-thread result used 14
workers. Ara's current `ecs` argument configures **Bevy pool workers**, and Bevy's
scope caller also executes tasks. For W1/W3, compare Unity's request of 15
(14 workers plus caller) with Ara's argument of **14** (14 workers plus caller).
The argument of one selects a serial loop in both fixtures. A separate Ara
argument of 15 is a different concurrency budget. This follows the installed
Bevy 0.20 sources: `TaskPoolBuilder::num_threads`, `TaskPool::scope`, and
`QueryState::contiguous_par_fold_init_unchecked_manual`.
[Unity worker-count semantics](https://docs.unity.com/en-us/engine/6000.5/script-reference/unity/jobs/lowlevel/unsafe/jobsutility/jobworkercount),
[maximum worker count](https://docs.unity.com/en-us/engine/6000.5/script-reference/unity/jobs/lowlevel/unsafe/jobsutility/jobworkermaximumcount).

W2 uses the same Bevy budget as W1/W3. W4 uses Rayon's `par_iter_mut` for recording;
its external caller blocks in `registry::in_worker_cold`, then performs serial
playback. Use Ara argument **15** versus Unity request **15** for that case, and
verify this with the separate diagnostic before making an observed-concurrency
claim:

```sh
/usr/bin/python3 benchmarks/iteration/unity/bench.py probe-rayon --threads 15 --run broad-01
```

The probe compiles only a tiny diagnostic against cached Rayon artifacts and
reports observed worker IDs, peak concurrent recorders and whether the caller
executed recording work. It runs outside all benchmark timings. Its entry path
matches W4, but its extra arithmetic makes it a diagnostic, not a W4 speed result.
The verification report distinguishes Rayon worker-only recording from Bevy's
workers-plus-caller kernels. Both engines retain serial structural playback.

New query/filter, dirty-fraction, archetype, dependency-job and buffer cases have
a separate [implementation contract](features/CONTRACT.md) and independent
[integer-state oracle](features/reference.py). They are not measured API fixtures
yet; the real Unity/Bevy implementations and full-ID audits remain required.

The math, timed loop, fixed seed, strict Burst FP mode and full state dump come
unchanged from S8; source hashes are recorded by preparation. W1 uses 120 warmup
steps, W3 uses 3000, and both measure 300 steps by default. Verify state against
the matched Rust contract before comparing throughput. Import, compilation,
startup, JSON writing, provenance collection and dumps are outside kernel timing.

After running the matching Ara `ecs` benchmark with identical count, workload,
warmup and measured-step counts, verify all state bytes:

```sh
/usr/bin/python3 benchmarks/iteration/unity/bench.py verify \
  --result target/perf-scale/unity-s8/results/mac-01/w1_1000000_t1.json \
  --reference target/perf-scale/ara-w1-t1.json
```

Verification compiles a tiny hash utility against an existing local BLAKE3
artifact; it neither downloads a dependency nor rebuilds Ara. Unity's raw dump
and Ara's hash both order 12 little-endian floats per logical slot (the stable ID
modulo count for W4). A mismatch
fails verification and writes a result under scratch; never hide it by rounding.

The Package Manager cache override is an environment setting documented by Unity;
the launcher sets it only in the child environment and does not write
`.upmconfig.toml`. This is separate from Burst's option persistence described above.
[Unity cache configuration](https://docs.unity.com/en-us/engine/6000.3/manual/packages-list/package-manager-caches/upm-config-cache).

Unreal is not installed in the inspected local paths. Its CPU measurements remain
unmeasured; this harness does not install an engine or produce an Unreal result.
