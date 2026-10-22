# S8: DOTS-class performance benchmark

## Status

**PASS:** all four setups, all four workloads (including both boid counts), both thread modes, 300 measured frames after warmup, and managed SIMD variants are recorded. Unity ran as a non-development standalone IL2CPP player with Burst enabled. Full-state comparisons are bit-identical across runtimes.

**Recommendation:** keep Rust plus batched C# and explicit SIMD. These results do **not justify starting a C#-subset LLVM compiler in M4**. Rust is competitive with Burst, but this harness does not establish blanket all-core parity: 50k boids are about 1.95x slower in Rust. Keep compiler work in the later, evidence-gated build order.

## What was built

Four independently launched, headless setups implement the same fixed-step workloads:

- **Unity DOTS:** Unity **6000.6.6f1**, a standalone Windows x64 **IL2CPP Release** player, development mode **off**, Burst **on**. `ISystem` schedules Burst `IJobEntity` jobs and waits for completion. W4 uses a Burst `IJobParallelFor` and `EntityCommandBuffer.ParallelWriter`.
- **Rust:** `bevy_ecs`/`bevy_tasks` **0.20.0**, `glam` **0.30.9**, LLVM release optimization, ThinLTO, one codegen unit, `target-cpu=native`. `Query` for single-thread mode; `par_iter_mut` for all-core mode. W4 records typed intents, then uses Bevy `Commands`/`CommandQueue` for structural playback.
- **C# JIT:** .NET **10.0.6**, SDK **10.0.202**, Release IL, default tiered compilation/PGO. A self-hosted managed executable calls a Rust DLL to allocate and free native component columns. Chunk-batched spans operate directly on that Rust-owned memory. There are no per-entity ABI calls or managed component objects.
- **C# NativeAOT:** the same project/kernels published for `win-x64`, optimized for speed and the native instruction set. W3 has both scalar and explicit **Vector256**, two-float4-per-vector variants, in **both** JIT and AOT.

Unity 6000.6 overrides registry package requests with built-in **Entities 6.6.0, Collections 6.6.0, Burst 2.0.0, Mathematics 1.4.0**. The manifest and package lock record those effective versions; these are not Entities 1.5/Burst 1.8 measurements. The IL2CPP build succeeded: no editor fallback was required.

[`results/burst-build.json`](results/burst-build.json) records the AOT build's **LLVM 21** backend, **AVX2/SSE2** targets, safety checks **Off**, and all seven compiled benchmark job entrypoints. Rust used **LLVM 22.1.2**. [`results/artifacts.json`](results/artifacts.json) records SHA-256 hashes of the measured executables/libraries.

The C# launch direction is deliberately the task's simpler self-hosted option: .NET starts the process and loads the Rust host library. This measures the kernels, borrowed-memory boundary, and batched structural ABI, rather than `hostfxr` startup. It is a throwaway column-layout stand-in, not a completed Ara scripting/ECS integration.

## Reproduce

From the repository root, in Windows PowerShell 5.1:

```powershell
# Explicitly authorized benchmark builds, scoped to this standalone spike.
& ./spikes/s8-dots-bench/run.ps1 -Phase Build

# Full counts, 10 steps, every float compared against single-thread Rust.
& ./spikes/s8-dots-bench/run.ps1 -Phase Validate -Run final
python ./spikes/s8-dots-bench/analyze.py --validate --run final --complete

# Each case: 120 warmup + 300 measured frames; sequential launches only.
& ./spikes/s8-dots-bench/run.ps1 -Phase Measure -Run run1 -Dump
& ./spikes/s8-dots-bench/run.ps1 -Phase Measure -Run run2 -Setups @('unity','aot','jit','rust') -Dump
python ./spikes/s8-dots-bench/analyze.py --run run1 --complete
python ./spikes/s8-dots-bench/analyze.py --run run2 --complete
python ./spikes/s8-dots-bench/analyze.py --validate --long --run run1 --complete
python ./spikes/s8-dots-bench/analyze.py --validate --long --run run2 --complete

# The fast JIT SIMD case needs longer warmup; repeat the entire W3 workload.
& ./spikes/s8-dots-bench/run.ps1 -Phase Measure -Run steady1 `
  -Cases @('w3:1000000') -Warmup 3000 -Dump
