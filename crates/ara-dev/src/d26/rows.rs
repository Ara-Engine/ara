//! Scoreboard rows: each measured value, the gates it is held to and how it fares.
//! Pure over `Results`, so the judging rules are tested without a GPU.
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::Section;
use super::measure::{Measured, Output, Results, SCENES, Window};
use crate::perf::Preset;

/// D26's GPU budgets and D27's record targets per preset (ms, 2560×1440, RTX 4070
/// SUPER), in `Preset::ALL` order.
const D26_GPU: [f64; 4] = [2., 3.5, 6., 10.];
const D27_GPU: [f64; 4] = [1., 1.75, 3., 5.];
/// D26: CPU frame and hitch limits (ms); startup limits (ms).
const D26_CPU: f64 = 4.;
const D26_HITCH: f64 = 16.;
const D26_FIRST_FRAME: f64 = 2000.;
const D26_FULL_SCENE: f64 = 3000.;
/// The editor's bar: nothing on the UI thread may take longer than a 60 Hz frame.
const EDITOR_FRAME: f64 = 16.7;

/// What each gate and metric means, for the report's footer.
pub(super) const DEFINITIONS: &[(&str, &str)] = &[
    (
        "D26",
        "Release gate (docs/decisions.md D26): GPU Low/Medium/High/Epic ≤ 2/3.5/6/10 ms, CPU frame ≤ 4 ms, no hitch > 16 ms after the first frame, a presented frame ≤ 2 s after launch (cold), the full first scene ≤ 3 s (warm).",
    ),
    (
        "D27",
        "Record targets (D27): whole-frame GPU Low/Medium/High/Epic 1/1.75/3/5 ms. D26 gates the alpha; D27 is the aim.",
    ),
    (
        "Editor",
        "Nothing on the editor's UI thread may exceed one 60 Hz frame (16.7 ms): input-to-present p95, frames over 16.7 ms, missed clicks.",
    ),
    (
        "GPU frame",
        "gpu_other.total_ms from `ara-dev perf`: the sum of the presenter's GPU timestamps per frame. Async-compute passes overlapping graphics stretch their timestamps, so the sum can overstate the frame.",
    ),
    (
        "CPU work",
        "The player's cpu.work_ms: each frame's wall time minus its explicit driver host waits, before quantiles. It gates D26 only when work_coverage covers every measured frame. Hitches are cpu.hitches_over_16ms, which excludes only the process's first presented frame.",
    ),
    (
        "Wall bound",
        "Where no complete CPU-work metric exists (older players, the editor), cpu.frame_ms and the editor's frame minus its UI fence wait still include GPU, present and other waits. Such a value passes at or under its limit and measures nothing above it.",
    ),
    (
        "Cold / warm",
        "Player cold: a fresh pipeline cache, only the first frame skipped; warm: the cache it left. Editor cold: a fresh copy of samples/outpost (imports and pipelines from nothing); warm: relaunched on that copy. The OS file cache and driver shader cache stay warm in both.",
    ),
    (
        "Full first scene",
        "Editor: the first Scene frame rendered and presented after no import is pending (models resolved; texture streaming not tracked). The player has no readiness signal yet.",
    ),
    (
        "ꟻLIP / RMSE",
        "`ara-samples capture --reference` (the visual sweep): LDR ꟻLIP and sRGB RMSE of each preset's frame against the path-traced reference, at 1280×720 evidence size; reported beside the timings, with no threshold.",
    ),
];

/// How a value fares against one gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Pass,
    Fail,
    NotMeasured,
    /// Measured, with no gate to hold it to.
    Reported,
}

#[derive(Serialize)]
pub(super) struct Judged {
    pub gate: &'static str,
    pub limit: f64,
    pub status: Status,
}

