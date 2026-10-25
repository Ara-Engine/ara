//! Whole-frame intervals are measured on one device clock before quantiles.
use super::{Value, json, quantiles};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Samples {
    spans: Vec<f64>,
    raw: Vec<Option<f64>>,
    pass_sums: Vec<f64>,
}
impl Samples {
    pub fn record(&mut self, span: Option<f64>, pass_sum: f64) {
        let span = span.filter(|v| v.is_finite() && *v >= 0.);
        if let Some(value) = span {
            self.spans.push(value);
        }
        self.raw.push(span);
        self.pass_sums.push(pass_sum);
    }
    pub fn report(&self) -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("gpu_span_ms".into(), quantiles(&self.spans)),
            (
                "gpu_span_coverage".into(),
                json!({"measured":self.spans.len(),"unavailable":self.raw.len()-self.spans.len()}),
            ),
            ("gpu_span_samples_ms".into(), json!(self.raw)),
            ("gpu_pass_sum_ms".into(), quantiles(&self.pass_sums)),
            (
                "gpu_span_definition".into(),
                json!(
                    "Earliest submission begin to latest completed submission end on the presenter device, including prepare/uploads, view, particles, post, output publication and gaps. Overlap counts once. Excludes the native swapchain/UI copy and compositor. Mixed queues require KHR/EXT_calibrated_timestamps; unavailable frames remain null."
                ),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spans_are_per_frame_and_are_not_inferred_from_pass_sums() {
        let mut samples = Samples::default();
        samples.record(Some(4.), 7.); // overlapping passes
        samples.record(Some(10.), 2.); // idle gaps between submissions
        let report = samples.report();
        assert_eq!(report["gpu_span_ms"]["p50"], 4.);
        assert_eq!(report["gpu_span_ms"]["max"], 10.);
        assert_eq!(report["gpu_pass_sum_ms"]["p50"], 2.);
        assert_eq!(report["gpu_span_samples_ms"], json!([4., 10.]));
    }
    #[test]
    fn missing_clock_or_failed_frame_does_not_report_a_zero_span() {
        let mut samples = Samples::default();
        for value in [None, Some(f64::NAN), Some(-1.)] {
            samples.record(value, 1.);
        }
        let report = samples.report();
        assert!(report["gpu_span_ms"].is_null());
        assert_eq!(
            report["gpu_span_coverage"],
            json!({"measured":0,"unavailable":3})
        );
        assert_eq!(report["gpu_span_samples_ms"], json!([null, null, null]));
    }
}