& ./spikes/s8-dots-bench/run.ps1 -Phase Measure -Run steady2 `
  -Cases @('w3:1000000') -Warmup 3000 -Setups @('unity','aot','jit','rust') -Dump
python ./spikes/s8-dots-bench/analyze.py --run steady1
python ./spikes/s8-dots-bench/analyze.py --run steady2
python ./spikes/s8-dots-bench/analyze.py --validate --long --run steady1
python ./spikes/s8-dots-bench/analyze.py --validate --long --run steady2

# Optional Unity editor-in-batchmode comparison (same long-warmup W3 state).
& ./spikes/s8-dots-bench/run.ps1 -Phase Measure -Run editor `
  -Cases @('w3:1000000') -Warmup 3000 -Setups @('unity') -EditorFallback -Dump
python ./spikes/s8-dots-bench/analyze.py --validate --long --run editor --reference-run steady2

# Local gates; no workspace-wide build or CI.
$env:CARGO_BUILD_JOBS = '4'
$env:RUSTFLAGS = '-C target-cpu=native'
cargo check --manifest-path ./spikes/s8-dots-bench/Cargo.toml
cargo clippy --manifest-path ./spikes/s8-dots-bench/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path ./spikes/s8-dots-bench/Cargo.toml
cargo fmt --manifest-path ./spikes/s8-dots-bench/Cargo.toml -- --check
dotnet build ./spikes/s8-dots-bench/managed/Managed.csproj -c Release --disable-build-servers -warnaserror
```

The scripts require the installed Rust/.NET/Visual Studio C++/Unity tools and Python with NumPy (measured with **2.5.2**). No global installs were performed. `run.ps1` adds the existing Visual Studio Installer directory to its process PATH for NativeAOT's `vswhere.exe` discovery; omitting it caused an initial malformed-linker-path failure. The script sets `CARGO_BUILD_JOBS=4` for every Cargo build.

The Unity build command issued by the script is:

```powershell
& 'C:/Program Files/Unity/Hub/Editor/6000.6.6f1/Editor/Unity.exe' `
  -batchmode -nographics -quit `
  -projectPath "$PWD/spikes/s8-dots-bench/unity" `
  -executeMethod S8Unity.Build.Player `
  -logFile "$PWD/spikes/s8-dots-bench/out/unity-build.log"
```

For an individual player case (the script waits for the process and checks its exit status):

```powershell
& ./spikes/s8-dots-bench/run.ps1 -Phase Measure -Run individual `
  -Setups @('unity') -Cases @('w3:1000000') -Threads @(16)
```

`-EditorFallback` runs `-batchmode -nographics -executeMethod S8Unity.Build.EditorBenchmark`; results identify themselves as `unity-editor`. It was used for an optional W3 comparison, not as a substitute for IL2CPP. Build/player logs and binary dumps live under ignored `out/` and `unity/Build/`. Raw frame times, validation summaries, and load snapshots are committed under `results/`.

## Identical workload contract

All setups create Position/Velocity/Rotation float4 values and a stable ID. Initial data uses the same integer avalanche hash of ID and fixed salts; its low 16 bits are converted exactly to floats. Positions are uniform in a 64-unit cube, velocities in [-1,1), initial rotation is (0,0,0,1). There is no wall-clock time or RNG in simulation. `dt = 1/60`.