#[derive(Serialize)]
pub(super) struct Row {
    pub section: &'static str,
    pub subject: String,
    pub metric: String,
    pub value: Option<f64>,
    pub unit: &'static str,
    /// The value only bounds the metric from above (see "Wall bound").
    pub bound: bool,
    pub gates: Vec<Judged>,
    /// `Reported` or `NotMeasured` for rows without gates; else the worst gate.
    pub status: Status,
    pub note: String,
    /// The tool output the value came from.
    pub source: Option<String>,
}

fn judge(value: Option<f64>, bound: bool, limit: f64) -> Status {
    match value {
        None => Status::NotMeasured,
        Some(v) if v <= limit => Status::Pass,
        Some(_) if bound => Status::NotMeasured,
        Some(_) => Status::Fail,
    }
}

impl Row {
    fn new(section: &'static str, subject: impl Into<String>, metric: impl Into<String>) -> Self {
        Self {
            section,
            subject: subject.into(),
            metric: metric.into(),
            value: None,
            unit: "ms",
            bound: false,
            gates: Vec::new(),
            status: Status::NotMeasured,
            note: String::new(),
            source: None,
        }
    }
    fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }
    fn bound(mut self) -> Self {
        self.bound = true;
        self
    }
    fn gate(mut self, gate: &'static str, limit: f64) -> Self {
        self.gates.push(Judged {
            gate,
            limit,
            status: Status::NotMeasured,
        });
        self
    }
    fn note(mut self, note: impl AsRef<str>) -> Self {
        if !self.note.is_empty() {
            self.note.push_str("; ");
        }
        self.note.push_str(note.as_ref());
        self
    }
    fn source(mut self, source: Option<String>) -> Self {
        self.source = source;
        self
    }
    /// Set the value (or the reason there is none) and judge every gate.
    fn value(mut self, value: Measured<f64>) -> Self {
        match value {
            Ok(v) => self.value = Some(v),
            Err(reason) => self = self.note(reason),
        }
        for judged in &mut self.gates {
            judged.status = judge(self.value, self.bound, judged.limit);
        }
        self.status = if self.gates.is_empty() {
            if self.value.is_some() {
                Status::Reported
            } else {
                Status::NotMeasured
            }
        } else if self.gates.iter().any(|g| g.status == Status::Fail) {
            Status::Fail
        } else if self.gates.iter().any(|g| g.status == Status::NotMeasured) {
            Status::NotMeasured
        } else {
            Status::Pass
        };
        self
    }
}

/// Pass/fail/not-measured counts per gate.
#[derive(Default, Serialize)]
pub(super) struct Counts {
    pub pass: usize,
    pub fail: usize,
    pub not_measured: usize,
}

#[derive(Serialize)]
pub(super) struct Summary {
    pub gates: BTreeMap<&'static str, Counts>,
}

