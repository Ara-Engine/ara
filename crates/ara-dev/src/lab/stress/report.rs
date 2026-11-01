use super::{
    contract::*,
    gates::{self, Verdict},
    write_json,
};
use anyhow::Result;
use serde_json::json;
use std::{collections::BTreeMap, fmt::Write, fs, path::Path};

pub fn write(gate: u8, suite: &Suite, output: &Path) -> Result<()> {
    let directory = output.join(format!("G{gate}"));
    fs::create_dir_all(&directory)?;
    let mut rows = Vec::new();
    let mut verdicts = Vec::new();
    let mut text = format!(
        "# G{gate} renderer gate\n\nContract: council-v3 architecture v9, sections 5 and 6.3. Cost uses uncapped replay; quality uses capped frames. Missing evidence is **n/a**, never zero. PNGs are bounded to 1920x1080; scoring uses native frames.\n\nThis is one-device evidence. Promotion additionally requires the complete case list on **both** target GPUs.\n\n| Case | Verdict | Control full GPU span p50 / p99 / max / average (ms) | Worst sampled region FLIP |\n|---|---|---|---|\n"
    );
    for case in suite.cases.iter().filter(|c| c.gates.contains(&gate)) {
        let base = output.join("cases").join(&case.id);
        let read = |kind: &str| -> Result<RunResult> {
            let run: RunResult =
                serde_json::from_slice(&fs::read(base.join(kind).join("run.json"))?)?;
            anyhow::ensure!(
                serde_json::to_value(&run.case)? == serde_json::to_value(case)?,
                "receipt does not match the versioned case"
            );
            Ok(run)
        };
        let a = read("uncapped");
        let b = read("capped");
        let pass_sum = a.as_ref().ok().and_then(|r| {
            let samples: Option<Vec<_>> = r
                .frames
                .iter()
                .map(|f| {
                    f.stages
                        .values()
                        .filter_map(|s| s.gpu_ms)
                        .reduce(|a, b| a + b)
                })
                .collect();
            samples.and_then(distribution)
        });
        let control = a.as_ref().ok().and_then(|r| {
            r.frames
                .iter()
                .map(|f| f.gpu_frame_ms)
                .collect::<Option<Vec<_>>>()
                .and_then(distribution)
        });
        let flip = b.as_ref().ok().and_then(|r| {
            r.frames
                .iter()
                .flat_map(|f| f.quality.region_flip.values().copied())
                .reduce(f64::max)
        });
        if let Ok(run) = &b {
            let captures = directory.join("captures").join(&case.id);
            fs::create_dir_all(&captures)?;
            for frame in &run.frames {
                for source in [&frame.capture, &frame.reference].into_iter().flatten() {
                    let source = Path::new(source);
                    if source.is_file() {
                        let name = source
                            .file_name()
                            .ok_or_else(|| anyhow::anyhow!("capture filename"))?;
                        let destination = captures.join(name);
                        if destination.exists() {
                            fs::remove_file(&destination)?;
                        }
                        if fs::hard_link(source, &destination).is_err() {
                            fs::copy(source, &destination)?;
                        }
                    }
                }
            }
        }
        let score = match (&a, &b) {
            (Ok(a), Ok(b)) => gates::score(gate, a, b),
            _ => Err(anyhow::anyhow!(
                "two runs unavailable: uncapped={}, capped={}",
                a.as_ref().err().map_or("ok".into(), ToString::to_string),
                b.as_ref().err().map_or("ok".into(), ToString::to_string)
            )),
        };
        let verdict = score.as_ref().map_or(Verdict::Unavailable, |s| s.verdict);
        verdicts.push(verdict);
        let mut timing = control.map_or("n/a".into(), |d| {
            format!(
                "{:.4} / {:.4} / {:.4} / {:.4}",
                d.p50, d.p99, d.max, d.average
            )
        });
        if control.is_some() && !a.as_ref().is_ok_and(|r| r.timing_valid) {
            timing.push_str(" (INVALID timing)");
        }
        writeln!(
            text,
            "| {} | {verdict:?} | {timing} | {} |",
            case.id,
            flip.map_or("n/a".into(), |v| format!("{v:.6}"))
        )?;
        let stages = a.as_ref().ok().map(|r| {
            let mut stages = BTreeMap::new();
            for f in &r.frames {
                for name in f.stages.keys() {
                    stages.entry(name.clone()).or_insert_with(|| {
                        let samples: Option<Vec<_>> = r
                            .frames
                            .iter()
                            .map(|f| f.stages.get(name)?.gpu_ms)
                            .collect();
                        samples.and_then(distribution)
                    });
                }
            }
            stages
        });
        let attribution = if gate == 5 {
            a.as_ref().ok().zip(b.as_ref().ok()).and_then(|(a, b)| {
                gates::validate_pair(a, b)
                    .ok()
                    .map(|()| super::attribution::route(a, b))
            })
        } else {
            None
        };
        rows.push(json!({"case":case,"verdict":verdict,"score":score.as_ref().ok(),"error":score.as_ref().err().map(ToString::to_string),"control_gpu_span_ms":control,"control_pass_sum_ms":pass_sum,"stage_gpu_ms":stages,"uncapped":base.join("uncapped/run.json"),"capped":base.join("capped/run.json"),"worst_sampled_region_flip":flip,"timing_valid":a.as_ref().ok().map(|r|r.timing_valid),"G5_attribution_only":attribution}));
    }
    let verdict = gates::combine(verdicts);
    let worst_sun_flip = rows
        .iter()
        .filter(|r| r["case"]["recipe"] == "sun")
        .filter_map(|r| {
            r["worst_sampled_region_flip"]
                .as_f64()
                .map(|v| (r["case"]["id"].clone(), v))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let worst_sun_cost = rows
        .iter()
        .filter(|r| r["case"]["recipe"] == "sun" && r["timing_valid"] == true)
        .filter_map(|r| {
            r["control_gpu_span_ms"]["average"]
                .as_f64()
                .map(|v| (r["case"]["id"].clone(), v))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if gate == 2 {
        writeln!(
            text,
            "\nWorst sampled sun band (FLIP): {}. Worst measured control cost band (average GPU span): {}. Missing bands remain required.\n",
            worst_sun_flip
                .as_ref()
                .map_or("n/a".into(), |v| format!("{}: {:.6}", v.0, v.1)),
            worst_sun_cost
                .as_ref()
                .map_or("n/a".into(), |v| format!("{}: {:.4} ms", v.0, v.1))
        )?;
    }
    writeln!(
        text,
        "\nOverall: **{verdict:?}**. No default changes.\n\nControl pass sums are descriptive and do not substitute for complete-frame timestamps or true uncapped demand. Check `timing_valid` before using timings. Finite-budget references are labelled; sampled frames cannot certify per-frame quality or temporal stability.\n"
    )?;
    for row in &rows {
        writeln!(
            text,
            "## {}\n",
            row["case"]["id"].as_str().unwrap_or("case")
        )?;
        if let Some(lines) = row["score"]["lines"].as_array() {
            writeln!(
                text,
                "| Line | Value | Pass | Kill above | Verdict |\n|---|---|---|---|---|"
            )?;
            for line in lines {
                writeln!(
                    text,
                    "| {} | {} | {} {} | {} | {} |",
                    line["metric"].as_str().unwrap_or("metric"),
                    number(&line["observed"]),
                    line["relation"].as_str().unwrap_or("<="),
                    number(&line["pass_limit"]),
                    number(&line["kill_limit"]),
                    line["verdict"].as_str().unwrap_or("unavailable")
                )?;
            }
        } else {
            writeln!(text, "{}", row["error"].as_str().unwrap_or("n/a"))?;
        }
        writeln!(
            text,
            "\nRaw runs: `{}` and `{}`.\n",
            row["uncapped"].as_str().unwrap_or("n/a"),
            row["capped"].as_str().unwrap_or("n/a")
        )?;
    }
    fs::write(directory.join("RESULT.md"), text)?;
    write_json(
        &directory.join("RESULT.json"),
        &json!({"version":VERSION,"gate":gate,"verdict":verdict,"promotion":false,"requires_both_gpus":true,"worst_sun_flip_band":worst_sun_flip,"worst_sun_control_average_band":worst_sun_cost,"cases":rows}),
    )?;
    Ok(())
}
fn number(v: &serde_json::Value) -> String {
    v.as_f64().map_or("n/a".into(), |v| format!("{v:.6}"))
}
