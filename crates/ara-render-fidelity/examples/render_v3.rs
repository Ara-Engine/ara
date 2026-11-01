//! Headless render-v3 lab worker. Build once, then let `ara-dev lab gate` invoke it.
#[path = "../../ara-dev/src/lab/stress/contract.rs"]
mod contract;
#[path = "render_v3/coverage.rs"]
mod coverage;
#[path = "render_v3/fixture.rs"]
mod fixture;
#[path = "render_v3/geometry.rs"]
mod geometry;
#[path = "render_v3/metrics.rs"]
mod metrics;
#[path = "render_v3/polar.rs"]
mod polar;
#[path = "render_v3/replay.rs"]
mod replay;
#[path = "render_v3/runner.rs"]
mod runner;

fn main() -> anyhow::Result<()> {
    runner::main()
}