impl Summary {
    pub fn of(rows: &[Row]) -> Self {
        let mut gates: BTreeMap<&'static str, Counts> = BTreeMap::new();
        for judged in rows.iter().flat_map(|row| &row.gates) {
            let counts = gates.entry(judged.gate).or_default();
            match judged.status {
                Status::Pass => counts.pass += 1,
                Status::Fail => counts.fail += 1,
                Status::NotMeasured | Status::Reported => counts.not_measured += 1,
            }
        }
        Self { gates }
    }
    /// PASS only when every row of a gate passed; one failure is FAIL; otherwise a gate
    /// with unmeasured rows is INCOMPLETE.
    pub fn word(counts: &Counts) -> &'static str {
        if counts.fail > 0 {
            "FAIL"
        } else if counts.not_measured > 0 {
            "INCOMPLETE"
        } else {
            "PASS"
        }
    }
    pub fn verdict(&self) -> String {
        self.gates
            .iter()
            .map(|(gate, c)| {
                format!(
                    "{gate}: {} ({} pass, {} fail, {} not measured)",
                    Self::word(c),
                    c.pass,
                    c.fail,
                    c.not_measured
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn title(preset: Preset) -> &'static str {
    match preset {
        Preset::Low => "Low",
        Preset::Medium => "Medium",
        Preset::High => "High",
        Preset::Epic => "Epic",
    }
}

fn at<'v>(value: &'v Value, path: &[&str]) -> &'v Value {
    path.iter().fold(value, |value, key| &value[key])
}

/// Why an entry is missing: its section was skipped (and why), or it never ran.
fn missing(results: &Results, section: Section) -> String {
    results
        .skipped
        .get(&section)
        .cloned()
        .unwrap_or_else(|| "not run".into())
}

/// A player metric from a window, with the window's report as the source.
fn window_metric(
    results: &Results,
    window: Option<&Measured<Window>>,
    path: &[&str],
) -> (Measured<f64>, Option<String>) {
    match window {
        None => (Err(missing(results, Section::Player)), None),
        Some(Err(reason)) => (Err(reason.clone()), None),
        Some(Ok(window)) => (
            at(&window.player, path)
                .as_f64()
                .ok_or_else(|| format!("the player report has no {}", path.join("."))),
            Some(window.report.display().to_string()),
        ),
    }
}

/// Nearest-rank percentile (as the player's perf windows compute theirs).
fn percentile(values: &mut [f64], fraction: f64) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let rank = ((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1);
    values.get(rank).copied()
}

/// Every row, in report order.
pub(super) fn rows(results: &Results) -> Vec<Row> {
    let mut rows = Vec::new();
    gpu(results, &mut rows);
    cpu(results, &mut rows);
    startup(results, &mut rows);
    editor(results, &mut rows);
    rows
}

fn gpu(results: &Results, rows: &mut Vec<Row>) {
    for scene in &SCENES {
        for (index, preset) in Preset::ALL.into_iter().enumerate() {
            let subject = format!("{} · {}", scene.name, title(preset));
            let window = results.steady.get(&(scene.name, preset.name()));
            for stat in ["p50", "p95"] {
                let (value, source) =
                    window_metric(results, window, &["gpu_other", "total_ms", stat]);
                rows.push(
                    Row::new("GPU frame", &subject, format!("GPU frame {stat}"))
                        .gate("D26", D26_GPU[index])
                        .gate("D27", D27_GPU[index])
                        .source(source)
                        .value(value),
                );
            }
            rows.extend(flip(results, scene.name, preset, &subject));
        }
    }
}

/// ꟻLIP and sRGB RMSE rows for one scene and preset.
fn flip(results: &Results, scene: &str, preset: Preset, subject: &str) -> [Row; 2] {
    let found: Result<(&Output, &Value), String> = match results.flip.get(scene) {
        None => Err(missing(results, Section::Flip)),
        Some(Err(reason)) => Err(reason.clone()),
        Some(Ok(output)) => output.value["modes"]
            .as_array()
            .and_then(|modes| {
                modes.iter().find(|mode| {
                    mode["preset"]
                        .as_str()
                        .is_some_and(|p| p.eq_ignore_ascii_case(preset.name()))
                })
            })
            .map(|mode| (output, mode))
            .ok_or_else(|| format!("no {} mode in {}", preset.name(), output.path.display())),
    };
    let spp = results
        .flip
        .get(scene)
        .and_then(|output| output.as_ref().ok())
        .and_then(|output| output.value["referenceSettings"]["samples"].as_u64());
    let rows = [
        ("flip", "ꟻLIP", "ꟻLIP vs path-traced reference"),
        ("srgbRmse", "RMSE", "sRGB RMSE vs path-traced reference"),
    ];
    rows.map(|(key, unit, metric)| {
        let row = Row::new("GPU frame", subject, metric).unit(unit);
        match &found {
            Err(reason) => row.value(Err(reason.clone())),
            Ok((output, mode)) => {
                let mut scores: Vec<f64> = mode["reference"]
                    .as_array()
                    .map(|shots| shots.iter().filter_map(|shot| shot[key].as_f64()).collect())
                    .unwrap_or_default();
                let mean =
                    (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64);
                let max = percentile(&mut scores, 1.);
                row.note(format!(
                    "mean of {} shots, max {}; {} reference; renderer {}",
                    scores.len(),
                    max.map_or("-".into(), |m| format!("{m:.4}")),
                    spp.map_or("unknown spp".into(), |s| format!("{s} spp")),
                    mode["renderer"].as_str().unwrap_or("unknown"),
                ))
                .source(Some(output.path.display().to_string()))
                .value(mean.ok_or_else(|| format!("no {key} scores")))
            }
        }
    })
}

/// `Ok` when the window's `cpu.work_ms` covers every measured frame (a complete D26
/// CPU verdict); otherwise why it does not.
fn work_coverage(window: &Window) -> Result<(), String> {
    let frames = window.player["frames"].as_f64();
    let coverage = &window.player["cpu"]["work_coverage"];
    match (
        coverage["measured"].as_f64(),
        coverage["unavailable"].as_f64(),
    ) {
        (Some(measured), Some(unavailable)) if Some(measured) == frames && unavailable == 0. => {
            Ok(())
        }
        (Some(measured), Some(unavailable)) => Err(format!(
            "cpu.work_ms covers {measured} of {} frames ({unavailable} unavailable)",
            frames.unwrap_or(0.)
        )),
        _ => Err("this player reports no cpu.work_ms".into()),
    }
}

/// A hitch row: the player's hitch count when it reports one, else its longest frame.
fn hitch(results: &Results, window: Option<&Measured<Window>>, subject: &str, label: &str) -> Row {
    let (count, source) = window_metric(results, window, &["cpu", "hitches_over_16ms"]);
    let (longest, _) = window_metric(results, window, &["cpu", "frame_ms", "max"]);
    match count {
        Ok(count) => {
            let (eligible, _) = window_metric(results, window, &["cpu", "hitch_samples"]);
            Row::new("CPU frame", subject, format!("Hitches over 16 ms, {label}"))
                .unit("frames")
                .gate("D26", 0.)
                .note(format!(
                    "of {} frames; longest {} ms",
                    eligible.map_or("?".into(), |n| format!("{n:.0}")),
                    longest.map_or("?".into(), |m| format!("{m:.1}"))
                ))
                .source(source)
                .value(Ok(count))
        }
        Err(_) => {
            let (value, source) = window_metric(results, window, &["cpu", "frame_ms", "max"]);
            Row::new("CPU frame", subject, format!("Longest frame, {label}"))
                .gate("D26", D26_HITCH)
                .source(source)
                .value(value)
        }
    }
}

fn cpu(results: &Results, rows: &mut Vec<Row>) {
    for scene in &SCENES {
        let subject = format!("{} · High (Play)", scene.name);
        let steady = results.steady.get(&(scene.name, "high"));
        let coverage = match steady {
            Some(Ok(window)) => work_coverage(window),
            _ => Err(String::new()),
        };
        for stat in ["p50", "p95"] {
            let row = match &coverage {
                Ok(()) => {
                    let (value, source) = window_metric(results, steady, &["cpu", "work_ms", stat]);
                    Row::new("CPU frame", &subject, format!("CPU work {stat}"))
                        .gate("D26", D26_CPU)
                        .source(source)
                        .value(value)
                }
                Err(reason) => {
                    let (value, source) =
                        window_metric(results, steady, &["cpu", "frame_ms", stat]);
                    let mut row = Row::new(
                        "CPU frame",
                        &subject,
                        format!("CPU frame {stat} (wall bound)"),
                    )
                    .bound()
                    .gate("D26", D26_CPU)
                    .source(source);
                    if !reason.is_empty() {
                        row = row.note(reason);
                    }
                    row.value(value)
                }
            };
            rows.push(row);
        }
        let cold = hitch(
            results,
            results.cold.get(scene.name),
            &subject,
            "after the first frame (cold pipeline cache)",
        );
        rows.push(cold.note("a fresh launch: pipeline compiles must not hitch"));
        rows.push(hitch(results, steady, &subject, "steady window"));
    }
    let orbit = editor_frames(results, false, |name| name == "scene-orbit");
    let subject = "Editor · Outpost Scene orbit (after imports)";
    for (stat, fraction) in [("p50", 0.5), ("p95", 0.95)] {
        let value = orbit
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|(frames, _)| {
                let mut work: Vec<f64> = frames
                    .iter()
                    .map(|s| number(s, "cpu_ms") - number(s, "gpu_ms"))
                    .collect();
                percentile(&mut work, fraction).ok_or_else(|| "no Scene orbit frames".into())
            });
        rows.push(
            Row::new(
                "CPU frame",
                subject,
                format!("Frame minus UI fence wait {stat} (wall bound)"),
            )
            .bound()
            .gate("D26", D26_CPU)
            .note("D26's CPU limit applied to the editor Scene view")
            .source(orbit.as_ref().ok().map(|(_, source)| source.clone()))
            .value(value),
        );
    }
    let value = orbit
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|(frames, _)| {
            frames
                .iter()
                .map(|s| number(s, "cpu_ms"))
                .reduce(f64::max)
                .ok_or_else(|| "no Scene orbit frames".into())
        });
    rows.push(
        Row::new("CPU frame", subject, "Longest frame")
            .gate("D26", D26_HITCH)
            .source(orbit.as_ref().ok().map(|(_, source)| source.clone()))
            .value(value),
    );
}