| Workload | Count | Per-step work |
|---|---:|---|
| W1 transforms | 1,000,000 | `p += v*dt`; angular velocity `0.1*v.xyz`; quaternion Euler derivative `omega*q`; `q += derivative*(0.5*dt)`; normalize using sqrt and reciprocal |
| W2 boids | 20,000 and 50,000 | Jacobi position/velocity snapshot; rebuild a spatial hash; separation/alignment/cohesion; speed limit 4; integrate and wrap each axis to [-32,32) |
| W3 particles | 1,000,000 | `v=(v+(0,-9.81,0,0)*dt)*0.999`; `p+=v*dt`; if y<0, reflect y and set vy=`abs(vy)*0.8` |
| W4 churn | 100,000 base | Replace the next ring of 10,000 entities: 10,000 despawns and 10,000 spawns, including initial component values, through deferred command buffers. Live count stays 100,000 |

The spatial hash is a bounded 16x16x16 grid, cell width 4, with collision-free flattened integer keys. It uses counting, a prefix sum, and ascending-ID scatter. Each boid visits the 27 adjacent cells in z/y/x order and their IDs in ascending order. Neighbours have `0 < distance² < 16`. Separation sums `delta/max(distance²,0.01)`; acceleration is `0.02*separation + 0.05*(meanVelocity-velocity) + 0.01*(meanPosition-position)`. The hash is rebuilt serially in every setup so unordered concurrent insertion does not change arithmetic. Snapshot and index construction are timed in both thread modes. Boundary wrapping does not make neighbour distances toroidal; all implementations use the same clipped search.

## Measurement method and limits

- One fresh process per case; 120 warmup frames then 300 measured frames. Each full matrix has 44 cases: 4 setups x 5 count/workload pairs x 2 thread modes, plus 4 managed W3 SIMD cases. W3 was additionally repeated twice, across all six scalar/SIMD variants and both thread modes, with **3000 warmup + 300 measured frames**.
- p50/p95 are nearest-rank percentiles of individual frame wall times. Creation, startup/JIT warmup, JSON writing, checksums, and final binary dumps are outside frame timing. Simulation, scheduling, job joins, spatial-index rebuilds, command recording, allocation attributable to those operations, and structural playback are inside it.
- Single-thread Unity has zero job workers; the main thread runs completed work. All-core Unity uses 15 workers plus the joining main thread. Bevy/Rayon pools and managed `Parallel.For` are configured for 16. This is a concurrency limit, not CPU affinity or an assurance that every worker is useful on every frame.
- Managed batches are 4096 entities for W1/W3 and 256 for W2/W4. Thread-pool minimum workers and maximum parallelism are 16 in all-core mode. `Parallel.For` scheduling allocations are included, with allocation and GC counters recorded separately.
- W4's C# cases share the Rust/Bevy structural playback implementation. They compare managed intent production plus ABI against native intent production, not an independent managed ECS. Rust/C# retain actual entity handles by logical slot; Unity's ECB requires a timed post-playback query to recover handles. That is a real implementation cost and prevents interpreting W4's ratio as pure compiler code quality.
- Rust and Unity use ECS columns, while C# has contiguous borrowed native columns. Their layouts, change tracking and schedulers differ. Rust uses `glam`; scalar C# expresses float4 arithmetic as struct fields. W3 SIMD makes vectorization explicit. These are complete workload comparisons, not isolated instruction-for-instruction compiler shootouts.
- Strict arithmetic and defined neighbour order prioritize equivalent results. Burst uses `FloatMode.Strict`/standard precision, not fast-math. This is not a test of maximum relaxed-precision Burst throughput, nor a cross-CPU determinism proof.

## Machine and background load

See [`results/machine.json`](results/machine.json) and each `*.load.json`. Measurements ran on the supplied Windows PC: **Ryzen 7 7800X3D, 8 cores/16 threads, 31.1 GiB visible RAM, RTX 4070 SUPER, Windows 11 Pro 10.0.26300**. CPU workloads are headless and do not use the GPU.

The desktop was active, with **VALORANT**, Discord, NZXT CAM, NVIDIA services/overlays, T3 and other agents. These are **contended desktop measurements**, not idle-machine peak throughput. All spike-owned compilers and the Unity editor were closed before the 300-frame player/native measurements; timed runners were sequential. The optional editor measurement runs only its own editor process. Every load file records free RAM, launch duration, the largest background CPU deltas, and background CPU as a percentage of 16 logical CPUs. The estimate spans process startup through output/exit and is coarse; it does not isolate the measured frames or count short-lived background processes that exit between snapshots. Short-run validation scratch files are ignored; their numerical summaries are committed separately.

