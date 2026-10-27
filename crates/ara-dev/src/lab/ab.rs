//! Paired, counterbalanced windows: AB, BA, AB, BA in one warm process.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;

impl super::session::Session {
    pub fn ab(
        &mut self,
        base: &std::path::Path,
        experiment: &str,
        windows: u32,
        frames: u32,
        preset: &str,
    ) -> Result<Value> {
        ensure!(experiment != "baseline", "choose an experimental variant B");
        super::variants::Variants::load(base).ready(experiment)?;
        let previous = self.selection.variant.clone();
        let pid = self.child.id();
        let mut readings = Vec::new();
        let trace = base
            .join("measurements")
            .join(format!("{}-ab-trace.json", super::session::id()));
        std::fs::create_dir_all(
            trace
                .parent()
                .ok_or_else(|| anyhow::anyhow!("trace parent"))?,
        )?;
        self.last_trace = Some(trace.clone());
        let result = (|| -> Result<Value> {
            for pair in 0..windows {
                let sides = if pair % 2 == 0 {
                    [("a", "baseline"), ("b", experiment)]
                } else {
                    [("b", experiment), ("a", "baseline")]
                };
                for (side, variant) in sides {
                    self.variant(base, variant)?;
                    let mut reading = self.measure_window(frames, preset)?;
                    ensure!(self.child.id() == pid, "A/B player changed");
                    reading["side"] = json!(side);
                    reading["variant"] = json!(variant);
                    reading["pair"] = json!(pair);
                    readings.push(reading);
                    super::session::write(
                        &trace,
                        &json!({"status":"incomplete","pid":pid,"windows":readings,"timing_guard":crate::gpu::last_reading()}),
                    )?;
                }
            }
            let comparison = summarize(&readings)?;
            self.variant(base, "baseline")?;
            let a = self.temporal()?;
            self.variant(base, experiment)?;
            let b = self.temporal()?;
            Ok(
                json!({"format":"ara.lab.ab/1","a":"baseline","b":experiment,"pid":pid,"order":"AB, BA, repeated","comparison":comparison,"windows":readings,"trace":trace,"temporal":{"baseline":a["temporal"],"experiment":b["temporal"]}}),
            )
        })();
        let restored = self.variant(base, &previous);
        match (result, restored) {
            (Ok(mut report), Ok(_)) => {
                report["restored_variant"] = json!(previous);
                Ok(report)
            }
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }
}

/// Describe paired deltas without treating GPU pass sums as elapsed frame time.
pub(super) fn summarize(windows: &[Value]) -> Result<Value> {
    ensure!(
        windows.len() >= 4 && windows.len().is_multiple_of(2),
        "A/B needs at least two complete pairs"
    );
    let pairs = windows
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let (a, b) = if pair[0]["side"] == "a" {
                (&pair[0], &pair[1])
            } else {
                (&pair[1], &pair[0])
            };
            ensure!(
                a["side"] == "a" && b["side"] == "b",
                "A/B pair has no baseline or experiment"
            );
            Ok((&a["player"], &b["player"]))
        })
        .collect::<Result<Vec<_>>>()?;
    let summarize = |read: &dyn Fn(&Value) -> Option<f64>| -> Value {
        let values = pairs
            .iter()
            .filter_map(|(a, b)| Some((read(a)?, read(b)?)))
            .collect::<Vec<_>>();
        if values.len() != pairs.len() {
            return json!({"status":"not_measured","reason":"missing metric in a window"});
        }
        statistics(&values)
    };
    let mut metrics = json!({});
    for name in ["gpu_span_ms", "gpu_pass_sum_ms", "gpu_submission_gap_ms"] {
        metrics[name] = summarize(&|player| player[name]["p50"].as_f64());
    }
    metrics["cpu_frame_ms"] = summarize(&|player| player["cpu"]["frame_ms"]["p50"].as_f64());
    let names = pairs
        .iter()
        .flat_map(|(a, b)| [a, b])
        .filter_map(|p| p["gpu_passes"].as_object())
        .flat_map(|passes| passes.keys().cloned())
        .collect::<BTreeSet<_>>();
    let mut passes = json!({});
    for name in names {
        passes[&name] = summarize(&|player| player["gpu_passes"][&name]["p50"].as_f64());
    }
    Ok(
        json!({"pair_count":pairs.len(),"window_statistic":"p50","delta_definition":"B minus baseline A; negative milliseconds are faster","noise_definition":"sample standard deviation of paired window deltas, standard error, and two-sided 95% Student-t interval","metrics":metrics,"gpu_passes":passes}),
    )
}

fn statistics(values: &[(f64, f64)]) -> Value {
    let count = values.len() as f64;
    let a = values.iter().map(|v| v.0).sum::<f64>() / count;
    let b = values.iter().map(|v| v.1).sum::<f64>() / count;
    let delta = b - a;
    let deviation = (values
        .iter()
        .map(|(a, b)| (b - a - delta).powi(2))
        .sum::<f64>()
        / (count - 1.))
        .sqrt();
    let standard_error = deviation / count.sqrt();
    // Conservative small-sample critical values; the normal limit is used after 30 pairs.
    let critical = match values.len() - 1 {
        1 => 12.707,
        2 => 4.303,
        3 => 3.183,
        4 => 2.777,
        5 => 2.571,
        6 => 2.447,
        7 => 2.365,
        8 => 2.307,
        9 => 2.263,
        10..=14 => 2.229,
        15..=19 => 2.145,
        20..=29 => 2.094,
        _ => 2.043,
    };
    let margin = critical * standard_error;
    json!({"baseline_a_ms":a,"variant_b_ms":b,"delta_ms":delta,"delta_percent":if a>0. {Some(100.*delta/a)}else{None},"paired_delta_sd_ms":deviation,"standard_error_ms":standard_error,"confidence_95_ms":[delta-margin,delta+margin],"result":if delta+margin<0. {"faster"}else if delta-margin>0. {"slower"}else{"inconclusive"}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counterbalancing_cancels_uniform_linear_drift_and_reports_missing_metrics() -> Result<()> {
        let windows = [("a", 10.), ("b", 9.), ("b", 10.), ("a", 13.)]
            .into_iter()
            .map(|(side, v)| json!({"side":side,"player":{"gpu_span_ms":{"p50":v}}}))
            .collect::<Vec<_>>();
        let report = summarize(&windows)?;
        assert_eq!(report["metrics"]["gpu_span_ms"]["delta_ms"], -2.);
        assert_eq!(
            report["metrics"]["gpu_span_ms"]["paired_delta_sd_ms"].as_f64(),
            Some(2_f64.sqrt())
        );
        assert_eq!(report["metrics"]["gpu_span_ms"]["result"], "inconclusive");
        assert_eq!(
            report["metrics"]["gpu_pass_sum_ms"]["status"],
            "not_measured"
        );
        assert!(summarize(&windows[..2]).is_err());
        Ok(())
    }
}
