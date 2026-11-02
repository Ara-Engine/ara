//! Section 6.3 scoring. Missing samples block promotion; observed kills survive
//! missing evidence. G5 attribution never substitutes for measured frame time.
use super::contract::*;
use anyhow::{Result, ensure};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Between,
    Kill,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct Line {
    pub metric: String,
    pub observed: Option<f64>,
    pub pass_limit: f64,
    pub kill_limit: Option<f64>,
    pub relation: &'static str,
    pub verdict: Verdict,
}

#[derive(Debug, Serialize)]
pub struct CaseScore {
    pub case: String,
    pub gate: u8,
    pub verdict: Verdict,
    pub promotion: bool,
    pub kill_scope: String,
    pub lines: Vec<Line>,
    pub notes: Vec<String>,
}

fn upper(
    lines: &mut Vec<Line>,
    metric: impl Into<String>,
    value: Option<f64>,
    pass: f64,
    kill: Option<f64>,
) {
    let value = value.filter(|v| v.is_finite() && *v >= 0.);
    let verdict = match value {
        None => Verdict::Unavailable,
        Some(v) if v <= pass => Verdict::Pass,
        Some(v) if kill.is_some_and(|k| v > k) => Verdict::Kill,
        Some(_) => Verdict::Between,
    };
    lines.push(Line {
        metric: metric.into(),
        observed: value,
        pass_limit: pass,
        kill_limit: kill,
        relation: "<=",
        verdict,
    });
}
fn lower(lines: &mut Vec<Line>, metric: &str, value: Option<f64>, pass: f64) {
    let value = value.filter(|v| v.is_finite() && *v >= 0.);
    lines.push(Line {
        metric: metric.into(),
        observed: value,
        pass_limit: pass,
        kill_limit: Some(pass),
        relation: ">=",
        verdict: match value {
            None => Verdict::Unavailable,
            Some(v) if v >= pass => Verdict::Pass,
            _ => Verdict::Kill,
        },
    });
}
fn required(lines: &mut Vec<Line>, metric: &str, met: bool) {
    upper(lines, metric, met.then_some(0.), 0., None);
}
fn reference_verdict(lines: &mut [Line], converged: bool) {
    if !converged {
        for line in lines {
            line.verdict = Verdict::Unavailable;
        }
    }
}
fn complete_max(run: &RunResult, metric: impl Fn(&FrameResult) -> Option<f64>) -> Option<f64> {
    let values: Option<Vec<_>> = run.frames.iter().map(metric).collect();
    distribution(values?).map(|d| d.max)
}
fn complete_min(run: &RunResult, metric: impl Fn(&FrameResult) -> Option<f64>) -> Option<f64> {
    let values: Option<Vec<_>> = run.frames.iter().map(metric).collect();
    let values = values?;
    distribution(values.iter().copied())?;
    values.into_iter().reduce(f64::min)
}
fn stage(run: &RunResult, name: &str) -> Option<Distribution> {
    if !run.timing_valid {
        return None;
    }
    let values: Option<Vec<_>> = run
        .frames
        .iter()
        .map(|f| f.stages.get(name)?.gpu_ms)
        .collect();
    distribution(values?)
}
fn counter(
    run: &RunResult,
    name: &str,
    get: impl Fn(&StageCounters) -> Option<f64>,
) -> Option<f64> {
    complete_max(run, |f| get(f.stages.get(name)?))
}
fn evidence(run: &RunResult, name: &str) -> Option<f64> {
    complete_max(run, |f| f.evidence.get(name).copied())
}
fn all_evidence(frames: &[FrameResult], name: &str) -> bool {
    !frames.is_empty() && frames.iter().all(|f| f.evidence.get(name) == Some(&1.))
}

