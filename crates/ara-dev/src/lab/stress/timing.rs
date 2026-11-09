//! Classify whole worker intervals using the existing D26 guard's load policy.
use crate::gpu_guard::{Interval, Policy, Report};
use anyhow::{Context, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Measurement metadata only; the deterministic replay never reads this clock.
pub fn unix_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()
        .context("measurement wall clock exceeds u64 milliseconds")
}

/// Let already-running builders finish before measurement. Never retry after any
/// case started, after a non-load failure, or beyond the bounded admission period.
pub fn retry_admission(result: &Result<()>, attempted: usize, elapsed: Duration) -> bool {
    attempted == 0
        && elapsed < Duration::from_secs(180)
        && result.as_ref().is_err_and(crate::gpu_guard::is_invalid)
}

/// A clean case is independent of another case's contamination. Coverage includes
/// startup and teardown, not just selected fast frames. Unknown evidence fails closed.
pub fn quiet(report: &Report, interval: Interval) -> bool {
    if interval.start_unix_ms >= interval.end_unix_ms
        || report.sampler.is_empty()
        || report.sampler == "test_fixture"
        || report.classification_version < 2
        || report.policy != Policy::default()
        || report.cpu.threshold_percent != Policy::default().cpu_threshold_percent
        || report.cpu.sustained_samples != Policy::default().cpu_sustained_samples
        || !report.errors.is_empty()
        || !report.cpu.errors.is_empty()
        || report.samples.is_empty()
        || report.overlaps(interval)
        || report.monitoring.overlaps(interval)
    {
        return false;
    }
    let (Some(start), Some(end), Some(gpu_first), Some(gpu_last), Some(cpu_first), Some(cpu_last)) = (
        report.monitoring.start_unix_ms,
        report.monitoring.end_unix_ms,
        report.samples.first(),
        report.samples.last(),
        report.cpu.samples.first(),
        report.cpu.samples.last(),
    ) else {
        return false;
    };
    start <= interval.start_unix_ms
        && end >= interval.end_unix_ms
        && gpu_first.interval_start_unix_ms <= interval.start_unix_ms
        && gpu_last.unix_ms >= interval.end_unix_ms
        && cpu_first.interval.start_unix_ms <= interval.start_unix_ms
        && cpu_last.interval.end_unix_ms >= interval.end_unix_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_retries_only_load_refusals_before_work_and_within_its_budget() {
        let busy = Err(crate::gpu_guard::invalid_error("background load".into()));
        assert!(retry_admission(&busy, 0, Duration::from_secs(10)));
        assert!(!retry_admission(&busy, 1, Duration::from_secs(10)));
        assert!(!retry_admission(&busy, 0, Duration::from_secs(180)));
        assert!(!retry_admission(&Ok(()), 0, Duration::ZERO));
        assert!(!retry_admission(
            &Err(anyhow::anyhow!("worker or queue failure")),
            0,
            Duration::ZERO
        ));
    }

    fn interval(start: u64, end: u64) -> Interval {
        Interval {
            start_unix_ms: start,
            end_unix_ms: end,
        }
    }

    fn report() -> Report {
        serde_json::from_value(serde_json::json!({
            "sampler":"windows_pdh", "classification_version":2,
            "samples":[{"unix_ms":100,"interval_start_unix_ms":0,"foreign_percent":0.,"foreign":[]}],
            "contaminants":[], "errors":[], "quiet_wait_seconds":0.,
            "monitoring":{"start_unix_ms":0,"end_unix_ms":100},
            "cpu":{"samples":[{"interval":{"start_unix_ms":0,"end_unix_ms":100},
                "total_percent":0.,"protected_percent":0.,"foreign_percent":0.,"foreign":[]}],
                "intervals":[],"threshold_percent":10.,"sustained_samples":3,"errors":[]}
        }))
        .unwrap()
    }

    #[test]
    fn contamination_rejects_the_whole_overlapping_case_only() {
        let mut guard = report();
        assert!(quiet(&guard, interval(10, 20)));
        guard.cpu.intervals.push(interval(50, 60));
        assert!(guard.invalid());
        assert!(quiet(&guard, interval(10, 20)));
        assert!(!quiet(&guard, interval(20, 70)));
        assert!(!quiet(&guard, interval(60, 80)));
        assert!(quiet(&guard, interval(61, 80)));
        guard.intervals.push(interval(10, 20));
        assert!(!quiet(&guard, interval(10, 20)));
    }

    #[test]
    fn unknown_coverage_and_changed_policy_cannot_certify_a_case() {
        let base = report();
        let case = interval(10, 20);
        assert!(!quiet(&base, interval(10, 101)));
        assert!(!quiet(&base, interval(20, 10)));
        let mut guard = base.clone();
        guard.cpu.samples.clear();
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.samples[0].unix_ms = 19;
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.cpu.samples[0].interval.end_unix_ms = 19;
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.classification_version = 0;
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.monitoring.end_unix_ms = None;
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.cpu.errors.push("CPU counters unavailable".into());
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.errors.push("GPU counters unavailable".into());
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.policy.cpu_threshold_percent = 30.;
        assert!(!quiet(&guard, case));
        guard = base.clone();
        guard.sampler = "test_fixture".into();
        assert!(!quiet(&guard, case));
        guard = base;
        guard.monitoring.gaps = serde_json::from_value(serde_json::json!([
            {"start_unix_ms":15,"end_unix_ms":25,"reason":"missing samples"}
        ]))
        .unwrap();
        assert!(!quiet(&guard, case));
        assert!(quiet(&guard, interval(30, 40)));
    }
}
