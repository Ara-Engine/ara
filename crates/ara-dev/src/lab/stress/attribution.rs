//! G5 section-4 charges route failures only. They never enter the pass scorer.
use super::contract::{Distribution, RunResult, distribution};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Serialize)]
pub struct Attribution {
    pub measured_frame_ms: Option<Distribution>,
    pub charged_frame_ms: Option<Distribution>,
    pub charged_samples_ms: Vec<Option<f64>>,
    pub rows_over_allocation: Vec<String>,
    pub unresolved_unrowed_cost: bool,
    pub use_for_promotion: bool,
    pub rule: &'static str,
}

pub fn route(run: &RunResult) -> Attribution {
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
        .map(|frame| {
            for (name, budget) in allocations {
                if frame
                    .stages
                    .get(name)
                    .and_then(|s| s.gpu_ms)
                    .is_some_and(|ms| ms > budget)
                {
                    over.insert(name.to_string());
                }
            }
            let measured = frame.gpu_frame_ms?;
            let mut subtract = 0.;
            let mut add = 0.;
            for (gate, name, budget) in [
                (1, "local_lights", if discrete { 1.03 } else { 3.42 }),
                (2, "sun_pages", if discrete { 0.15 } else { 1. }),
            ] {
                let failed = *frame.evidence.get(&format!("g{gate}_workload_failed"))?;
                if failed == 1. {
                    subtract += frame.stages.get(name)?.gpu_ms?;
                    add += budget;
                } else if failed != 0. {
                    return None;
                }
            }
            (measured.is_finite() && measured >= subtract).then_some(measured - subtract + add)
        })
        .collect::<Vec<_>>();
    let stats = charged
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()
        .and_then(distribution);
    Attribution {
        measured_frame_ms: run
            .frames
            .iter()
            .map(|f| f.gpu_frame_ms)
            .collect::<Option<Vec<_>>>()
            .and_then(distribution),
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
