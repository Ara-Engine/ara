//! Matched, repeated AB/BA overhead measurements; no frame/record allocation in timing.
use ara_bench::LoadSampler;
use ara_profile::{Config, Counters, Memory, Profiler, ThreadRegistration, scope};
use std::{hint::black_box, time::Instant};

#[global_allocator]
static ALLOCATOR: ara_profile::CountingAllocator<std::alloc::System> =
    ara_profile::CountingAllocator(std::alloc::System);

fn run(iterations: u64, instrumented: bool) -> f64 {
    let start = Instant::now();
    for i in 0..iterations {
        if instrumented {
            let _span = scope(black_box("bench.scope"));
            black_box(i);
        } else {
            black_box(i);
        }
    }
    start.elapsed().as_nanos() as f64 / iterations as f64
}
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut load = LoadSampler::new();
    let mut profiler = Profiler::new(Config {
        history: 2,
        ring_capacity: 32768,
        spans_per_frame: 32768,
        ..Config::default()
    })?;
    profiler.register_thread("overhead").attach()?;
    let mut disabled = Vec::new();
    let mut enabled = Vec::new();
    let mut base = Vec::new();
    let mut samples = Vec::new();
    let mut collections = Vec::new();
    for round in 0..21 {
        profiler.stop();
        let (b, d) = if round % 2 == 0 {
            (run(2_000_000, false), run(2_000_000, true))
        } else {
            let d = run(2_000_000, true);
            (run(2_000_000, false), d)
        };
        base.push(b);
        disabled.push(d - b);
        profiler.start();
        profiler.begin_frame()?;
        let (b, e) = if round % 2 == 0 {
            (run(16000, false), run(16000, true))
        } else {
            let e = run(16000, true);
            (run(16000, false), e)
        };
        let collection = Instant::now();
        profiler.end_frame(Counters::default(), Memory::default())?;
        let collection = collection.elapsed().as_nanos() as f64 / 16000.0;
        collections.push(collection);
        enabled.push(e - b);
        samples.push(
            serde_json::json!({"round":round,"enabled_baseline_ns":b,"enabled_added_ns":e-b,"collection_ns_per_span":collection,
            "disabled_baseline_ns":base.last(),"disabled_added_ns":disabled.last()}),
        );
    }
    ThreadRegistration::detach();
    let value = serde_json::json!({"rounds":21,"baseline_ns":median(base),"disabled_added_ns":median(disabled),"enabled_added_ns":median(enabled),
        "collection_ns_per_span":median(collections),
        "tracy":cfg!(feature="tracy"),"allocation":cfg!(feature="allocation"),"load":load.sample(),
        "tracy_connected":ara_profile::tracy_connected(),"samples":samples,
        "platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,
        "logical_cpus":std::thread::available_parallelism().map(|n|n.get()).ok(),
        "note":"Release build, median paired differences; Tracy emits zones even without a viewer (not on-demand). Enabled spans include two clock reads and bounded ring publication."});
    print!("{}", ara_json::to_canonical_string(&value));
    Ok(())
}