The first full run had **9.4–33.6%** estimated background CPU (median **20.3%**), with **5.3–7.1 GiB** free RAM. The reversed-order second run had **2.3–36.1%** background CPU (median **7.4%**), with **5.3–9.3 GiB** free RAM. Load decreased substantially during the second run. The active power plan was **Balanced**. User/background applications were left under their existing owners' control.

The report uses **run2** for W1/W2/W4 and **steady2** for every W3 case. This is a whole-workload warmup correction, not a per-case fastest-run selection. Original runs and the first long-warmup repeat remain committed for comparison.

## Results

All cells are **p50 / p95 milliseconds per frame**, rounded to three decimals. W1/W2/W4 use 120 warmup frames; W3 uses 3000, as marked. Every cell has 300 measured frames. A dash means the explicit-SIMD variant was only implemented for managed C#.

### Single thread

| Workload | Unity IL2CPP + Burst | Rust Bevy | C# JIT | C# NativeAOT |
|---|---:|---:|---:|---:|
| W1: 1M transforms | 6.238 / 7.550 | 4.581 / 5.630 | 6.789 / 7.673 | 6.917 / 8.549 |
| W2: 20k boids | 10.403 / 11.697 | 12.626 / 14.698 | 12.894 / 15.162 | 13.340 / 16.367 |
| W2: 50k boids | 55.023 / 60.142 | 70.101 / 79.264 | 72.388 / 80.825 | 72.117 / 80.883 |
| W3: 1M particles, scalar, long warmup | 6.949 / 7.807 | 6.265 / 6.767 | 6.559 / 7.606 | 6.439 / 7.128 |
| W3: 1M particles, Vector256, long warmup | — | — | 0.434 / 0.489 | 0.465 / 0.677 |
| W4: 10k spawn + 10k despawn / 100k | 2.500 / 2.809 | 0.978 / 1.197 | 1.119 / 1.437 | 1.172 / 1.385 |

### All cores: concurrency configured to 16

| Workload | Unity IL2CPP + Burst | Rust Bevy | C# JIT | C# NativeAOT |
|---|---:|---:|---:|---:|
| W1: 1M transforms | 0.529 / 0.588 | 0.774 / 0.890 | 1.126 / 1.312 | 1.157 / 1.383 |
| W2: 20k boids | 0.988 / 1.140 | 1.323 / 1.951 | 1.827 / 2.017 | 2.079 / 2.468 |
| W2: 50k boids | 4.771 / 5.377 | 9.300 / 10.871 | 10.526 / 12.662 | 11.220 / 13.693 |
| W3: 1M particles, scalar, long warmup | 0.505 / 0.641 | 0.730 / 0.770 | 0.939 / 1.607 | 0.913 / 1.014 |
| W3: 1M particles, Vector256, long warmup | — | — | 0.118 / 0.177 | 0.132 / 0.177 |
| W4: 10k spawn + 10k despawn / 100k | 2.590 / 3.979 | 1.174 / 1.603 | 1.111 / 1.684 | 1.085 / 1.292 |

### Repeats, warmup sensitivity, and particle state

Both complete initial matrices are in [`results/run1/`](results/run1/) and [`results/run2/`](results/run2/). Across the 44 cases, run2/run1 p50 had median **0.924**, with six cases moving by more than 20%. The original JIT all-core SIMD case was the clear outlier: **0.194 / 0.962 ms** in run1 and **0.705 / 0.942 ms** in run2. In run1, the median of the first 100 measured frames was **4.82x** the last 100. These are valid recorded timings, but a poor estimate of optimized steady performance. Delayed tiering is a plausible explanation; JIT code transitions were not traced.

