//! S6's exact 5k/20k bowl workload; run with CARGO_BUILD_JOBS=2.
#[path = "../tests/support/mod.rs"]
mod support;
use ara_physics::*;
use std::time::Instant;
use support::*;

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() as f64 * p).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1)]
}
fn run(
    count: usize,
    threads: u32,
    steps: usize,
) -> std::result::Result<serde_json::Value, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut w = world(threads, count as u32 + 16);
    bowl(&mut w, Vec3::ZERO, 25.0, 12.0);
    let ids = grid(&mut w, count, Vec3::Y * 13.0, 50, 7);
    w.backend_mut().optimize()?;
    let setup_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut samples = Vec::with_capacity(steps);
    let mut extraction = Vec::new();
    let mut states = Vec::with_capacity(count);
    for tick in 0..steps {
        let start = Instant::now();
        w.step()?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        if tick % 10 == 0 {
            let start = Instant::now();
            w.backend_mut().read_bodies(&ids, &mut states)?;
            extraction.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    w.backend_mut().read_bodies(&ids, &mut states)?;
    let escaped = states
        .iter()
        .filter(|s| s.pose.position[1] < -1.0 || !s.pose.position.iter().all(|v| v.is_finite()))
        .count();
    if escaped != 0 {
        return Err(format!("{escaped} bodies escaped the bowl").into());
    }
    let start = Instant::now();
    let snapshot = w.snapshot()?;
    let save_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    w.restore(&snapshot)?;
    let restore_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut warm_save = Vec::new();
    let mut warm_restore = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let checkpoint = w.snapshot()?;
        warm_save.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        w.restore(&checkpoint)?;
        warm_restore.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(
        serde_json::json!({"bodies":count,"threads":threads,"steps":steps,"deterministic":w.backend().deterministic(),
        "velocitySteps":w.config().velocity_steps,"setupMs":setup_ms,"p50Ms":percentile(&mut samples,0.5),"p95Ms":percentile(&mut samples,0.95),
        "extractP50Ms":percentile(&mut extraction,0.5),"activeAtEnd":w.backend().active_count(),"escaped":escaped,
        "nativeSnapshotBytes":snapshot.native().byte_len(),"saveMs":save_ms,"restoreMs":restore_ms,
        "warmSaveP50Ms":percentile(&mut warm_save,0.5),"warmSaveP95Ms":percentile(&mut warm_save,0.95),
        "warmRestoreP50Ms":percentile(&mut warm_restore,0.5),"warmRestoreP95Ms":percentile(&mut warm_restore,0.95)}),
    )
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let steps = std::env::var("ARA_PHYSICS_BENCH_STEPS")
        .ok()
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(600);
    if steps < 2 {
        return Err("benchmark requires at least two ticks".into());
    }
    let threads: Vec<u32> = std::env::var("ARA_PHYSICS_BENCH_THREADS")
        .unwrap_or_else(|_| "1,4,8".into())
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    let mut records = Vec::new();
    for count in [5000, 20000] {
        for &threads in &threads {
            let record = run(count, threads, steps)?;
            print!(
                "{}",
                ara_json::to_canonical_string(&ara_json::sort_keys(&record))
            );
            records.push(record);
        }
    }
    if let Some(path) = std::env::var_os("ARA_PHYSICS_BENCH_OUTPUT") {
        let artifact = serde_json::json!({"scenario":"S6 bowl: mixed boxes/spheres, 15 velocity / 2 position steps, contact collection disabled",
            "architecture":std::env::consts::ARCH,"os":std::env::consts::OS,"records":records});
        std::fs::write(
            path,
            ara_json::to_canonical_string(&ara_json::sort_keys(&artifact)),
        )?;
    }
    Ok(())
}
