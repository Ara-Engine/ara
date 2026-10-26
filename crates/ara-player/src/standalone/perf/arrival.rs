//! Arrival peaks are reported separately; CPU/hitch gates still retain them.
use super::{CpuFrame, Presenter, Value, cpu, json, quantiles};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Samples {
    cpu: cpu::Samples,
    gpu: Vec<f64>,
    missing: usize,
    passes: BTreeMap<String, Vec<f64>>,
}
impl Samples {
    pub fn record(&mut self, presenter: &Presenter, frame: &CpuFrame) {
        self.cpu
            .record(frame.total_ms, frame.wait_ms, frame.first_presented);
        if let Some(ms) = presenter
            .stats()
            .gpu_span_ms
            .filter(|v| v.is_finite() && *v >= 0.)
        {
            self.gpu.push(ms);
        } else {
            self.missing += 1;
        }
        let mut sums = BTreeMap::<&str, f64>::new();
        for timing in presenter.pass_timings() {
            *sums.entry(&timing.pass).or_default() += timing.milliseconds;
        }
        for (pass, ms) in sums {
            self.passes.entry(pass.into()).or_default().push(ms);
        }
    }
    pub fn report(&self) -> Value {
        let passes: BTreeMap<_, _> = self.passes.iter().map(|(k, v)| (k, quantiles(v))).collect();
        json!({"definition":"Frames excluded while waiting for readiness/warmup; peaks include content/GI arrival, not an isolated GI cost.",
            "cpu":self.cpu.report(),"gpu_span_ms":quantiles(&self.gpu),
            "gpu_span_coverage":{"measured":self.gpu.len(),"unavailable":self.missing},"gpu_passes":passes})
    }
}
