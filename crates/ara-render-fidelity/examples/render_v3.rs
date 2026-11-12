//! Headless render-v3 lab worker. Build once, then let `ara-dev lab gate` invoke it.
#[path = "../../ara-dev/src/lab/stress/contract.rs"]
mod contract;
#[path = "render_v3/coverage.rs"]
mod coverage;
#[path = "render_v3/fixture.rs"]
mod fixture;
#[path = "render_v3/geometry.rs"]
mod geometry;
#[path = "render_v3/polar.rs"]
mod polar;
#[path = "render_v3/replay.rs"]
mod replay;
#[path = "render_v3/runner.rs"]
mod runner;

fn main() -> anyhow::Result<()> {
    let timing = std::env::args().nth(2).as_deref() == Some("uncapped");
    let result = runner::main();
    if timing {
        // The load guard samples CPU once per second. Keep the completed child
        // alive for its final counters, so short replay CPU work is not lost and
        // charged to foreign load. This wait is outside all recorded frame times.
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    result
}
