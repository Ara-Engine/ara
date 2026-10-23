# ECS tuning: S8 workloads on the 7800X3D

Measured 2026-10-23, Windows 11, Ryzen 7 7800X3D (8 cores / 16 logical CPUs),
Rust 1.99.1. Bevy ECS/tasks 0.20.0 and glam 0.30.9 match S8. Cargo uses four build
jobs. The Unity numbers below are S8's supplied standalone IL2CPP/Burst reference;
Unity was not rebuilt or rerun for this task.

## All-core result

Milliseconds, p50. The two Ara columns are the median of two fresh-process run
percentiles, with the second matrix reversing case/variant order. The before
column runs S8's original Rust query/kernel code on the current machine load.

| Workload | Unity S8 target | Rust S8 | Ara before | Ara optimized | vs Unity |
|---|---:|---:|---:|---:|---:|
| W1: 1M transforms | 0.529 | 0.774 | 0.871 | **0.334** | 36.9% faster |
| W2: 20k boids | 0.988 | 1.323 | 2.121 | **1.265** | 28.0% slower |
| W2: 50k boids | 4.771 | 9.300 | 9.953 | **4.937** | 3.5% slower |
| W3: 1M particles | 0.505 | 0.730 | 0.727 | **0.210** | 58.4% faster |
| W4: replace 10k of 100k | 2.590 | 1.174 | 1.114 | **1.113** | 57.0% faster |

W1/W3 beat their supplied Unity targets. W4 retains the exact S8 command recording
and playback path, with effectively unchanged all-core timing. Boid parity remains
open; these measurements do not establish a win against Unity for W2.

## Single-core and tail latency

Milliseconds; medians of the two run percentiles. A worker count of one uses the
serial query/kernel path; 16 uses all-core dispatch. W4 includes deferred playback.

| Workload | Workers | Before p50 | Optimized p50 | Optimized p95 |
|---|---:|---:|---:|---:|
| W1, 1M | 1 | 4.441 | 2.158 | 2.770 |
| W1, 1M | 16 | 0.871 | 0.334 | 0.432 |
| W2, 20k | 1 | 14.009 | 10.149 | 11.411 |
| W2, 20k | 16 | 2.121 | 1.265 | 1.581 |
| W2, 50k | 1 | 78.150 | 50.159 | 51.855 |
| W2, 50k | 16 | 9.953 | 4.937 | 5.882 |
| W3, 1M | 1 | 5.587 | 1.278 | 1.752 |
| W3, 1M | 16 | 0.727 | 0.210 | 0.252 |
| W4, 100k | 1 | 0.987 | 1.016 | 1.316 |
| W4, 100k | 16 | 1.114 | 1.113 | 1.622 |

The unchanged W4 path's single-core medians differ by about 3%; the broad,
overlapping frame distributions and concurrent background work limit attribution.
No structural-playback optimization or algorithm change was introduced.

## What changed

- Use Bevy 0.20's safe contiguous queries and slice kernels. Each batch updates
  change ticks in bulk through `ContiguousMut::split`; change detection is preserved.
- W1 transposes four quaternions/velocities into SIMD lanes, performs four matching
  updates, and transposes back. W3 uses four-particle lanes and a masked bounce.
  Component table layout and all four float lanes remain the S8 layout.
- W2 preserves the counting-sort grid, ascending IDs within cells, clipped 27-cell
  search and Jacobi update. It repacks positions/velocities into cell order so
  neighbour loops read contiguous columns, and uses Vec4 accumulators. Scratch
  buffers are allocated once. Grid rebuild, source snapshot, repacking and joins
  are included in timings.
- Final batch sizes: 4,096 for W1/W3, 128 for W2. W4 retains its original path.
  Exploratory batch/pool sweeps and a separate parallel repack dispatch did not
  justify replacing this configuration under the observed load.
- Bevy's existing contiguous API suffices for these gains; no unsafe Ara query
  executor, storage fork, sparse-set workaround or second production job pool
  was added. Rayon is a benchmark-only dependency for the unchanged S8 W4 path.

## Remaining boid cost and next work

The separate `profile-w2` diagnostic runs the same math and verifies its final
digest against the matrix. These are medians of two phase p50s, in milliseconds;
phase medians need not sum to the full-tick median from a different run.

| Count | Jacobi snapshot | Grid + cell repack | Neighbour kernel + dispatch |
|---|---:|---:|---:|
| 20k | 0.030 | 0.293 | 0.974 |
| 50k | 0.072 | 0.781 | 3.963 |

The serial grid/repack remains on the critical path. At 20k, neighbour work alone
nearly consumes Unity's entire 0.988 ms target, before Ara's snapshot/grid work.
At 50k, the 0.166 ms target gap is about one fifth of the measured grid/repack cost.
These phase measurements locate Ara's remaining work; they do not isolate Unity's
internal job costs or separate background contention from the residual difference.

Next: cache the benchmark's query states, fuse snapshot/counting/scatter/repack into
a stable cell-owned pipeline, and profile neighbour traversal/dispatch on an idle
machine. A separate parallel repack pass did not show a reliable full-tick benefit
in probes and was reverted. Preserve the defined neighbour accumulation order while
testing further SIMD and batching; a custom executor needs stronger evidence first.

## Method and evidence

- Each case is a fresh process: 120 warmup + 300 measured ticks; W3 uses S8's steady
  3,000 warmup + 300 measured ticks. Setup, output writing and digest computation
  are outside timing. Kernel/queries/grid rebuild/playback are inside.
- Build flags: `-C target-cpu=native`, opt-level 3, thin LTO, one codegen unit.
  Benchmark processes use AboveNormal priority; affinity remains the OS default.
- Loaded-machine run: pre-case CPU counters were **5-93% (mean 30.2%)**, with
  **3.26-7.91 GiB** free physical RAM. Per-process/load snapshots accompany each
  case. Runs were repeated/reversed; this is not an idle-machine parity claim.
- All **40 final case digests match** across baseline/optimized, worker counts and
  repetitions. W1/W2 were also checked after 420 ticks, W3 after 3,300 ticks, and
  a 1,003-entity case checks partial SIMD/chunk batches. Equality is bit-for-bit.
- Final raw samples: `bench-results/run1`, `run2`; machine metadata:
  `bench-results/machine.json`; phase samples: `bench-results/phases`.
  Earlier native matrices (`chunks`, `simd-v1`) and initial portable-CPU probes
  remain local ignored scratch data. They are excluded from every result above;
  the final matrix and phase evidence are committed in full.

## Reproduce

From the workspace root, in PowerShell:

```powershell
$env:CARGO_BUILD_JOBS = '4'
$env:RUSTFLAGS = '-C target-cpu=native'
$env:CARGO_PROFILE_BENCH_LTO = 'thin'
$env:CARGO_PROFILE_BENCH_CODEGEN_UNITS = '1'
cargo bench -p ara-sim --bench ecs --no-run
# Use the executable path printed by Cargo:
$exe = 'target/release/deps/ecs-<hash>.exe'
& crates/ara-sim/benches/run.ps1 -Executable $exe -Rounds 2 -Variants @('baseline','simd')
python crates/ara-sim/benches/analyze.py
& $exe validate-w1 1000000 16 420 1 simd 4096 -
& $exe validate-w2 50000 16 420 1 simd 128 -
& $exe validate-w3 1000000 16 3300 1 simd 4096 -
& $exe validate 1003 16 120 1 simd 128 -
& $exe profile-w2 50000 16 120 300 simd 128 phases.json
```

These benches exercise matched ECS kernels. Optional runtime schema hashing,
recording, checkpoint encoding, rendering and protocol transport are not timed.