fn number(sample: &Value, key: &str) -> f64 {
    sample[key].as_f64().unwrap_or(0.)
}

/// Samples of the editor run's frames whose scenario `keep` accepts; the cold run
/// keeps only frames with imports pending. Also returns the run's file.
fn editor_frames(
    results: &Results,
    cold: bool,
    keep: impl Fn(&str) -> bool,
) -> Measured<(Vec<&Value>, String)> {
    let run = if cold {
        &results.editor_cold
    } else {
        &results.editor_warm
    };
    let output = match run {
        None => return Err(missing(results, Section::Editor)),
        Some(Err(reason)) => return Err(reason.clone()),
        Some(Ok(output)) => output,
    };
    let frames: Vec<&Value> = output.value["frames"]
        .as_array()
        .map(|frames| {
            frames
                .iter()
                .filter(|f| f["scenario"].as_str().is_some_and(&keep))
                .map(|f| &f["sample"])
                .filter(|s| !cold || s["importing"] == true)
                .collect()
        })
        .unwrap_or_default();
    if frames.is_empty() && cold {
        return Err("no interaction ran while imports were pending (they finished first)".into());
    }
    Ok((frames, output.path.display().to_string()))
}

/// An editor startup field, in ms from launch.
fn editor_value(results: &Results, cold: bool, path: &[&str]) -> (Measured<f64>, Option<String>) {
    let run = if cold {
        &results.editor_cold
    } else {
        &results.editor_warm
    };
    match run {
        None => (Err(missing(results, Section::Editor)), None),
        Some(Err(reason)) => (Err(reason.clone()), None),
        Some(Ok(output)) => (
            at(&output.value, path)
                .as_f64()
                .ok_or_else(|| format!("the probe recorded no {}", path.join("."))),
            Some(output.path.display().to_string()),
        ),
    }
}

