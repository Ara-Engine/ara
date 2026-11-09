//! B4 G6 worker. Reuses T1's fixture, replay and metric contracts verbatim.
#[path = "../../../../../ara-dev/src/lab/stress/contract.rs"]
mod contract;
#[path = "../../../../../ara-render-fidelity/examples/render_v3/coverage.rs"]
mod coverage;
#[path = "../../../../../ara-render-fidelity/examples/render_v3/fixture.rs"]
mod fixture;
#[path = "../../../../../ara-render-fidelity/examples/render_v3/geometry.rs"]
mod geometry;
#[path = "../../../../../ara-render-fidelity/examples/render_v3/metrics.rs"]
mod metrics;
#[path = "../../../../../ara-render-fidelity/examples/render_v3/polar.rs"]
mod polar;
mod reference_cache;
#[path = "../../../../../ara-render-fidelity/examples/render_v3/replay.rs"]
mod replay;
mod runner;
mod showcase;
mod showcase_rig;

fn main() -> anyhow::Result<()> {
    runner::main()
}
