#![allow(clippy::unwrap_used)]
use ara_profile::*;

fn profile() -> Profile {
    Profile {
        version: 1,
        budget_ns: 1000,
        labels: vec!["system".into(), "pass".into(), "compute".into()],
        frames: (1..=100)
            .map(|id| Frame {
                id,
                duration_ns: id * 100,
                spans: vec![CpuSpan {
                    label: 0,
                    thread: 0,
                    id,
                    parent: 0,
                    start_ns: 0,
                    duration_ns: id * 100,
                    self_ns: id * 50,
                    allocated_bytes: 0,
                    self_allocated_bytes: 0,
                }],
                gpu: vec![GpuPass {
                    label: 1,
                    queue: 2,
                    duration_ns: id * 20,
                }],
                ..Frame::default()
            })
            .collect(),
        ..Profile::default()
    }
}
fn full_summary(p: &Profile) -> Summary {
    p.summary(SummaryBudget {
        top_n: 1024,
        hitches: 1024,
        max_bytes: 1_048_576,
    })
    .unwrap()
}
#[test]
fn percentiles_hitches_budget_and_flame_data() {
    let p = profile();
    let s = full_summary(&p);
    assert_eq!(
        (s.cpu_frame.p50, s.cpu_frame.p95, s.cpu_frame.p99),
        (5000, 9500, 9900)
    );
    assert_eq!(s.metrics[0].time.p95, 4750);
    assert_eq!(s.metrics[0].inclusive.p95, 9500);
    assert_eq!(s.hitch_count, 90);
    assert_eq!(s.hitches[0].frame, 100);
    assert!(s.hitches[0].hints[0].contains("cpu/system"));
    let small = p
        .summary(SummaryBudget {
            top_n: 2,
            hitches: 5,
            max_bytes: 512,
        })
        .unwrap();
    assert!(serde_json::to_vec(&small).unwrap().len() <= 512);
    assert!(small.truncated);
    assert!(
        p.summary(SummaryBudget {
            max_bytes: 1,
            ..SummaryBudget::default()
        })
        .is_err()
    );
}
#[test]
fn gpu_hitch_is_detected_without_summing_parallel_passes() {
    let mut p = profile();
    p.frames.truncate(1);
    p.frames[0].duration_ns = 500;
    p.frames[0].gpu = vec![
        GpuPass {
            label: 1,
            queue: 2,
            duration_ns: 600
        };
        2
    ];
    assert!(p.hitches(5).is_empty());
    p.frames[0].gpu[0].duration_ns = 1001;
    assert_eq!(p.hitches(5).len(), 1);
}
#[test]
fn compare_fails_regressions_missing_metrics_empty_and_incomplete_evidence() {
    let baseline = full_summary(&profile());
    let mut current = baseline.clone();
    assert!(compare(&baseline, &current, 0.05, 10).unwrap().passed);
    current.metrics[0].time.p95 *= 2;
    let report = compare(&baseline, &current, 0.05, 10).unwrap();
    assert!(!report.passed);
    assert!(report.table().contains("REGRESSION"));
    current = baseline.clone();
    current.metrics.pop();
    assert!(!compare(&baseline, &current, 0.05, 10).unwrap().passed);
    current = baseline.clone();
    current.frames = 0;
    assert!(!compare(&baseline, &current, 0.05, 10).unwrap().passed);
    current = baseline.clone();
    current.dropped = 1;
    assert!(!compare(&baseline, &current, 0.05, 10).unwrap().passed);
    assert!(compare(&baseline, &baseline, f64::NAN, 0).is_err());
}
#[test]
fn versioned_binary_and_canonical_summary_round_trip() {
    let p = profile();
    let path = std::env::temp_dir().join(format!("ara-profile-{}.araprof", std::process::id()));
    write_capture(&path, &p).unwrap();
    let read = read_capture(&path).unwrap();
    assert_eq!(read.frames.len(), 100);
    assert_eq!(read.frames[99].gpu[0].duration_ns, 2000);
    let text = std::fs::read_to_string(path.with_extension("json")).unwrap();
    let value = ara_json::parse(&text).unwrap();
    assert_eq!(text, ara_json::to_canonical_string(&value));
    assert_eq!(
        read_summary(&path.with_extension("json"))
            .unwrap()
            .cpu_frame
            .p95,
        9500
    );
    std::fs::write(&path, b"invalid").unwrap();
    assert!(read_capture(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(path.with_extension("json")).unwrap();
}
