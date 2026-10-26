//! Subtract measured host waits per frame, then calculate quantiles. GPU timestamp
//! durations cannot be subtracted from wall time: CPU/GPU work overlaps.
use super::{Value, quantiles};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Samples {
    frames: Vec<f64>,
    work: Vec<f64>,
    waits: Vec<f64>,
    hitches: u32,
    hitch_samples: u32,
}
impl Samples {
    pub fn count(&self) -> usize {
        self.frames.len()
    }
    pub fn record(&mut self, wall_ms: f64, wait_ms: Option<f64>, first_presented: bool) {
        self.frames.push(wall_ms);
        if let Some(wait) = wait_ms.filter(|v| v.is_finite() && *v >= 0. && *v <= wall_ms) {
            self.waits.push(wait);
            self.work.push(wall_ms - wait);
        }
        if !first_presented {
            self.hitch_samples += 1;
            self.hitches += u32::from(wall_ms > 16.);
        }
    }
    pub fn report(&self) -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("frames".into(), json!(self.frames.len())),
            ("frame_ms".into(), quantiles(&self.frames)),
            ("work_ms".into(), quantiles(&self.work)),
            ("gpu_wait_ms".into(), quantiles(&self.waits)),
            ("frame_samples_ms".into(), json!(self.frames)),
            ("hitches_over_16ms".into(), json!(self.hitches)),
            ("hitch_samples".into(), json!(self.hitch_samples)),
            (
                "work_coverage".into(),
                json!({"measured":self.work.len(),"unavailable":self.frames.len()-self.work.len()}),
            ),
            (
                "work_definition".into(),
                json!(
                    "Frame wall minus host time in explicit Vulkan timeline, upload-idle, swapchain acquire/present and present-queue-idle calls; includes simulation, extraction, recording, submission, audio and host scheduling. Not thread CPU cycles or wall minus GPU timestamps."
                ),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn work_is_subtracted_per_frame_and_hitches_exclude_only_first_presented_frame() {
        let mut samples = Samples::default();
        samples.record(100., Some(90.), true);
        samples.record(20., Some(19.), false);
        samples.record(20., Some(1.), false);
        samples.record(16., Some(15.), false);
        let report = samples.report();
        assert_eq!(report["work_ms"]["p50"], 1.);
        assert_eq!(report["work_ms"]["max"], 19.);
        assert_eq!(report["hitches_over_16ms"], 2);
        assert_eq!(report["hitch_samples"], 3);
        assert_eq!(report["frame_samples_ms"], json!([100., 20., 20., 16.]));
    }
    #[test]
    fn missing_or_inconsistent_wait_coverage_never_claims_zero_work() {
        let mut samples = Samples::default();
        for wait in [None, Some(-1.), Some(21.), Some(f64::NAN)] {
            samples.record(20., wait, false);
        }
        let report = samples.report();
        assert!(report["work_ms"].is_null());
        assert_eq!(
            report["work_coverage"],
            json!({"measured":0,"unavailable":4})
        );
        assert_eq!(report["hitches_over_16ms"], 4);
    }
}
