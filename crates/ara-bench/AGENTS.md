# ara-bench

Shared runtime performance convention. Distinct from `arabench`, which measures
agent ergonomics and runs agent CLIs. This crate never launches an agent.

## API

- `Scenario { reset, frame, memory }`: reset/load outside timing; frame runs one
  fixed tick or presentation frame. Keep gameplay state in the scenario ECS world.
- `interleave(a, b, profiler, ScenarioConfig)`: matched AB/BA rounds, each with reset,
  warmup and measured frames. Register/attach/start the profiler before calling.
  `RunSample` preserves variant, order, inclusive frame range and background load.
- `LoadSampler`: process/system/competing CPU load; no sampler thread. Too-short
  intervals return explicit missing values instead of fabricated zero utilization.
- `examples/scenario.rs`: strict JSON script -> real project scene -> ara-sim ticks
  -> versioned trace, canonical JSON summary, A/B baselines and run/load records.
- `benches/scopes.rs`: Divan kernel convention; use `#[divan::bench]` and
  `harness = false`. No exact version pins or build-time scripting dependencies.
- `examples/overhead.rs`: paired release measurements for disabled/enabled spans,
  optional Tracy and allocator modes. Measurement output includes background CPU.

## Gates

Always `CARGO_BUILD_JOBS=2`. `cargo test -p ara-bench`;
`cargo clippy -p ara-bench --all-targets --all-features -- -D warnings`;
`cargo fmt -p ara-bench -- --check`.
Explicit measurements: `cargo run -p ara-bench --release --example overhead` and
again with `--features tracy` / `--features allocation`.
`cargo bench -p ara-bench --bench scopes` runs the Divan microbenchmarks.

Keep file IO, load sampling and report generation outside measured frames.
Do not mix A/B frames into one baseline; split using RunSample frame ranges.
Size history for all required frames; otherwise only the rolling tail is retained.
See README.md for scripts, integration and recorded overhead.
