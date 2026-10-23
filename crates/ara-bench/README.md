# Ara runtime performance passes

`ara-bench` measures engine/runtime performance. `arabench` measures agent
ergonomics. Both are local D9 gates; no CI is required.

## Instrument a frame

```rust,ignore
let mut profiler = ara_profile::Profiler::new(Default::default())?;
profiler.register_thread("presentation").attach()?; // setup only
profiler.start();
let frame = profiler.begin_frame()?;
{
    ara_profile::scope!("extract");
    // Extract ECS state and submit render work.
}
// Keep frame.id() with the GPU submission/readback.
profiler.gpu_timings(frame.id(), renderer.timings()); // feature `gpu`
profiler.end_frame(counters, memory)?;
```

Register each task-pool worker during setup. Move a `ThreadRegistration` onto
that worker and attach it once. Move the copyable `FrameToken` with queued work,
then call `token.in_scope(|| { ara_profile::scope!("job.name"); ... })` there.
Workers never lock frame history, allocate records, wait for the host or publish
authoring mutations. Collect at presentation boundaries; late data updates the
original frame while retained. Stop producers before saving a complete capture.
Records and cursors stay local until the frame/job boundary. Custom worker loops
without `FrameToken::in_scope` must call `ThreadRegistration::flush()` there.

Enable subsystem features such as `ara-sim/profiling`, `ara-render/profiling`,
`ara-anim/profiling`, `ara-physics/profiling`, `ara-nav/profiling`,
`ara-audio/profiling`, `ara-asset/profiling`, `ara-script/profiling`.
They default off. Compose `tracing_subscriber::registry().with(ProfileLayer)`
before registering simulation/C# systems, so their reusable named spans have a
subscriber. Attach each executing worker before recording starts.

`Host::profiler()` owns the recorder used by the automatically registered
`profile` Engine Protocol service. `profile.start/stop/summary/hitches/compare`
also back the generated `ara_profile` MCP tool. Parameters carry the session;
summary parameters are `topN`, `limit`, `maxBytes`; compare accepts baseline/current
JSON strings plus `relative` and `absoluteNs`. No service path opens arbitrary files.
Build the CLI/daemon with `--features ara-profile/instrumentation,ara-profile/allocation`
for native CPU scopes and its installed counting allocator. Other executables choose their own global allocator and
can wrap it in `CountingAllocator`; the feature remains off in ordinary builds.

`ImportJob::record_profile` and `BakeJob::record_profile` attach job-manager samples
alongside readiness polls. `Boundary::record_profile` imports compile-server
microseconds as separate elapsed job durations. They do not become CPU self time.

## Script scenes and compare

Save a strict JSON script with a project path relative to the script, a scene
path relative to that project, a seed and run settings:

```json
{
  "project": "../../../ara-host/tests/fixtures/project",
  "scene": "levels/arena.scene",
  "seed": 1,
  "run": { "warmup": 8, "frames": 120, "rounds": 4 }
}
```

```powershell
$env:CARGO_BUILD_JOBS = '2'
cargo run -p ara-bench --release --features scenario,allocation --example scenario -- script.json result.araprof
cargo run -p ara -- profile compare result.a.json result.b.json
```

The example exercises the same scene/seed twice as a harness calibration. Perf
passes implement `Scenario` twice for their baseline and candidate, then use
`interleave` unchanged. Each round reloads/reset state and reverses order on odd
rounds. Outputs: binary `.araprof`, `.json` summary, `.a.json` / `.b.json` complete
metric baselines, `.runs.json` with execution order/frame IDs/background CPU.
OS sampling runs outside timing; intervals below the OS minimum are explicitly
unavailable. Record sufficiently long scenarios to obtain load samples.

The compare command needs no project/daemon. It returns 0 for pass, 1 for a
regression/incomplete evidence, 2 for malformed input. `--json` prints a structured
verdict; text prints a p95 delta table. Defaults: both >5% and >1000 ns increase
must be exceeded. Missing/new metrics fail, and empty/lost/truncated metrics fail.
Retain complete metric summaries for baselines; top-N is only for agent responses.

## Editor frame inspector seam

`Profile::frames` exposes chronological frames. `Frame::timeline()` supplies
thread IDs, nanosecond ranges, parents and allocation totals. `flame_children`
selects roots or children within one thread. `gpu` lists label/queue/duration;
`jobs`, `counters`, `memory`, `load` and `dropped` complete the panel data.
Resolve label indices through `Profile::labels` and CPU thread IDs through
`threads`. Job worker identities belong to their respective job managers.
Durations do not imply absolute clock alignment or a cross-queue critical path.
VRAM categories/counters come from their owning subsystems at `end_frame`.

## Original overhead measurements

Run `cargo run -p ara-bench --release --example overhead`, then repeat with
`--features tracy` and `--features allocation`. Each output records paired median
added cost, 21 rounds, machine background CPU and the active features. Tracy is
not on-demand: its zones are emitted even without an attached viewer.
Recording uses two monotonic clock reads and bounded ring publication; collection
and serialization are outside the timed scope loop. Compile-time-disabled subsystem
markers emit no code. Allocation tests assert zero requested bytes across 100 warm
native frames including collection and GPU/job import.

