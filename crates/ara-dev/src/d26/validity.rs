//! Apply GPU intervals to source windows, never aggregate a PID's separated bursts.
use super::rows::{Row, Status};
use crate::gpu_guard::{Interval, Report};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// A measured source's UTC bounds and how they were established.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Window {
    #[serde(flatten)]
    pub interval: Interval,
    pub basis: String,
}

pub(crate) fn unix_ms() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64)
}

/// Record process bounds around the actual child call, after GPU admission.
pub(crate) fn record(path: &Path, start: u64) -> Result<()> {
    let window = Window {
        interval: Interval {
            start_unix_ms: start,
            end_unix_ms: unix_ms()?,
        },
        basis: "orchestrator UTC child-call bounds".into(),
    };
    fs::write(path, serde_json::to_vec_pretty(&window)?)?;
    Ok(())
}

fn milliseconds(time: SystemTime) -> Result<u64> {
    Ok(time.duration_since(UNIX_EPOCH)?.as_millis() as u64)
}
fn metric_file(source: &Path) -> std::path::PathBuf {
    let player = source
        .parent()
        .unwrap_or(Path::new("."))
        .join("player.json");
    if source.file_name().is_some_and(|n| n == "report.json") && player.is_file() {
        player
    } else {
        source.to_owned()
    }
}

/// Legacy files have no absolute UTC fields. Preserve a conservative, labelled
/// source envelope using the original log creation and metrics-write times.
fn source_window(source: &Path) -> Result<Window> {
    let parent = source.parent().context("source directory")?;
    for sidecar in ["measurement-window.json", "editor.window.json"] {
        if let Ok(bytes) = fs::read(parent.join(sidecar)) {
            return Ok(serde_json::from_slice(&bytes)?);
        }
    }
    let metric = metric_file(source);
    let end = milliseconds(fs::metadata(&metric)?.modified()?)?;
    let log = if metric.file_name().is_some_and(|n| n == "player.json") {
        parent.join("player.log")
    } else if source
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.ends_with("-timings.json"))
    {
        let ancestors: Vec<_> = parent.ancestors().take(4).collect();
        ensure!(ancestors.len() == 4, "capture source directory shape");
        let project = ancestors[2]
            .file_name()
            .context("capture project")?
            .to_string_lossy();
        let look = ancestors[1]
            .file_name()
            .context("capture look")?
            .to_string_lossy();
        let log = ancestors[3].join(format!("{project}-{look}.log"));
        if let Ok(bytes) = fs::read(log.with_extension("window.json")) {
            return Ok(serde_json::from_slice(&bytes)?);
        }
        log
    } else {
        parent.join("editor.log")
    };
    let start = milliseconds(
        fs::metadata(&log)
            .context("no absolute source window or original launch log")?
            .created()?,
    )?;
    ensure!(
        start <= end,
        "source timestamps no longer describe the original measurement"
    );
    Ok(Window{interval:Interval{start_unix_ms:start,end_unix_ms:end},basis:"legacy source envelope: original launch-log creation to metrics-file write (conservative)".into()})
}

fn source_windows(source: &Path) -> Result<(Window, Option<Window>)> {
    let window = source_window(source)?;
    let mut steady = window.clone();
    let metric = metric_file(source);
    let mut has_steady = false;
    if metric.file_name().is_some_and(|n| n == "player.json") {
        let player: Value = serde_json::from_slice(&fs::read(&metric)?)?;
        let duration = player["cpu"]["frame_samples_ms"]
            .as_array()
            .and_then(|frames| frames.iter().map(Value::as_f64).collect::<Option<Vec<_>>>())
            .map(|frames| frames.into_iter().sum::<f64>());
        if let Some(duration) = duration.filter(|n| n.is_finite() && *n >= 0.) {
            let end = milliseconds(fs::metadata(&metric)?.modified()?)?;
            // PDH is sampled at ~1 Hz. Preserve a one-sample safety envelope
            // for legacy metrics serialization, rather than invent exact GPU UTC.
            let start = end.saturating_sub(duration.ceil() as u64 + 1000);
            steady.interval.start_unix_ms = steady.interval.start_unix_ms.max(start);
            steady.interval.end_unix_ms = steady.interval.end_unix_ms.min(end);
            steady.basis.push_str("; steady frame-duration envelope ending at metrics write, with one PDH-sample safety margin");
            has_steady = true;
        }
    }
    Ok((window, has_steady.then_some(steady)))
}