fn startup(results: &Results, rows: &mut Vec<Row>) {
    for scene in &SCENES {
        let subject = format!("Player · {}", scene.name);
        let (value, source) = window_metric(
            results,
            results.cold.get(scene.name),
            &["startup", "first_frame_ms"],
        );
        rows.push(
            Row::new("Startup", &subject, "First presented frame, cold")
                .gate("D26", D26_FIRST_FRAME)
                .note("from the player's main(); it presents the scene itself, with no separate loading view")
                .source(source)
                .value(value),
        );
        let (value, source) = window_metric(
            results,
            results.steady.get(&(scene.name, "high")),
            &["startup", "first_frame_ms"],
        );
        rows.push(
            Row::new("Startup", &subject, "First presented frame, warm")
                .gate("D26", D26_FIRST_FRAME)
                .source(source)
                .value(value),
        );
        rows.push(
            Row::new("Startup", &subject, "Full first scene frame, warm")
                .gate("D26", D26_FULL_SCENE)
                .value(Err("the player has no content-readiness signal yet (texture refinement, probe GI); its first frame is not full-scene-ready".into())),
        );
    }
    let subject = "Editor · Outpost";
    for (cold, metric, path, gate) in [
        (
            true,
            "First presented frame (loading view), cold",
            ["startup", "firstPresentMs"],
            Some(D26_FIRST_FRAME),
        ),
        (
            false,
            "First presented frame, warm",
            ["startup", "firstPresentMs"],
            Some(D26_FIRST_FRAME),
        ),
        (
            false,
            "Full first scene frame, warm",
            ["startup", "sceneCompleteMs"],
            Some(D26_FULL_SCENE),
        ),
        (
            true,
            "Full first scene frame, cold (first import)",
            ["startup", "sceneCompleteMs"],
            None,
        ),
    ] {
        let (value, source) = editor_value(results, cold, &path);
        let mut row = Row::new("Startup", subject, metric).source(source);
        if let Some(limit) = gate {
            row = row.gate("D26", limit);
        }
        rows.push(row.value(value));
    }
    let plays = match &results.editor_warm {
        None => Err(missing(results, Section::Editor)),
        Some(Err(reason)) => Err(reason.clone()),
        Some(Ok(output)) => Ok((
            output.value["play"]
                .as_array()
                .map(|plays| {
                    plays
                        .iter()
                        .map(|p| p["clickToFirstGameFrameMs"].as_f64())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            output.path.display().to_string(),
        )),
    };
    let source = plays.as_ref().ok().map(|(_, source)| source.clone());
    let first = plays.as_ref().map_err(Clone::clone).and_then(|(plays, _)| {
        plays
            .first()
            .copied()
            .flatten()
            .ok_or_else(|| "the first Play showed no Game frame".into())
    });
    rows.push(
        Row::new(
            "Startup",
            subject,
            "Play click → first Game frame, first Play (warm editor)",
        )
        .source(source.clone())
        .value(first),
    );
    let repeat = plays.as_ref().map_err(Clone::clone).and_then(|(plays, _)| {
        let repeats: Option<Vec<f64>> = plays.iter().skip(1).copied().collect();
        repeats
            .and_then(|r| r.into_iter().reduce(f64::max))
            .ok_or_else(|| "a repeat Play showed no Game frame".into())
    });
    rows.push(
        Row::new(
            "Startup",
            subject,
            "Play click → first Game frame, repeat Plays (max)",
        )
        .source(source)
        .value(repeat),
    );
}

fn editor(results: &Results, rows: &mut Vec<Row>) {
    for (cold, label) in [
        (true, "during imports (cold)"),
        (false, "after imports (warm)"),
    ] {
        let subject = format!("Editor · Outpost, {label}");
        let interaction = |name: &str| name != "warmup" && name != "settle";
        for (metric, keep) in [
            (
                "Click → present p95",
                (|n: &str| n.starts_with("click-")) as fn(&str) -> bool,
            ),
            ("Typing → present p95", |n: &str| {
                n.starts_with("type-") && (n.ends_with("-keystroke") || n.ends_with("-commit"))
            }),
            ("Drag → present p95", |n: &str| {
                n == "drag-move" || n == "drag-commit"
            }),
        ] {
            let frames = editor_frames(results, cold, keep);
            let value = frames
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|(frames, _)| {
                    let mut latencies: Vec<f64> = frames
                        .iter()
                        .filter_map(|s| s["input_to_present_ms"].as_f64())
                        .collect();
                    percentile(&mut latencies, 0.95)
                        .ok_or_else(|| "no input reached a presented frame".into())
                });
            rows.push(
                Row::new("Editor responsiveness", &subject, metric)
                    .gate("Editor", EDITOR_FRAME)
                    .source(frames.as_ref().ok().map(|(_, source)| source.clone()))
                    .value(value),
            );
        }
        let frames = editor_frames(results, cold, interaction);
        let over = frames.as_ref().map(|(frames, _)| {
            let max = frames
                .iter()
                .map(|s| number(s, "cpu_ms"))
                .fold(0., f64::max);
            let over = frames
                .iter()
                .filter(|s| number(s, "cpu_ms") > EDITOR_FRAME)
                .count();
            (over, frames.len(), max)
        });
        let mut row = Row::new("Editor responsiveness", &subject, "Frames over 16.7 ms")
            .unit("frames")
            .gate("Editor", 0.)
            .source(frames.as_ref().ok().map(|(_, source)| source.clone()));
        if let Ok((_, total, max)) = &over {
            row = row.note(format!("of {total} frames; longest {max:.1} ms"));
        }
        rows.push(row.value(over.map(|(over, _, _)| over as f64).map_err(|e| e.clone())));
        let run = if cold {
            &results.editor_cold
        } else {
            &results.editor_warm
        };
        let clicks = match run {
            None => Err(missing(results, Section::Editor)),
            Some(Err(reason)) => Err(reason.clone()),
            Some(Ok(output)) => {
                let total = output.value["clicks"]["total"].as_f64().unwrap_or(0.);
                let landed = output.value["clicks"]["landed"].as_f64().unwrap_or(0.);
                Ok((total - landed, total))
            }
        };
        let mut row = Row::new("Editor responsiveness", &subject, "Missed clicks")
            .unit("clicks")
            .gate("Editor", 0.);
        if let Ok((_, total)) = &clicks {
            row = row.note(format!(
                "of {total} clicks in the run, imports pending or not"
            ));
        }
        rows.push(row.value(clicks.map(|(missed, _)| missed)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn missing_and_bounded_values_never_pass() {
        assert_eq!(judge(None, false, 6.), Status::NotMeasured);
        assert_eq!(judge(Some(5.), false, 6.), Status::Pass);
        assert_eq!(judge(Some(7.), false, 6.), Status::Fail);
        // A wall bound over its limit says nothing about the CPU work under it.
        assert_eq!(judge(Some(7.), true, 6.), Status::NotMeasured);
        assert_eq!(judge(Some(3.), true, 6.), Status::Pass);
        let row = Row::new("x", "y", "z").value(Err("skipped".into()));
        assert_eq!(
            (row.status, row.note.as_str()),
            (Status::NotMeasured, "skipped")
        );
        let row = Row::new("x", "y", "z").value(Ok(1.));
        assert_eq!(row.status, Status::Reported);
    }

    #[test]
    fn gpu_rows_judge_each_gate_and_unrun_scenes_stay_unmeasured() {
        let mut results = Results::default();
        results.steady.insert(
            ("Outpost", "high"),
            Ok(Window {
                report: PathBuf::from("r.json"),
                player: json!({"gpu_other": {"total_ms": {"p50": 4.0, "p95": 6.5}}}),
            }),
        );
        let rows = rows(&results);
        let find = |subject: &str, metric: &str| {
            rows.iter()
                .find(|r| r.subject == subject && r.metric == metric)
                .expect("row")
        };
        let p50 = find("Outpost · High", "GPU frame p50");
        assert_eq!(
            p50.gates
                .iter()
                .map(|g| (g.gate, g.status))
                .collect::<Vec<_>>(),
            [("D26", Status::Pass), ("D27", Status::Fail)]
        );
        assert_eq!(
            find("Outpost · High", "GPU frame p95").gates[0].status,
            Status::Fail
        );
        let epic = find("Hangar · Epic", "GPU frame p50");
        assert_eq!(
            (epic.status, epic.note.as_str()),
            (Status::NotMeasured, "not run")
        );
        let summary = Summary::of(&rows);
        assert_eq!(Summary::word(&summary.gates["D26"]), "FAIL");
    }

    #[test]
    fn cpu_work_gates_only_with_full_coverage_and_hitches_count() {
        let window = |measured: u32, unavailable: u32| {
            Ok(Window {
                report: PathBuf::from("r.json"),
                player: json!({"frames": 600, "cpu": {
                    "work_ms": {"p50": 3.0, "p95": 4.5},
                    "frame_ms": {"p50": 9.0, "p95": 10.0, "max": 40.0},
                    "work_coverage": {"measured": measured, "unavailable": unavailable},
                    "hitches_over_16ms": 2, "hitch_samples": 599,
                }}),
            })
        };
        let mut results = Results::default();
        results.steady.insert(("Outpost", "high"), window(600, 0));
        results.steady.insert(("Hangar", "high"), window(590, 10));
        let rows = rows(&results);
        let find = |subject: &str, metric: &str| {
            rows.iter()
                .find(|r| r.subject == subject && r.metric == metric)
                .expect("row")
        };
        let outpost = "Outpost · High (Play)";
        assert_eq!(find(outpost, "CPU work p50").status, Status::Pass);
        assert_eq!(find(outpost, "CPU work p95").status, Status::Fail);
        let hitches = find(outpost, "Hitches over 16 ms, steady window");
        assert_eq!((hitches.value, hitches.status), (Some(2.), Status::Fail));
        // Partial coverage keeps the wall bound, which cannot fail.
        let hangar = find("Hangar · High (Play)", "CPU frame p95 (wall bound)");
        assert_eq!(hangar.status, Status::NotMeasured);
        assert!(hangar.note.contains("590 of 600"));
    }

    #[test]
    fn editor_rows_split_frames_by_imports_and_count_slow_frames() {
        let frame = |scenario: &str, cpu: f64, importing: bool, latency: Option<f64>| json!({"scenario": scenario, "sample": {"cpu_ms": cpu, "gpu_ms": 1.0, "importing": importing, "input_to_present_ms": latency}});
        let run = |frames: Vec<Value>| {
            Some(Ok(Output {
                path: PathBuf::from("responsiveness.json"),
                value: json!({"frames": frames, "clicks": {"total": 4, "landed": 3}, "startup": {"firstPresentMs": 900.0}}),
            }))
        };
        let results = Results {
            editor_cold: run(vec![
                frame("warmup", 40., true, None),
                frame("click-toolbar", 9., true, Some(8.)),
                frame("click-toolbar", 30., false, Some(25.)),
            ]),
            editor_warm: run(vec![frame("scene-orbit", 3.5, false, Some(3.))]),
            ..Results::default()
        };
        let rows = rows(&results);
        let find = |subject: &str, metric: &str| {
            rows.iter()
                .find(|r| r.subject == subject && r.metric == metric)
                .expect("row")
        };
        let cold = "Editor · Outpost, during imports (cold)";
        // Only the frame with imports pending counts while importing; warmup never does.
        assert_eq!(find(cold, "Click → present p95").value, Some(8.));
        assert_eq!(find(cold, "Frames over 16.7 ms").value, Some(0.));
        assert_eq!(find(cold, "Missed clicks").status, Status::Fail);
        let warm = "Editor · Outpost, after imports (warm)";
        assert_eq!(
            find(warm, "Typing → present p95").status,
            Status::NotMeasured
        );
        let orbit = "Editor · Outpost Scene orbit (after imports)";
        assert_eq!(
            find(orbit, "Frame minus UI fence wait p95 (wall bound)").value,
            Some(2.5)
        );
        let startup = find(
            "Editor · Outpost",
            "First presented frame (loading view), cold",
        );
        assert_eq!(startup.status, Status::Pass);
    }
}