Repeating **all W3 variants** with 3000 warmup frames stabilized this case: JIT SIMD all-core p50 was **0.115 ms** in steady1 and **0.118 ms** in steady2. Its first-100/last-100 median ratio was **0.93 / 1.07** respectively. NativeAOT SIMD all-core was **0.142 / 0.132 ms** p50 across those repeats. The full raw data is in [`results/steady1/`](results/steady1/) and [`results/steady2/`](results/steady2/).

Longer warmup also changes the physical distribution: after 50 simulated seconds the particles bounce close to the plane. Collision-heavy scalar kernels become substantially slower, while masked SIMD remains fast. For context, the **120-warmup** run2 W3 p50 values were Unity/Rust/JIT/AOT **1.515 / 1.110 / 2.404 / 2.413 ms** single-threaded and **0.190 / 0.259 / 0.531 / 0.596 ms** all-core. The corresponding SIMD values were JIT/AOT **0.477 / 0.466 ms** single-threaded; NativeAOT SIMD was **0.178 ms** all-core. The unstable short-warmup JIT SIMD all-core value is reported above. All setups use identical state evolution within each warmup regime; the two regimes should not be mixed as though their branch distributions were identical.

### Managed allocation and GC

Across run1/run2/steady1/steady2, there were **zero reported gen0/gen1/gen2 collections and zero measured GC pause time** during every 300-frame window. Single-thread JIT reported **0 allocated bytes**, and NativeAOT reported **40 bytes total** over the window. Parallel runs allocated about **3.5–5.2 kB/frame**, from the harness/scheduling path including `Parallel.For`. Native component storage and kernel data remain in Rust memory. These counters cover this small benchmark heap, not a full game's GC behaviour.

### Optional editor comparison

For the same W3 long-warmup workload, Unity in batchmode measured **6.722 / 7.713 ms** single-thread and **0.546 / 0.812 ms** all-core, versus the standalone player's **6.949 / 7.807 ms** and **0.505 / 0.641 ms**. These are synchronous system/job timings, not complete editor-rendered frames. Burst was enabled and Burst safety checks explicitly disabled; editor-side container/EntityManager checks and bookkeeping still differ. Background load also differed, so this is illustrative rather than a controlled estimate of editor overhead. Raw results are in [`results/editor/`](results/editor/).

## Validation and local gates

The full 10-step matrix compared **every entity and every one of its 12 floats** against single-thread Rust. All **44/44 cases were bit-identical**; tolerance was `abs(error) <= 2e-4 + 2e-5*abs(reference)`, with non-finite output rejected. This includes both boid counts, both thread modes, churn outputs ordered by logical slot, and scalar/SIMD variants. [`results/validation-final.json`](results/validation-final.json) records counts and maximum errors.

The same comparison passed bit-for-bit after all **420 steps in both complete runs** (44 cases each) and **3300 steps in both long-warmup W3 runs** (12 cases each). See [`results/validation-long-run1.json`](results/validation-long-run1.json), [`results/validation-long-run2.json`](results/validation-long-run2.json), [`results/validation-long-steady1.json`](results/validation-long-steady1.json), and [`results/validation-long-steady2.json`](results/validation-long-steady2.json). Each full matrix compares **256.32 million floats**; each W3 matrix compares **144 million**. Maximum absolute error was **0.0** throughout.

Rust `check`, strict Clippy, formatting, and the two focused tests pass. C# Release builds with warnings as errors and NativeAOT publish pass. Unity builds as standalone IL2CPP and player JSON confirms `burst_enabled=true` and `development=false`.

## Findings and recommendation

**Is Rust at Burst-class speed?** It is competitive, but general parity is not proven. On the reported W1/W2/W3 cases, Rust's single-thread p50 is **0.73–1.27x** Unity's, while all-core p50 is **1.34–1.95x** Unity's. The 50k boid result (**9.300 vs 4.771 ms**) is a concrete gap to investigate in query layout, math code generation, index construction, and scheduling. Sharing LLVM is not a sufficient performance guarantee. W4 is faster in this Rust harness, with the structural-playback asymmetry described above.