fn common_quality(
    lines: &mut Vec<Line>,
    run: &RunResult,
    flip_limit: Option<f64>,
    kill: Option<f64>,
) {
    let first = lines.len();
    let measured_flip = run
        .frames
        .iter()
        .filter_map(|f| f.quality.region_flip.values().copied().reduce(f64::max))
        .reduce(f64::max);
    if let Some(flip_limit) = flip_limit {
        upper(
            lines,
            "worst_region_flip",
            complete_max(run, |f| {
                f.quality.region_flip.values().copied().reduce(f64::max)
            })
            .or(measured_flip.filter(|v| *v > flip_limit)),
            flip_limit,
            kill,
        );
    }
    let measured_temporal = run
        .frames
        .iter()
        .skip(1)
        .filter_map(|f| f.quality.temporal_block_rms_white)
        .reduce(f64::max);
    let temporal = run
        .frames
        .iter()
        .skip(1)
        .map(|f| f.quality.temporal_block_rms_white)
        .collect::<Option<Vec<_>>>()
        .and_then(distribution)
        .map(|d| d.max)
        .or(measured_temporal.filter(|v| *v > 0.005));
    if run.case.frames > 1 {
        upper(
            lines,
            "D29_reference_subtracted_16x16_RMS_white",
            temporal,
            0.005,
            Some(0.005),
        );
    }
    // Finite-reference noise can manufacture a temporal or image miss. Preserve
    // observations, but neither pass nor kill quality without convergence evidence.
    reference_verdict(&mut lines[first..], run.reference_converged);
    required(
        lines,
        "reference_convergence_certified",
        run.reference_converged,
    );
}

pub fn validate_pair(a: &RunResult, b: &RunResult) -> Result<()> {
    ensure!(
        a.version == VERSION
            && b.version == VERSION
            && a.run == RunKind::Uncapped
            && b.run == RunKind::Capped,
        "wrong two-run contract/version"
    );
    ensure!(
        serde_json::to_value(&a.case)? == serde_json::to_value(&b.case)?
            && a.revision == b.revision
            && a.size == b.size
            && a.device == b.device
            && a.fps == b.fps
            && a.renderer == b.renderer,
        "replays do not share the same inputs/renderer"
    );
    for run in [a, b] {
        ensure!(
            run.frames.len() == run.case.frames as usize
                && run
                    .frames
                    .iter()
                    .enumerate()
                    .all(|(i, f)| f.frame as usize == i),
            "missing, duplicate or reordered replay frames"
        );
    }
    Ok(())
}

