//! G5 section-4 charges route failures only. They never enter the pass scorer.
use super::contract::{Distribution, FrameResult, RunResult, distribution};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Serialize)]
pub struct Attribution {
    pub timing_valid: bool,
    pub measured_frame_ms: Option<Distribution>,
    pub charged_frame_ms: Option<Distribution>,
    pub charged_samples_ms: Vec<Option<f64>>,
    pub rows_over_allocation: Vec<String>,
    pub unresolved_unrowed_cost: bool,
    pub use_for_promotion: bool,
    pub rule: &'static str,
}

pub fn route(run: &RunResult, capped: &RunResult) -> Attribution {
    let discrete = run.device.contains("4070 SUPER");
    let allocations = [
        ("local_lights", if discrete { 1.03 } else { 3.42 }),
        ("sun_pages", if discrete { 0.15 } else { 1. }),
        ("dynamic_pages", if discrete { 0.15 } else { 1. }),
        ("local_pages", if discrete { 0.10 } else { 0.5 }),
        ("event_total", if discrete { 0.25 } else { 2. }),
        ("bounce_readout", if discrete { 0.12 } else { 0.4 }),
        ("patch_sources", if discrete { 0.14 } else { 0.8 }),
    ];
    let mut over = BTreeSet::new();
    let charged = run
        .frames
        .iter()
        .zip(&capped.frames)
        .map(|(frame, quality)| {
            if !run.timing_valid {
                return None;
            }
            for (name, budget) in allocations {
                // Already failed rows are charged at their allocation; they must
                // not hide an otherwise unrowed miss in the charged frame.
                let failed = match name {
                    "local_lights" => quality.evidence.get("g1_workload_failed"),
                    "sun_pages" => quality.evidence.get("g2_workload_failed"),
                    _ => None,
                };
                if failed == Some(&1.) {
                    continue;
                }
                if frame
                    .stages
                    .get(name)
                    .and_then(|s| s.gpu_ms)
                    .is_some_and(|ms| ms > budget)
                {
                    over.insert(name.to_string());
                }
            }
            charge(
                frame,
                quality,
                [
                    (1, "local_lights", if discrete { 1.03 } else { 3.42 }),
                    (2, "sun_pages", if discrete { 0.15 } else { 1. }),
                ],
            )
        })
        .collect::<Vec<_>>();
    let stats = charged
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()
        .and_then(distribution);
    Attribution {
        timing_valid: run.timing_valid,
        measured_frame_ms: run
            .frames
            .iter()
            .map(|f| f.gpu_frame_ms)
            .collect::<Option<Vec<_>>>()
            .and_then(distribution)
            .filter(|_| run.timing_valid),
        charged_frame_ms: stats,
        charged_samples_ms: charged,
        unresolved_unrowed_cost: stats.is_some_and(|d| d.max > if discrete { 3.3 } else { 16.67 })
            && over.is_empty(),
        rows_over_allocation: over.into_iter().collect(),
        use_for_promotion: false,
        rule: if discrete {
            "300 fps remains unsigned. Kernel efficiency only; each piece is blocked solely by its own 4070S gate. Memory routes to G3."
        } else {
            "Failed dense lights route only to G1 overload policy; failed sun routes to G2. If the charged frame still misses, overrun rows are not promoted; unexplained CPU/submission/cold-publication cost blocks the integrated default. Memory routes to G3."
        },
    }
}

/// Charge measured demand using the matching capped frame's quality adjudication.
fn charge(cost: &FrameResult, quality: &FrameResult, rows: [(u8, &str, f64); 2]) -> Option<f64> {
    if cost.frame != quality.frame {
        return None;
    }
    let measured = cost.gpu_frame_ms?;
    let mut subtract = 0.;
    let mut add = 0.;
    for (gate, name, budget) in rows {
        let failed = *quality.evidence.get(&format!("g{gate}_workload_failed"))?;
        if failed == 1. {
            subtract += cost.stages.get(name)?.gpu_ms?;
            add += budget;
        } else if failed != 0. {
            return None;
        }
    }
    (measured.is_finite() && measured >= subtract).then_some(measured - subtract + add)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn charges_use_capped_quality_and_matching_uncapped_cost() {
        let mut cost: FrameResult = serde_json::from_value(serde_json::json!({
            "frame":8,"gpu_frame_ms":10.,"stages":{"local_lights":{"gpu_ms":6.}},
            "quality":{"region_flip":{},"temporal_blocks":[]},"evidence":{}
        }))
        .unwrap();
        let mut quality = cost.clone();
        quality.evidence.insert("g1_workload_failed".into(), 1.);
        quality.evidence.insert("g2_workload_failed".into(), 0.);
        let rows = [(1, "local_lights", 1.03), (2, "sun_pages", 0.15)];
        assert_eq!(charge(&cost, &quality, rows), Some(5.03));
        assert_eq!(charge(&cost, &cost, rows), None);
        cost.frame = 9;
        assert_eq!(charge(&cost, &quality, rows), None);
    }
}
