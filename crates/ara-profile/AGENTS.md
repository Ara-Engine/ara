# ara-profile

Bounded CPU/GPU frame inspection for the editor, agents and local perf passes.
No gameplay state. No implicit subscriber, allocator, worker or viewer startup.

## API

- `Profiler::new(Config)`: preallocates rolling frame/CPU/GPU/job storage.
- `register_thread(name)` then move the endpoint to its worker and `attach()` once.
  `ThreadRegistration::detach()` only after scopes exit; never share a producer.
- `start` / `stop`, `begin_frame` -> `FrameToken`, `end_frame(Counters, Memory)`.
- `scope("static.name")`: native RAII interval. No allocation or locks on entry/exit.
- `ProfileLayer`: composable tracing subscriber layer, including Bevy's `name` field.
  Tracing span creation/name registration is a cold path; reuse spans in hot loops.
- `FrameToken::in_scope`: registered worker work retains its originating frame.
  Tokens reject other captures. `collect` never waits for a worker/GPU/job.
- `gpu_timings(frame, &[PassTiming])` (`gpu` feature): imports ara-gpu timestamps.
  Render integration: keep the submission frame ID with the readback; do not use
  the frame in which a delayed readback is collected. Durations do not align clocks.
- `job_activity`: separate worker/process elapsed activity, not CPU self time.
- `profile()`: borrowed thread timeline, parent edges, pass list, jobs and counters.
- `Profile::summary(SummaryBudget)` / `hitches`; frame-level p50/p95/p99 by label.
- `write_capture` / `read_capture`: versioned postcard trace + canonical JSON summary.
  Disk IO and summaries belong on a control/background thread, never a tick worker.
- `write_summary` / `read_summary` / `compare`: local D9 regression baselines.
  CLI: `ara profile compare baseline.json current.json`; exit 0 pass, 1 fail, 2 error.
- Host registers `profile.start/stop/summary/hitches/compare`, MCP tool `ara_profile`.
  `Host::profiler()` lets presentation own frames; host ticks reuse open frames.

## Features / cost

Default: recording runtime-disabled. Subsystem `profiling` features default off,
so shipping builds have no subsystem marker code. Native producers use a bounded
SPSC ring and fixed nesting stack, never an `Arc<Mutex<_>>` around frame state.
Labels are interned on first observation. Warm native recording, GPU/job imports
and frame collection reuse allocated storage (covered by the allocation test).

`allocation`: executable installs `CountingAllocator<System>` explicitly with
`#[global_allocator]`. Counts requested bytes, reallocations and live requested
heap bytes; native/C#/driver heaps need their owning subsystem's memory counters.
Per-scope attribution is thread-local requested bytes, including/excluding children.

`tracy`: links tracy-client's vendored C++ through cc; never CMake. Zones and frame
marks are emitted even without a viewer. Shipping omits this feature. Trace/viewer
startup and Tracy's own storage are outside the native allocation-free guarantee.

## Semantics

CPU spans measure monotonic elapsed intervals, including preemption/waits.
CPU frame wall time is independent of overlapping worker durations. GPU passes
remain separate by queue; no sum is claimed to be a GPU critical path. Scope IDs
are unique per registered thread; parent IDs are resolved within that thread.
Hitches use CPU frame wall time or an individual GPU pass over the frame budget.
Counters/VRAM categories are supplied by their owning managers, never fabricated.
Late results update retained frames; evicted results increment `orphaned_results`.
Overflow/incomplete captures cannot pass a regression check. Percentiles include
zero for frames in which a known label is inactive. Comparisons use full metric
sets; top-N/budget-truncated metric reports cannot serve as passing baselines.

## Gates

Set `CARGO_BUILD_JOBS=2`. `cargo test -p ara-profile --features allocation,gpu`;
`cargo test -p ara-profile --all-features`;
`cargo clippy -p ara-profile --all-targets --all-features -- -D warnings`;
`cargo fmt -p ara-profile -- --check`.
Explicit hardware gate: `cargo test -p ara-profile --features gpu --test gpu -- --ignored`.
Vulkan 1.3/validation required; scope VK_IMPLICIT_LAYER_PATH to an empty directory
if this machine's third-party implicit layers inject loader noise.

Measurements and scenario conventions live in `../ara-bench/README.md`.