pub fn score(gate: u8, a: &RunResult, b: &RunResult) -> Result<CaseScore> {
    validate_pair(a, b)?;
    ensure!((1..=6).contains(&gate), "unknown gate");
    let is_4070 = a.device.contains("4070 SUPER");
    let is_760 = a.device.contains("760M");
    let mut lines = Vec::new();
    required(&mut lines, "supported_target_GPU", is_4070 || is_760);
    required(
        &mut lines,
        "native_target_resolution",
        a.size == if is_4070 { [2560, 1440] } else { [1920, 1080] },
    );
    required(&mut lines, "true_uncapped_replay", a.uncapped_supported);
    required(&mut lines, "capped_run_applied", b.caps_applied);
    required(&mut lines, "quiet_timing_valid", a.timing_valid);
    let page_cap = if is_4070 { 0.15 } else { 1. };
    match gate {
        1 => {
            let limit = if is_4070 { 0.75 } else { 2.5 };
            upper(
                &mut lines,
                "build_prep_select_shade_p99_ms",
                stage(a, "local_lights").map(|d| d.p99),
                limit,
                Some(limit),
            );
            let lpp_upper = complete_max(b, |f| {
                f.lpp.or_else(|| f.evidence.get("lpp_upper").copied())
            });
            let lpp_lower = complete_min(b, |f| {
                f.lpp.or_else(|| f.evidence.get("lpp_lower").copied())
            });
            if b.case.target_lpp.is_some_and(|v| v > 3.) {
                lower(&mut lines, "dense_LPP", lpp_lower, 26.);
                upper(&mut lines, "dense_LPP_upper", lpp_upper, 28., Some(28.));
                upper(
                    &mut lines,
                    "omitted_direct_energy_fraction",
                    complete_max(b, |f| f.quality.omitted_energy_fraction),
                    0.01,
                    Some(0.01),
                );
                upper(
                    &mut lines,
                    "region_FLIP_excess",
                    complete_max(b, |f| f.quality.region_flip_excess),
                    0.02,
                    Some(0.02),
                );
            } else {
                upper(
                    &mut lines,
                    "lights_per_covered_pixel",
                    lpp_upper,
                    3.,
                    Some(3.),
                );
                lower(&mut lines, "nonempty_LPP_workload", lpp_lower, 2.);
            }
            upper(
                &mut lines,
                "leak_pixels",
                complete_max(b, |f| f.quality.leak_pixels.map(|n| n as f64)),
                0.,
                Some(0.),
            );
            upper(
                &mut lines,
                "pop_events_above_2_percent",
                complete_max(b, |f| f.quality.pop_events.map(|n| n as f64)),
                0.,
                Some(0.),
            );
            lower(
                &mut lines,
                "highlight_retention",
                b.frames
                    .iter()
                    .map(|f| f.quality.highlight_retention)
                    .collect::<Option<Vec<_>>>()
                    .and_then(|v| v.into_iter().reduce(f64::min)),
                1.,
            );
            lower(
                &mut lines,
                "shadow_retention",
                b.frames
                    .iter()
                    .map(|f| f.quality.shadow_retention)
                    .collect::<Option<Vec<_>>>()
                    .and_then(|v| v.into_iter().reduce(f64::min)),
                1.,
            );
            if b.case.recipe == "moving_lights" {
                let cap = if is_4070 { 0.10 } else { 0.5 };
                upper(
                    &mut lines,
                    "local_pages_average_ms",
                    stage(a, "local_pages").map(|d| d.average),
                    cap,
                    Some(cap),
                );
                upper(
                    &mut lines,
                    "local_page_lag_texels",
                    counter(b, "local_pages", |s| s.edge_lag_page_texels),
                    1.,
                    Some(1.),
                );
            }
            common_quality(&mut lines, b, None, None);
        }
        2 => {
            for name in ["sun_pages", "dynamic_pages"] {
                upper(
                    &mut lines,
                    format!("{name}_average_ms"),
                    stage(a, name).map(|d| d.average),
                    page_cap,
                    Some(page_cap),
                );
                upper(
                    &mut lines,
                    format!("{name}_peak_ms"),
                    stage(a, name).map(|d| d.max),
                    1.,
                    Some(3.),
                );
                upper(
                    &mut lines,
                    format!("{name}_lag_texels"),
                    counter(b, name, |s| s.edge_lag_page_texels),
                    1.,
                    Some(1.),
                );
                upper(
                    &mut lines,
                    format!("{name}_near_lag_texels"),
                    counter(b, name, |s| s.near_edge_lag_page_texels),
                    0.5,
                    Some(0.5),
                );
            }
            common_quality(&mut lines, b, Some(0.02), Some(0.02));
            upper(
                &mut lines,
                "missing_shadow_features_ge_2px",
                complete_max(b, |f| f.quality.missing_shadow_features.map(|v| v as f64)),
                0.,
                Some(0.),
            );
            upper(
                &mut lines,
                "penumbra_relative_error",
                complete_max(b, |f| f.quality.penumbra_relative_error),
                0.1,
                Some(0.1),
            );
            upper(
                &mut lines,
                "contact_displacement_px",
                complete_max(b, |f| f.quality.contact_displacement_px),
                0.5,
                Some(0.5),
            );
            required(
                &mut lines,
                "secondary_views_scored",
                b.frames
                    .iter()
                    .all(|f| f.quality.secondary_views_scored == Some(true)),
            );
        }
        3 => {
            let cap = if is_4070 { 0.25 } else { 2. };
            upper(
                &mut lines,
                "discovery_forcing_repair_solve_average_ms",
                stage(a, "event_total").map(|d| d.average),
                cap,
                Some(cap),
            );
            let quality_start = lines.len();
            // Keep per-frame F vs F+1 limits; never average away a transient miss.
            for f in &b.frames {
                let limit = if f.frame == b.case.event_frame {
                    0.02
                } else {
                    0.01
                };
                upper(
                    &mut lines,
                    format!("frame_{}_max_solution_error_white", f.frame),
                    f.quality.max_solution_error_white,
                    limit,
                    Some(0.05),
                );
            }
            upper(
                &mut lines,
                "region_FLIP_excess",
                complete_max(b, |f| f.quality.region_flip_excess),
                0.005,
                Some(0.005),
            );
            reference_verdict(&mut lines[quality_start..], b.reference_converged);
            for name in ["link_repair", "event_solve"] {
                upper(
                    &mut lines,
                    format!("{name}_oldest_debt_frames"),
                    counter(b, name, |s| s.oldest_debt_frames.map(f64::from)),
                    3.,
                    Some(3.),
                );
                upper(
                    &mut lines,
                    format!("{name}_deadline_misses"),
                    counter(b, name, |s| s.deadline_misses.map(|v| v as f64)),
                    0.,
                    Some(0.),
                );
            }
            memory(&mut lines, b, is_4070);
            upper(
                &mut lines,
                "index_bytes_per_link",
                evidence(b, "index_bytes_per_link"),
                8.,
                Some(8.),
            );
            required(
                &mut lines,
                "global_and_per_patch_directions_compared",
                all_evidence(&b.frames, "direction_comparison_complete"),
            );
            required(
                &mut lines,
                "current_geometry_receiver_transport",
                all_evidence(&b.frames, "current_geometry_transport"),
            );
            common_quality(&mut lines, b, None, None);
        }
        4 => {
            let kill = matches!(b.case.recipe.as_str(), "overlap" | "hand" | "stained_glass")
                .then_some(0.05);
            common_quality(&mut lines, b, Some(0.02), kill);
            upper(
                &mut lines,
                "penumbra_relative_error",
                complete_max(b, |f| f.quality.penumbra_relative_error),
                0.1,
                None,
            );
            required(
                &mut lines,
                "per_source_sample_occluder_combine",
                all_evidence(&b.frames, "per_sample_combine"),
            );
            required(
                &mut lines,
                "real_time_sample_counts",
                all_evidence(&b.frames, "real_time_samples"),
            );
        }
        5 => {
            let limit = if is_4070 { 3.3 } else { 16.67 };
            let frames = a
                .frames
                .iter()
                .map(|f| f.gpu_frame_ms)
                .collect::<Option<Vec<_>>>()
                .and_then(distribution)
                .filter(|_| a.timing_valid);
            upper(
                &mut lines,
                "measured_complete_frame_p99_ms",
                frames.map(|d| d.p99),
                limit,
                Some(limit),
            );
            upper(
                &mut lines,
                "measured_complete_frame_max_ms",
                frames.map(|d| d.max),
                limit,
                Some(limit),
            );
            required(
                &mut lines,
                "CPU_critical_path_reported",
                complete_max(a, |f| f.cpu_critical_path_ms).is_some(),
            );
            required(
                &mut lines,
                "submission_stalls_reported",
                complete_max(a, |f| f.submission_stall_ms).is_some(),
            );
            for g in [1, 2, 3, 4, 6] {
                required(
                    &mut lines,
                    &format!("same_run_G{g}_quality_pass"),
                    all_evidence(&b.frames, &format!("g{g}_quality_pass")),
                );
            }
            if b.case.recipe == "combined" {
                lower(
                    &mut lines,
                    "combined_dense_LPP",
                    complete_min(b, |f| {
                        f.lpp.or_else(|| f.evidence.get("lpp_lower").copied())
                    }),
                    26.,
                );
                lower(
                    &mut lines,
                    "receiver_coverage_relative_to_G1",
                    complete_min(b, |f| {
                        f.evidence.get("receiver_coverage_relative_to_g1").copied()
                    }),
                    1.,
                );
                required(
                    &mut lines,
                    "cold_topology_publication_scored",
                    b.frames
                        .iter()
                        .find(|f| f.frame == b.case.event_frame)
                        .and_then(|f| f.evidence.get("cold_publication_scored"))
                        == Some(&1.),
                );
            }
            memory(&mut lines, b, is_4070);
            common_quality(&mut lines, b, Some(0.02), Some(0.02));
        }
        6 => {
            if b.case.recipe == "polar" {
                required(
                    &mut lines,
                    "real_Polar_asset",
                    all_evidence(&b.frames, "real_polar_asset"),
                );
            }
            common_quality(&mut lines, b, Some(0.02), Some(0.02));
            required(
                &mut lines,
                "secondary_hit_visibility_scored",
                b.frames
                    .iter()
                    .all(|f| f.quality.secondary_views_scored == Some(true)),
            );
            required(
                &mut lines,
                "coverage_beyond_glossy_mask_costed",
                all_evidence(&a.frames, "beyond_mask_costed"),
            );
        }
        _ => unreachable!(),
    }
    let verdict = combine(lines.iter().map(|l| l.verdict));
    let scope = match gate {
        1 if b.case.target_lpp.is_some_and(|l| l > 3.) => {
            "overload policy only; V2 sparse policy is unaffected"
        }
        1 => "V2; keep look::local, never clustering",
        2 => "sun/dynamic-page case; no clearing fallback is known",
        3 => "transport receiver/index policy; sector streaming for memory overflow",
        4 if b.case.recipe == "stained_glass" => "coloured shadows",
        4 => "shadow quality; dual-depth pages for overlap/hand",
        5 if is_4070 => "300 fps unsigned; kernel efficiency only, own gates decide promotion",
        5 => "route failed workloads to their own gate; otherwise overrun rows/integrated renderer",
        _ => "secondary transport",
    };
    Ok(CaseScore {case:b.case.id.clone(),gate,verdict,promotion:verdict==Verdict::Pass,kill_scope:scope.into(),lines,notes:vec!["n/a is unavailable evidence, never a zero or a pass. Both target GPUs and all gate cases are required before promotion.".into()]})
}
fn memory(lines: &mut Vec<Line>, b: &RunResult, discrete: bool) {
    let cap = if discrete {
        1_000_000_000.
    } else {
        512_000_000.
    };
    upper(
        lines,
        "peak_transport_resident_bytes",
        counter(b, "transport_memory", |s| {
            s.resident_bytes.map(|v| v as f64)
        }),
        cap,
        Some(cap),
    );
    if !discrete {
        for name in ["links_reverse_index_bytes", "channel_bytes"] {
            upper(
                lines,
                name,
                evidence(b, name),
                256_000_000.,
                Some(256_000_000.),
            );
        }
    }
}
pub fn combine(values: impl IntoIterator<Item = Verdict>) -> Verdict {
    let values: Vec<_> = values.into_iter().collect();
    if values.contains(&Verdict::Kill) {
        Verdict::Kill
    } else if values.is_empty() || values.contains(&Verdict::Unavailable) {
        Verdict::Unavailable
    } else if values.contains(&Verdict::Between) {
        Verdict::Between
    } else {
        Verdict::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_good_frame_cannot_certify_per_frame_quality() {
        let frame: FrameResult = serde_json::from_value(serde_json::json!({
            "frame":0,"stages":{},"quality":{"region_flip":{},"temporal_blocks":[]},
            "evidence":{"g1_quality_pass":1.}
        }))
        .unwrap();
        let mut frames = vec![frame.clone(), frame];
        frames[1].frame = 1;
        assert!(all_evidence(&frames, "g1_quality_pass"));
        for value in [0., 2.] {
            frames[1].evidence.insert("g1_quality_pass".into(), value);
            assert!(!all_evidence(&frames, "g1_quality_pass"));
        }
        frames[1].evidence.clear();
        assert!(!all_evidence(&frames, "g1_quality_pass"));
        assert!(!all_evidence(&[], "g1_quality_pass"));
    }
    #[test]
    fn average_debt_and_observed_peak_are_independent_gate_lines() {
        let mut lines = Vec::new();
        upper(&mut lines, "average", Some(1.01), 1., Some(1.));
        upper(&mut lines, "peak", Some(2.), 1., Some(3.));
        assert_eq!(lines[0].verdict, Verdict::Kill);
        assert_eq!(lines[1].verdict, Verdict::Between);
        upper(&mut lines, "missing", None, 1., Some(3.));
        assert_eq!(combine(lines.iter().map(|l| l.verdict)), Verdict::Kill);
    }
    #[test]
    fn boundaries_and_missing_numbers_cannot_pass() {
        assert!(distribution([1., f64::NAN]).is_none());
        assert!(distribution([]).is_none());
        let d = distribution([1., 2., 3., 4.]).unwrap();
        assert_eq!(d.average, 2.5);
        assert_eq!(d.p50, 2.);
        assert_eq!(d.p99, 4.);
        assert_eq!(
            combine([Verdict::Pass, Verdict::Unavailable]),
            Verdict::Unavailable
        );
        let mut l = Vec::new();
        upper(&mut l, "strict kill", Some(3.), 1., Some(3.));
        assert_eq!(l[0].verdict, Verdict::Between);
    }
    #[test]
    fn finite_reference_noise_cannot_pass_or_kill() {
        let mut lines = Vec::new();
        upper(&mut lines, "FLIP", Some(0.2), 0.02, Some(0.05));
        upper(&mut lines, "temporal", Some(0.001), 0.005, Some(0.005));
        reference_verdict(&mut lines, false);
        assert_eq!(lines[0].observed, Some(0.2));
        assert!(lines.iter().all(|l| l.verdict == Verdict::Unavailable));
        upper(&mut lines, "average_ms", Some(4.), 1., Some(1.));
        assert_eq!(combine(lines.iter().map(|l| l.verdict)), Verdict::Kill);
    }
}