fn window(row: &Row, source: &(Window, Option<Window>)) -> Window {
    let mut window = if row.metric.starts_with("GPU frame ") {
        source.1.as_ref().unwrap_or(&source.0).clone()
    } else {
        source.0.clone()
    };
    if row.section == "Startup"
        && (row.metric.starts_with("First presented") || row.metric.starts_with("Full first scene"))
        && let Some(duration) = row.value.filter(|v| v.is_finite() && *v >= 0.)
    {
        window.interval.end_unix_ms = window.interval.end_unix_ms.min(
            window
                .interval
                .start_unix_ms
                .saturating_add(duration.ceil() as u64),
        );
        window
            .basis
            .push_str("; startup endpoint relative to launch");
    }
    window
}

/// Unknown source timing cannot prove a gated reading clean. It stays unmeasured;
/// only an observed overlap is INVALID (monitor errors invalidate all evidence).
pub(super) fn apply(
    rows: &mut [Row],
    report: &Report,
    saved: &BTreeMap<String, Window>,
) -> Result<usize> {
    let mut sources = BTreeMap::<String, Result<(Window, Option<Window>), String>>::new();
    let mut invalid = 0;
    for row in rows {
        if !report.errors.is_empty() {
            row.invalidate("GPU monitoring was unavailable");
            invalid += 1;
            continue;
        }
        if row.value.is_none() && row.status != Status::Fail {
            continue;
        }
        let key = format!("{}|{}|{}", row.subject, row.section, row.metric);
        let measured = if let Some(recorded) = saved.get(&key) {
            Ok(recorded.clone())
        } else {
            match &row.source {
                Some(source) => sources
                    .entry(source.clone())
                    .or_insert_with(|| {
                        source_windows(Path::new(source)).map_err(|e| format!("{e:#}"))
                    })
                    .as_ref()
                    .map(|source| window(row, source))
                    .map_err(Clone::clone),
                None => Err("no measurement source".into()),
            }
        };
        match measured {
            Ok(window) => {
                ensure!(
                    window.interval.start_unix_ms <= window.interval.end_unix_ms,
                    "invalid saved source window for {key}"
                );
                if let (Some(first), Some(last)) = (report.samples.first(), report.samples.last()) {
                    let coverage = Interval {
                        start_unix_ms: first.interval_start_unix_ms,
                        end_unix_ms: last.unix_ms,
                    };
                    if (window.interval.start_unix_ms < coverage.start_unix_ms
                        || window.interval.end_unix_ms > coverage.end_unix_ms)
                        && !row.gates.is_empty()
                    {
                        row.status = Status::NotMeasured;
                        for gate in &mut row.gates {
                            gate.status = Status::NotMeasured;
                        }
                        row.note.push_str(
                            "; source window falls outside saved GPU monitoring coverage",
                        );
                        continue;
                    }
                }
                let overlaps = report.overlaps(window.interval);
                row.measurement_window = Some(window);
                if overlaps {
                    row.invalidate("measurement window overlaps foreign GPU contamination");
                    invalid += 1;
                }
            }
            Err(reason) if !row.gates.is_empty() => {
                row.status = Status::NotMeasured;
                for gate in &mut row.gates {
                    gate.status = Status::NotMeasured;
                }
                row.note.push_str(&format!(
                    "; GPU contamination coverage unavailable: {reason}"
                ));
            }
            Err(_) => {}
        }
    }
    Ok(invalid)
}

#[cfg(test)]
mod tests {
    use super::super::rows::Judged;
    use super::*;

    fn row(section: &'static str, metric: &str) -> Row {
        Row {
            section,
            subject: "Outpost".into(),
            metric: metric.into(),
            value: Some(3.),
            unit: "ms",
            bound: false,
            gates: vec![Judged {
                gate: "D26",
                limit: 4.,
                status: Status::Pass,
            }],
            status: Status::Pass,
            note: String::new(),
            source: None,
            measurement_window: None,
        }
    }