Recorded 2026-10-23 on Windows 11, Ryzen 7 7800X3D (16 logical CPUs), RTX 4070
SUPER, Rust 1.99.1. Release builds, 21 paired AB/BA rounds; disabled scopes use a
registered thread with recording stopped. Values are median added nanoseconds
per scope above the paired empty loop. Collection is measured separately.

| Mode / raw samples | Disabled | Recording | Collection / span | Background CPU |
| --- | ---: | ---: | ---: | ---: |
| [Native](measurements/native.json) | 7.42 ns | 66.54 ns | 7.91 ns | 16.10% |
| [Counting allocator](measurements/allocation.json) | 7.57 ns | 68.56 ns | 7.58 ns | 26.71% |
| [Tracy](measurements/tracy.json) | 7.52 ns | 103.04 ns | 7.79 ns | 15.51% |

Tracy had no viewer attached; its zone emission was enabled, without on-demand,
sampling or system tracing. Its Windows helper reported `SymInitialize` error 87:
`tracy-client-sys` 0.29.0 documents this harmless duplicate DbgHelp initialization
in `src/dbghelp.rs`. This run measures explicit zones, without call stacks.
The machine is shared, so retain the raw round samples and load alongside results
instead of treating these medians as isolated-machine limits.

## Overhead after the hot-path pass

Same machine/toolchain, 21 paired AB/BA rounds. The longer disabled loops now use
100 million iterations per round so even compiled-out scopes produce a real OS
load sample. Recording uses 16,000 scopes per round; every run validates record
counts and fails on data loss. Negative added costs are measurement noise from
subtracting pipelined empty-loop throughput, not negative execution time.

| Mode / raw samples | Runtime-disabled added cost | Recording | Native collection / span | Background CPU |
| --- | ---: | ---: | ---: | ---: |
| [Shipping, feature off](measurements/overhead-v2/shipping.json) | ~0 ns | 0 ns | N/A | 23.22% |
| [Native static sites](measurements/overhead-v2/native.json) | ~0 ns (-0.04 raw) | 19.84 ns | 10.62 ns | 25.51% |
| [Counting allocator](measurements/overhead-v2/allocation.json) | 0.03 ns | 22.19 ns | 10.92 ns | 46.83% |
| [Tracy static sites](measurements/overhead-v2/tracy.json) | 0.07 ns | 21.60 ns | N/A | 30.99% |

Shipping omits `ara-profile/instrumentation`; `scope!` expands to nothing. With
instrumentation compiled in, the first branch is one relaxed global atomic load,
before any TLS/ring/clock access. The isolated [atomic-gate step](measurements/overhead-v2/atomic-gate.json)
measured 0.45 ns added cost. Native recording uses invariant TSC ticks, calibrated
against the reference clock once per frame; collection converts to nanoseconds.
The non-invariant/non-x86 fallback keeps the monotonic reference clock. TSC reads
bound compiler reordering without CPU instruction-serialization fences, as Tracy
does. Ring cursors/records are TLS-owned; publication atomics occur at boundaries.

Tracy's direct `Client::span` control measured **19.75 ns**, beside the wrapper's
**21.60 ns**; the public Rust/C API on this machine costs more than the 2-10 ns
estimate. Each literal callsite initializes `SpanLocation` once and emits fast
static zones. Tracy replaces native recording, verified by zero native records.
No viewer was attached; zone emission was enabled. `ProfileLayer` also selects
one backend, using zone text for dynamic tracing aliases. The explicit `scope`
function is native-only for callers who need that backend directly.

Incremental samples are retained: [fresh baseline](measurements/overhead-v2/baseline-native.json)
77.14 ns, [atomic gate](measurements/overhead-v2/atomic-gate.json) 74.46 ns,
[TSC](measurements/overhead-v2/tsc-clock.json) 22.13 ns,
[local batches/guards](measurements/overhead-v2/local-ring.json) 19.02 ns, and
[static sites](measurements/overhead-v2/static-callsites.json) 19.71 ns.
A [metadata-cache experiment](measurements/overhead-v2/metadata-cache.json)
measured 28.84 ns under 65.39% background load and was reverted; it did not
establish an improvement. These shared-machine samples are not isolated estimates.

## Local validation

`./crates/ara-bench/run.ps1 -Mode Gate` runs profiler allocation/GPU/Tracy tests,
the real Vulkan correlation test, tests for every instrumented subsystem and the
host/CLI, strict Clippy across all targets, and formatting checks. It scopes its
environment and always uses two Cargo jobs. Pass `-TargetDir` to reuse a cache.

Before this overhead pass, the fixture scene
`ara-host/tests/fixtures/project/levels/arena.scene`, seed 1,
completed four AB/BA rounds of 120 measured ticks after eight warmup ticks per
variant. The 960-frame binary capture and canonical summaries had no dropped
records or hitches. CPU frame p50/p95/p99 were 500/1500/3600 ns; both measured
systems allocated zero requested bytes. A/B comparison passed: frame p95 changed
1100 -> 1500 ns, below the 1000 ns absolute threshold; system p95s were unchanged.
This tiny calibration completed below the OS sampling interval, so its load fields
are explicitly unavailable. The longer overhead runs above include measured load.
