# ara-profile

Bounded CPU/GPU frame inspection for the editor, agents and local perf passes.
No gameplay state. No implicit subscriber, allocator, worker or viewer startup.

## API

- `Profiler::new(Config)`: preallocates rolling frame/CPU/GPU/job storage.
- `register_thread(name)` then move the endpoint to its worker and `attach()` once.
  `ThreadRegistration::detach()` only after scopes exit; never share a producer.
- `start` / `stop`, `begin_frame` -> `FrameToken`, `end_frame(Counters, Memory)`.
- `scope!("static.name")`: lexical scope with static callsite metadata. Compiles
  away without `instrumentation`; subsystem `profiling` features opt in.
- `callsite!` / `scope_at`: same backend with an explicit guard lifetime.
- `scope("static.name")`: explicit native-only guard, including in Tracy builds.
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

Default: scope macros compile away. Subsystem `profiling` features default off.
With instrumentation compiled in, the stopped path checks one global relaxed
atomic before any TLS access. Native producers use a bounded SPSC ring and fixed
nesting stack, never an `Arc<Mutex<_>>` around frame state.
Labels are interned on first observation. Warm native recording, GPU/job imports
and frame collection reuse allocated storage (covered by the allocation test).
Scope edges only update TLS-owned cursors and records, with no ring atomics.
`FrameToken::in_scope`, collection and detach publish batches. Workers with custom
loops must call `ThreadRegistration::flush()` at frame/job boundaries. Full local
buffers drop records until publication; losses remain explicit and bounded.

Invariant x86-64 TSC ticks replace per-edge reference-clock calls; frequency is
calibrated once per frame and conversion happens during collection. Cold setup
uses a 2 ms reference interval. Reads use compiler barriers, not CPU instruction
serialization, matching Tracy's counter style. Other CPUs use the monotonic clock.

`allocation`: executable installs `CountingAllocator<System>` explicitly with
`#[global_allocator]`. Counts requested bytes, reallocations and live requested
heap bytes; native/C#/driver heaps need their owning subsystem's memory counters.
Per-scope attribution is thread-local requested bytes, including/excluding children.

`tracy`: links tracy-client's vendored C++ through cc; never CMake. Zones and frame
marks are emitted even without a viewer. Shipping omits this feature. Trace/viewer
startup and Tracy's own storage are outside the native allocation-free guarantee.
It selects Tracy instead of native recording for scope macros and ProfileLayer.
Literal sites cache `SpanLocation` once and use `Client::span`; dynamic tracing
aliases use a static adapter zone with alias text. Agent CPU summaries use native
mode; the explicit native-only `scope` function remains available in either mode.

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