    #[test]
    fn gpu_uses_steady_bounds_cpu_keeps_arrival_and_play_keeps_probe_bounds() {
        let full = Window {
            interval: Interval {
                start_unix_ms: 1000,
                end_unix_ms: 9000,
            },
            basis: "child bounds".into(),
        };
        let steady = Window {
            interval: Interval {
                start_unix_ms: 6000,
                end_unix_ms: 8000,
            },
            basis: "steady bounds".into(),
        };
        let sources = (full.clone(), Some(steady.clone()));
        assert_eq!(
            window(&row("GPU frame", "GPU frame p95"), &sources).interval,
            steady.interval
        );
        assert_eq!(
            window(&row("CPU frame", "CPU work p95"), &sources).interval,
            full.interval
        );
        assert_eq!(
            window(&row("Startup", "Play click → first Game frame"), &sources).interval,
            full.interval
        );
        assert_eq!(
            window(&row("Startup", "First presented frame, warm"), &sources)
                .interval
                .end_unix_ms,
            1003
        );
    }

    #[test]
    fn only_overlapping_rows_are_invalid_and_unknown_timing_never_passes() {
        let mut rows = vec![
            row("GPU frame", "GPU frame p50"),
            row("CPU frame", "CPU work p50"),
            row("Startup", "unknown"),
        ];
        let report = Report {
            classification_version: 2,
            intervals: vec![
                Interval {
                    start_unix_ms: 1500,
                    end_unix_ms: 2500,
                },
                Interval {
                    start_unix_ms: 7500,
                    end_unix_ms: 8500,
                },
            ],
            ..Default::default()
        };
        let saved = BTreeMap::from([
            (
                "Outpost|GPU frame|GPU frame p50".into(),
                Window {
                    interval: Interval {
                        start_unix_ms: 2000,
                        end_unix_ms: 3000,
                    },
                    basis: "fixture".into(),
                },
            ),
            (
                "Outpost|CPU frame|CPU work p50".into(),
                Window {
                    interval: Interval {
                        start_unix_ms: 4000,
                        end_unix_ms: 6000,
                    },
                    basis: "fixture".into(),
                },
            ),
        ]);
        assert_eq!(apply(&mut rows, &report, &saved).unwrap(), 1);
        assert_eq!(rows[0].status, Status::Invalid);
        assert_eq!(rows[1].status, Status::Pass);
        assert_eq!(rows[2].status, Status::NotMeasured);
    }

    #[test]
    fn recorded_child_bounds_and_frame_duration_recover_source_windows() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/tests")
            .join(format!(
                "source-window-{}-{}",
                std::process::id(),
                unix_ms().unwrap()
            ));
        fs::create_dir_all(&directory).unwrap();
        let start = unix_ms().unwrap().saturating_sub(10_000);
        fs::write(
            directory.join("player.json"),
            r#"{"cpu":{"frame_samples_ms":[5,5]}}"#,
        )
        .unwrap();
        record(&directory.join("measurement-window.json"), start).unwrap();
        let (full, steady) = source_windows(&directory.join("report.json")).unwrap();
        assert_eq!(full.interval.start_unix_ms, start);
        let steady = steady.unwrap();
        assert!(steady.interval.start_unix_ms > full.interval.start_unix_ms + 8000);
        assert!(steady.interval.end_unix_ms <= full.interval.end_unix_ms);
        assert!(steady.basis.contains("safety margin"));
    }

    #[test]
    fn partial_monitoring_coverage_cannot_pass_a_gated_row() {
        let mut rows = vec![row("GPU frame", "GPU frame p95")];
        let report:Report=serde_json::from_value(serde_json::json!({"sampler":"windows_pdh","samples":[{"unix_ms":2000,"interval_start_unix_ms":1000,"foreign_percent":0.,"foreign":[]}],"contaminants":[],"errors":[],"quiet_wait_seconds":0.})).unwrap();
        let saved = BTreeMap::from([(
            "Outpost|GPU frame|GPU frame p95".into(),
            Window {
                interval: Interval {
                    start_unix_ms: 500,
                    end_unix_ms: 1500,
                },
                basis: "partially covered".into(),
            },
        )]);
        assert_eq!(apply(&mut rows, &report, &saved).unwrap(), 0);
        assert_eq!(rows[0].status, Status::NotMeasured);
    }
}