**How far is C# from Burst?** Scalar JIT/AOT are around **0.93–1.32x** Unity's single-thread p50 and **1.81–2.35x** all-core on the reported W1/W2/W3 cases. NativeAOT is not a consistent throughput upgrade over optimized JIT. Explicit Vector256 W3 is faster than the reference Burst kernel: **0.434/0.465 vs 6.949 ms** single-thread and **0.118/0.132 vs 0.505 ms** all-core (JIT/AOT vs Unity). That improvement combines branchless masked collisions, vector width, contiguous columns, and different scheduling/layout costs. Equivalent branchless Burst/Rust kernels would be a useful next control before interpreting this as a compiler ranking.

**Is a C#-subset LLVM compiler justified for M4? No, on this evidence.** Explicit SIMD already removes the demonstrated particle gap. The all-core boid gap also exists in Rust, and structural playback is primarily an ECS implementation concern. Building a compiler before identifying kernel-only losses would risk solving the wrong problem. Keep Rust for engine/hot gameplay systems, generated batched-span C# bindings, explicit SIMD helpers, and NativeAOT exports. Prioritize the actual ECS/ABI scheduler integration and profiling before custom compiler work. Repeat this matrix on an idle desktop before making firm frame-budget commitments.

Gotchas worth carrying into the real integration:

- **Warmup needs a time/steady-state check**, not just a low fixed frame count. The fastest kernels can finish 120 warmup frames before optimized JIT code is ready. Simulation state changes during longer warmup and must remain matched across setups.
- **Different ECS bookkeeping matters.** Bevy 0.20's entity metadata length is allocated capacity, and total spawned entities include an internal entity. Count the benchmark's `StableId` query. Bevy change tracking, Unity chunk metadata and ECB handle recovery are not language/compiler costs.
- **Managed SIMD is practical over native memory.** The same source compiles under JIT and NativeAOT; there are no per-entity calls or marshaled component copies. The future real chunk API must preserve non-overlapping ownership and await all workers before playback/free.
- **Allocation-free kernels do not make the scheduler allocation-free.** The current `Parallel.For` path allocates a few kB/frame. A real engine job dispatcher should reuse work descriptors; a new C# compiler would not address that by itself.

## PASS/FAIL against the task

| Criterion | Result | Evidence |
|---|---|---|
| Four setups x four workloads, both boid counts, single/all-core | **PASS** | Two complete 44-case matrices, plus two complete W3 repeats |
| p50/p95, 300 measured frames after warmup | **PASS** | Per-frame arrays and percentile fields in every result JSON; W3 steady-state reruns use 3000 warmup |
| Standalone non-development IL2CPP + Burst | **PASS** | Successful CLI build; every player JSON reports Burst on/development off |
| Identical math and results within tolerance | **PASS** | Every float bit-identical after 10, 420 and 3300 steps |
| Machine and background load recorded; noisy runs repeated | **PASS** | `machine.json`, per-case load files, reverse-order repeats, and warmup investigation |
| Clear Rust/C#/compiler recommendation | **PASS** | Competitive Rust with an all-core boid gap; explicit SIMD works; defer compiler implementation from M4 |

## Proposed decision

Proposed text for the parent agent to adopt or revise; **existing A12 remains in force until the parent changes the decision log**:

> S8 supports retaining Rust engine/hot gameplay systems and C# systems over chunk-batched native spans, with explicit SIMD helpers and NativeAOT for exports. Treat “Burst tier” as a performance target, not parity guaranteed by LLVM: the current Rust all-core 50k-boid harness is about 1.95x Unity DOTS. Prioritize ECS query/layout/job-dispatch tuning and representative whole-game profiling. Keep the C#-subset LLVM compiler as a later, evidence-gated item rather than placing its implementation on M4's critical path. Revisit when matched-layout, stable-warmup, idle-machine profiles show persistent compiler-generated kernel gaps that explicit SIMD and the Rust escape hatch do not address.

No parent-owned decisions, plans, root `AGENTS.md`, or `README.md` were edited.
