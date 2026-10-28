//! Explicit durations exercise starvation without GPU work, waiting or sleeps.
use super::*;

fn requested() -> Option<Sample> {
    Some(Sample {
        requested: true,
        submissions: 0,
        presentations: 0,
        offscreen: true,
    })
}
fn output() -> Option<Sample> {
    Some(Sample {
        submissions: 1,
        ..requested().unwrap()
    })
}
fn stalled(health: &mut Health) {
    health.record(Duration::from_secs(5), requested());
    health.record(Duration::from_secs(5), requested());
}

#[test]
fn cpu_only_and_single_native_sample_remain_unknown() {
    let mut health = Health::new(Duration::from_secs(10));
    health.record(Duration::from_secs(60), None);
    assert_eq!(health.report()["observed"], false);
    assert_eq!(health.report()["healthy"], Value::Null);
    assert_eq!(health.stalls(), 0);
    // One slow frame is the existing frame/watchdog signal, not repeated-output evidence.
    health.record(Duration::from_secs(10), requested());
    assert_eq!(health.report()["healthy"], Value::Null);
    assert_eq!(health.stalls(), 0);
    health.record(Duration::ZERO, requested());
    assert_eq!(health.report()["observed"], true);
    assert_eq!(health.stalls(), 1);
}

#[test]
fn threshold_opens_one_interval_and_only_real_output_recovers_it() {
    let mut health = Health::new(Duration::from_secs(10));
    health.record(Duration::from_millis(9999), requested());
    assert_eq!(health.stalls(), 0);
    health.record(Duration::from_millis(1), requested());
    assert_eq!(health.stalls(), 1);
    health.record(Duration::from_secs(60), requested());
    let report = health.report();
    assert_eq!(report["stalls"], 1);
    assert_eq!(report["intervals"][0]["firstRequestedFrame"], 1);
    assert_eq!(report["intervals"][0]["detectedFrame"], 2);
    assert_eq!(report["intervals"][0]["noOutputRequestedMs"], 70000.);
    health.record(Duration::from_millis(16), output());
    let report = health.report();
    assert_eq!(report["activeStall"], false);
    assert_eq!(report["recoveredIntervals"], 1);
    assert_eq!(report["intervals"][0]["endReason"], "submission");
    assert_eq!(
        report["healthy"], false,
        "Recovery cannot erase the historical fault"
    );
    stalled(&mut health);
    assert_eq!(health.stalls(), 2);
}

#[test]
fn idle_resets_a_wait_without_claiming_recovery_or_bridging_request_windows() {
    let mut health = Health::new(Duration::from_secs(10));
    health.record(Duration::from_secs(8), requested());
    health.record(
        Duration::from_secs(60),
        Some(Sample {
            requested: false,
            ..requested().unwrap()
        }),
    );
    health.record(Duration::from_secs(5), requested());
    assert_eq!(health.stalls(), 0);
    health.record(Duration::from_secs(5), requested());
    assert_eq!(health.stalls(), 1);
    health.record(
        Duration::from_secs(60),
        Some(Sample {
            requested: false,
            ..requested().unwrap()
        }),
    );
    let report = health.report();
    assert_eq!(report["idleResetIntervals"], 1);
    assert_eq!(report["recoveredIntervals"], 0);
    assert_eq!(report["intervals"][0]["endReason"], "not-requested");
    assert_eq!(report["intervals"][0]["noOutputRequestedMs"], 10000.);
    assert_eq!(report["activeStall"], false);
}

#[test]
fn lost_observation_is_unknown_and_does_not_recover_or_extend_an_interval() {
    let mut health = Health::new(Duration::from_secs(10));
    stalled(&mut health);
    health.record(Duration::from_secs(120), None);
    let report = health.report();
    assert_eq!(report["latest"], "unknown");
    assert_eq!(report["unknownResetIntervals"], 1);
    assert_eq!(report["recoveredIntervals"], 0);
    assert_eq!(report["intervals"][0]["endReason"], "unknown-sample");
    assert_eq!(report["intervals"][0]["noOutputRequestedMs"], 10000.);
    health.record(Duration::from_secs(9), requested());
    assert_eq!(health.stalls(), 1);
    health.record(Duration::from_secs(1), requested());
    assert_eq!(health.stalls(), 2);
}

#[test]
fn presenting_old_images_does_not_satisfy_fresh_output_requests() {
    let mut health = Health::new(Duration::from_secs(10));
    let old_image = Some(Sample {
        presentations: 1,
        offscreen: false,
        ..requested().unwrap()
    });
    health.record(Duration::from_secs(5), old_image);
    health.record(Duration::from_secs(5), old_image);
    assert_eq!(health.stalls(), 1);
    assert_eq!(health.report()["nativePresentations"], 2);
    assert_eq!(health.report()["gpuSurfaceSubmissions"], 0);
    health.record(Duration::ZERO, output());
    assert_eq!(health.report()["recoveredIntervals"], 1);
}

#[test]
fn interval_retention_is_bounded_but_fault_counts_are_not_truncated() {
    let mut health = Health::new(Duration::from_secs(10));
    for _ in 0..35 {
        stalled(&mut health);
        health.record(Duration::ZERO, output());
    }
    let report = health.report();
    assert_eq!(report["stalls"], 35);
    assert_eq!(report["recoveredIntervals"], 35);
    assert_eq!(
        report["intervals"].as_array().unwrap().len(),
        RETAINED_INTERVALS
    );
    assert_eq!(report["intervalsOmitted"], 3);
    assert_eq!(report["intervals"][0]["firstRequestedFrame"], 10);
    assert_eq!(report["healthy"], false);
}

#[test]
fn native_runner_reports_starvation_and_preserves_max_based_hang_lower_bound() {
    use super::super::super::responsiveness;
    use super::super::Monkey;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let output = crate::test_target()
        .join("monkey-output-health")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    let mut runner = Monkey::new(1, Duration::from_secs(30), Some(1), &output).unwrap();
    runner.record_frame(Duration::from_secs(11));
    let mut sample = responsiveness::Sample {
        output_requested: true,
        offscreen_output: true,
        ..Default::default()
    };
    for _ in 0..2 {
        runner.record_frame_with_stages(Duration::from_secs(5), &sample);
    }
    sample.gpu_surface_submissions = 1;
    runner.record_frame_with_stages(Duration::ZERO, &sample);
    sample.gpu_surface_submissions = 0;
    for _ in 0..2 {
        runner.record_frame_with_stages(Duration::from_secs(5), &sample);
    }
    let report: Value = serde_json::from_str(&runner.report_text(false).unwrap()).unwrap();
    assert_eq!(report["longFrames"], 1);
    assert_eq!(report["gpuOutputStalls"], 2);
    assert_eq!(
        report["hangs"], 2,
        "Overlapping fault signals are not summed"
    );
    assert_eq!(report["gpuOutputHealth"]["observed"], true);
    assert_eq!(report["gpuOutputHealth"]["healthy"], false);
    drop(runner);
    std::fs::remove_dir_all(output).unwrap();
}
