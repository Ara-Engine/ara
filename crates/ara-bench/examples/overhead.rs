//! Matched, repeated AB/BA overhead measurements; no frame/record allocation in timing.
use ara_bench::LoadSampler;
use ara_profile::{Config, Counters, Memory, Profiler, ThreadRegistration};
use std::{hint::black_box, time::Instant};

const DISABLED_ITERATIONS: u64 = 100_000_000;
const ENABLED_ITERATIONS: u64 = 16_000;

#[global_allocator]
static ALLOCATOR: ara_profile::CountingAllocator<std::alloc::System> =
    ara_profile::CountingAllocator(std::alloc::System);

fn run(iterations: u64, instrumented: bool) -> f64 {
    let start = Instant::now();
    for i in 0..iterations {
        if instrumented {
            ara_profile::scope!("bench.scope");
            black_box(i);
        } else {
            black_box(i);
        }
    }
    start.elapsed().as_nanos() as f64 / iterations as f64
}
#[cfg(feature = "tracy")]
fn run_tracy(iterations: u64, instrumented: bool) -> f64 {
    let start = Instant::now();
    for i in 0..iterations {
        if instrumented {
            let client =
                ara_profile::__tracy::Client::running().expect("Profiler::start started Tracy");
            let _span = client.span(ara_profile::__tracy::span_location!("bench.direct"), 0);
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
    let mut tracy_direct = Vec::new();
    let mut recorded = 0;
    for round in 0..21 {
        profiler.stop();
        let (b, d) = if round % 2 == 0 {
            (
                run(DISABLED_ITERATIONS, false),
                run(DISABLED_ITERATIONS, true),
            )
        } else {
            let d = run(DISABLED_ITERATIONS, true);
            (run(DISABLED_ITERATIONS, false), d)
        };
        base.push(b);
        disabled.push(d - b);
        profiler.start();
        profiler.begin_frame()?;
        let (b, e) = if round % 2 == 0 {
            (
                run(ENABLED_ITERATIONS, false),
                run(ENABLED_ITERATIONS, true),
            )
        } else {
            let e = run(ENABLED_ITERATIONS, true);
            (run(ENABLED_ITERATIONS, false), e)
        };
        #[cfg(feature = "tracy")]
        let direct_added = {
            let (baseline, direct) = if round % 2 == 0 {
                (
                    run_tracy(ENABLED_ITERATIONS, false),
                    run_tracy(ENABLED_ITERATIONS, true),
                )
            } else {
                let direct = run_tracy(ENABLED_ITERATIONS, true);
                (run_tracy(ENABLED_ITERATIONS, false), direct)
            };
            tracy_direct.push(direct - baseline);
            Some(direct - baseline)
        };
        #[cfg(not(feature = "tracy"))]
        let direct_added: Option<f64> = None;
        let collection = Instant::now();
        profiler.end_frame(Counters::default(), Memory::default())?;
        let collection = collection.elapsed().as_nanos() as f64 / ENABLED_ITERATIONS as f64;
        if let Some(frame) = profiler.profile().frames.back() {
            if frame.dropped != 0 {
                return Err("benchmark lost records".into());
            }
            recorded = frame.spans.len();
            if (ara_profile::scope_backend() == "native" && recorded != ENABLED_ITERATIONS as usize)
                || (ara_profile::scope_backend() != "native" && recorded != 0)
            {
                return Err("benchmark backend recorded the wrong number of native spans".into());
            }
        }
        collections.push(collection);
        enabled.push(e - b);
        samples.push(
            serde_json::json!({"round":round,"enabled_baseline_ns":b,"enabled_added_ns":e-b,"collection_ns_per_span":collection,
            "disabled_baseline_ns":base.last(),"disabled_added_ns":disabled.last(),"tracy_direct_ns":direct_added}),
        );
    }
    ThreadRegistration::detach();
    let value = serde_json::json!({"rounds":21,"disabled_iterations":DISABLED_ITERATIONS,"enabled_iterations":ENABLED_ITERATIONS,"baseline_ns":median(base),"disabled_added_ns":median(disabled),"enabled_added_ns":median(enabled),
        "collection_ns_per_span":if recorded==0 {None} else {Some(median(collections))},
        "tracy":cfg!(feature="tracy"),"allocation":cfg!(feature="allocation"),"load":load.sample(),"clock":profiler.clock_source(),"backend":ara_profile::scope_backend(),"native_records_per_round":recorded,
        "tracy_direct_ns": if tracy_direct.is_empty() { None } else { Some(median(std::mem::take(&mut tracy_direct))) },
        "tracy_connected":ara_profile::tracy_connected(),"samples":samples,
        "platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,
        "logical_cpus":std::thread::available_parallelism().map(|n|n.get()).ok(),
        "note":"Release build, 21 paired AB/BA differences. Static scopes select one backend. Shipping expands to nothing; runtime-disabled scopes use one relaxed load; native ticks convert at collection; Tracy uses cached source locations, without a viewer or native double recording."});
    print!("{}", ara_json::to_canonical_string(&value));
    Ok(())
}
