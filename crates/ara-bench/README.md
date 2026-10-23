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
    let _span = ara_profile::scope("extract");
    // Extract ECS state and submit render work.
}
// Keep frame.id() with the GPU submission/readback.
profiler.gpu_timings(frame.id(), renderer.timings()); // feature `gpu`
profiler.end_frame(counters, memory)?;
```

Register each task-pool worker during setup. Move a `ThreadRegistration` onto
that worker and attach it once. Move the copyable `FrameToken` with queued work,
then call `token.in_scope(|| { let _span = scope("job.name"); ... })` there.
Workers never lock frame history, allocate records, wait for the host or publish
authoring mutations. Collect at presentation boundaries; late data updates the
original frame while retained. Stop producers before saving a complete capture.

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
Build the CLI/daemon with `--features ara-profile/allocation` to enable its installed
counting allocator. Other engine executables choose their own global allocator and
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
cargo run -p ara-bench --release --features allocation --example scenario -- script.json result.araprof
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

## Overhead measurements

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
