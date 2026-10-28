//! Opt-in measurement of the additional scalar output-health observation.
use super::*;
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "Opt-in CPU measurement; writes a canonical target artifact"]
fn benchmark_output_health_observation() {
    let sample = Some(Sample {
        requested: true,
        submissions: 1,
        presentations: 0,
        offscreen: true,
    });
    let measure = |enabled: bool| {
        let mut health = Health::new(Duration::from_secs(10));
        let mut submissions = 0_u64;
        let began = Instant::now();
        for _ in 0..100_000 {
            let sample = black_box(sample);
            // These submission counters already existed before output health.
            submissions += u64::from(sample.unwrap().submissions);
            if enabled {
                black_box(&mut health).record(black_box(Duration::from_millis(16)), sample);
            }
            black_box(submissions);
        }
        let elapsed = began.elapsed().as_nanos() as f64 / 100_000.;
        assert_eq!(submissions, 100_000);
        assert_eq!(health.stalls(), 0);
        black_box(health);
        elapsed
    };
    measure(false);
    measure(true);
    let mut before = Vec::new();
    let mut after = Vec::new();
    for round in 0..9 {
        for enabled in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let elapsed = measure(enabled);
            if enabled {
                after.push(elapsed);
            } else {
                before.push(elapsed);
            }
        }
    }
    let median = |values: &[f64]| {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let output = crate::test_target().join("editor-monkey-output-health-benchmarks");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join(format!("observation-{}.json", std::process::id())),
        ara_json::to_canonical_string(&json!({
            "schema":1,"scope":"CPU-only scalar observation; excludes native event handling, GPU waits and evidence serialization",
            "baseline":"Existing submission scalar count, health disabled", "current":"Same count plus actual Health::record on successful offscreen submissions",
            "productionSourceBlake3":blake3::hash(include_str!("../output_health.rs").as_bytes()).to_hex().to_string(),
            "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"debugAssertions":cfg!(debug_assertions),
            "iterationsPerRound":100000,"rounds":9,"beforeNsPerObservation":before,"afterNsPerObservation":after,
            "beforeMedianNsPerObservation":median(&before),"afterMedianNsPerObservation":median(&after),
        }))).unwrap();
}
