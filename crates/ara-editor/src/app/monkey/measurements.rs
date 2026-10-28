//! Scalar harness costs, separate from the production action callback and GPU phases.
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) enum Stage {
    SurfaceSetup,
    DiagnosticsRead,
    ReportSnapshotBuild,
    ReportCanonicalSerialize,
    ReportSubmit,
    JournalWrite,
    JournalFlush,
}

const STAGES: [&str; 7] = [
    "surfaceSetupMs",
    "diagnosticsReadMs",
    "reportSnapshotBuildMs",
    "reportCanonicalSerializeMs",
    "reportSubmitMs",
    "journalWriteMs",
    "journalFlushMs",
];

#[derive(Default)]
pub(super) struct Measurements {
    step: [f64; 7],
    totals: [f64; 7],
    maxima: [f64; 7],
    calls: [u64; 7],
}

impl Measurements {
    pub fn begin_step(&mut self) {
        self.step.fill(0.);
    }

    pub fn record(&mut self, stage: Stage, elapsed: Duration) {
        let index = stage as usize;
        let ms = elapsed.as_secs_f64() * 1000.;
        self.step[index] += ms;
        self.totals[index] += ms;
        self.maxima[index] = self.maxima[index].max(ms);
        self.calls[index] += 1;
    }

    /// Allocate detailed fields only when recording a slow frame.
    pub fn step(&self) -> Value {
        Value::Object(
            STAGES
                .iter()
                .zip(self.step)
                .map(|(stage, ms)| ((*stage).into(), json!(ms)))
                .collect(),
        )
    }

    /// Snapshot work appears in the next report's aggregates. Actual background
    /// storage costs are reported separately by the bounded report writer.
    pub fn summary(&self) -> Value {
        json!({
            "scope":"completed calls before this snapshot; build/serialize/queue-submission costs appear in the next snapshot and completed slow frame; reportWriter records actual background storage IO",
            "diagnosticsScope":"Host logs query only; diagnostic journal writes are measured separately",
            "stages":STAGES.iter().enumerate().map(|(index, stage)| {
                ((*stage).to_owned(), json!({"calls":self.calls[index],"totalMs":self.totals[index],"maxCallMs":self.maxima[index]}))
            }).collect::<serde_json::Map<_, _>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_tags_sum_calls_and_step_reset_preserves_aggregates() {
        let mut timings = Measurements::default();
        timings.record(Stage::DiagnosticsRead, Duration::from_millis(2));
        timings.record(Stage::JournalWrite, Duration::from_millis(3));
        timings.record(Stage::JournalWrite, Duration::from_millis(4));
        timings.record(Stage::JournalFlush, Duration::from_millis(5));
        assert_eq!(timings.step()["diagnosticsReadMs"], 2.);
        assert_eq!(timings.step()["journalWriteMs"], 7.);
        assert_eq!(timings.step()["journalFlushMs"], 5.);
        timings.begin_step();
        assert_eq!(timings.step()["journalWriteMs"], 0.);
        let summary = timings.summary();
        assert_eq!(summary["stages"]["journalWriteMs"]["calls"], 2);
        assert_eq!(summary["stages"]["journalWriteMs"]["totalMs"], 7.);
        assert_eq!(summary["stages"]["journalWriteMs"]["maxCallMs"], 4.);
        assert_eq!(
            summary["stages"].as_object().map(|stages| stages.len()),
            Some(7)
        );
    }
}
