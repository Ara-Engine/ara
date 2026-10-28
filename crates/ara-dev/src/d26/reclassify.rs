//! Re-score existing sources and samples. No build, GPU query or process launch.
use super::{
    Section,
    measure::{Output, Results, SCENES, Window},
};
use crate::{
    gpu_guard::{Policy, Report},
    perf::Preset,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{fs, path::Path};

fn json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )?)
}
fn player(path: &Path) -> Result<Window, String> {
    (|| -> Result<Window> {
        let value = json(path)?;
        let raw = if value["format"] == "ara.perf/1" {
            value["player"].clone()
        } else {
            value
        };
        Ok(Window {
            report: path.to_owned(),
            player: raw,
        })
    })()
    .map_err(|e| format!("{e:#}"))
}
fn output(path: &Path) -> Result<Output, String> {
    json(path)
        .map(|value| Output {
            path: path.to_owned(),
            value,
        })
        .map_err(|e| format!("{e:#}"))
}

fn player_file(directory: &Path) -> std::path::PathBuf {
    let report = directory.join("report.json");
    if report.is_file() {
        report
    } else {
        directory.join("player.json")
    }
}

fn results(directory: &Path, original: &Value, load: Report) -> Result<Results> {
    let mut result = Results {
        gpu_load: Some(load),
        device: original["device"].clone(),
        ..Default::default()
    };
    if let Some(rows) = original["rows"].as_array() {
        for row in rows {
            if !row["measurement_window"].is_null() {
                let key = format!(
                    "{}|{}|{}",
                    row["subject"].as_str().unwrap_or(""),
                    row["section"].as_str().unwrap_or(""),
                    row["metric"].as_str().unwrap_or("")
                );
                result.windows.insert(
                    key,
                    serde_json::from_value(row["measurement_window"].clone())?,
                );
            }
        }
    }
    if let Some(skipped) = original["skipped"].as_object() {
        for (name, reason) in skipped {
            let section = match name.as_str() {
                "player" => Section::Player,
                "flip" => Section::Flip,
                "editor" => Section::Editor,
                _ => continue,
            };
            result
                .skipped
                .insert(section, reason.as_str().unwrap_or("skipped").into());
        }
    }
    for scene in &SCENES {
        let stem = scene.name.to_ascii_lowercase();
        let cold = player_file(&directory.join("player").join(format!("{stem}-high-cold")));
        if cold.exists() {
            result.cold.insert(scene.name, player(&cold));
        }
        for preset in Preset::ALL {
            let path = player_file(
                &directory
                    .join("player")
                    .join(format!("{stem}-{}", preset.name())),
            );
            if path.exists() {
                result
                    .steady
                    .insert((scene.name, preset.name()), player(&path));
            }
        }
        let path = directory
            .join("captures")
            .join(scene.project)
            .join(scene.look)
            .join("evidence")
            .join(format!("{}-timings.json", scene.look));
        if path.exists() {
            result.flip.insert(scene.name, output(&path));
        } else if let Some(source) = original["rows"]
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|row| {
                    row["subject"]
                        .as_str()
                        .is_some_and(|subject| subject.starts_with(scene.name))
                        && row["metric"].as_str().is_some_and(|m| m.contains("RMSE"))
                })
            })
            .and_then(|row| row["source"].as_str())
        {
            result.flip.insert(scene.name, output(Path::new(source)));
        }
    }
    for (label, cold) in [("cold", true), ("warm", false)] {
        let path = directory
            .join("editor")
            .join(label)
            .join("responsiveness.json");
        if path.exists() {
            if cold {
                result.editor_cold = Some(output(&path));
            } else {
                result.editor_warm = Some(output(&path));
            }
        }
    }
    Ok(result)
}

fn backup(directory: &Path, name: &str) -> Result<()> {
    let source = directory.join(name);
    let backup = directory.join(format!("{name}.before-reclassify"));
    if !backup.exists() {
        fs::copy(source, backup)?;
    }
    Ok(())
}

pub(super) fn run(root: &Path, directory: &Path, policy: Policy) -> Result<bool> {
    let policy = policy.validate()?;
    let original = json(&directory.join("report.json"))?;
    ensure!(original["format"] == "ara.d26/1", "not a D26 reading");
    let mut gpu = json(&directory.join("gpu-load.json"))?;
    let mut previous: Report = serde_json::from_value(gpu["reading"].clone())?;
    // Copying an old reading must preserve the GPU evidence file's timestamps.
    // Its last save bounds a sampler which died before the timed run finished.
    let end = fs::metadata(directory.join("gpu-load.json"))?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    previous.recover_legacy_monitoring(end);
    let load = previous.reclassify(policy)?;
    let results = results(directory, &original, load.clone())?;
    // Preserve original evidence once. Source metrics, binaries and source IDs are untouched.
    for name in ["report.json", "report.html", "gpu-load.json"] {
        backup(directory, name)?;
    }
    let verdict = super::write_value(directory, &original["run"], &results)?;
    gpu["status"] = serde_json::json!(load.status());
    gpu["reading"] = serde_json::to_value(&load)?;
    gpu["threshold_percent"] = serde_json::json!(policy.threshold_percent);
    gpu["sustained_samples"] = serde_json::json!(policy.sustained_samples);
    gpu["dwm_allowance_percent"] = serde_json::json!(policy.dwm_allowance_percent);
    gpu["diagnostic"] = serde_json::json!(if load.invalid() {
        Some(crate::gpu_guard::CODE)
    } else {
        None
    });
    gpu["reclassified_by"] = serde_json::json!(crate::git::text(root, ["rev-parse", "HEAD"])?);
    fs::write(
        directory.join("gpu-load.json"),
        serde_json::to_vec_pretty(&gpu)?,
    )?;
    println!(
        "{verdict}\nReclassified: {}",
        directory.join("report.html").display()
    );
    if verdict.contains("INVALID") {
        return Err(crate::gpu_guard::invalid_error(
            "some source windows overlap contamination; clean windows retain their judgments"
                .into(),
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn saved_reading_recovers_clean_judgments_without_launching_or_changing_metrics() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = root.join("target/ara-dev/tests").join(format!(
            "reclassify-{}-{}",
            std::process::id(),
            super::super::validity::unix_ms().unwrap()
        ));
        fs::create_dir_all(&directory).unwrap();
        let mut windows = Vec::new();
        let mut sources = Vec::new();
        for (preset, start, end) in [("low", 2000, 4000), ("medium", 7000, 9000)] {
            let filename = if preset == "medium" {
                "player.json"
            } else {
                "report.json"
            };
            let source = directory
                .join("player")
                .join(format!("outpost-{preset}/{filename}"));
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            let player = json!({"completed":true,"frames":2,"pass_presence":{"format":"ara.timing.passes/1","expected":"raster","status":"valid","required_passes":[],"timed_frames":2,"rt_present_frames":0},"gpu_span_ms":{"p50":1.,"p95":1.,"samples":2},"gpu_span_coverage":{"measured":2,"unavailable":0},"readiness":{"required":true,"state":"ready","renderer_ready":true,"renderer":{"raster":true,"gi_enabled":true},"ready_frame":1,"warmup_frames":1,"gpu_start_frame":3,"time_to_ready_ms":1.,"timeout_ms":1000.}});
            let value = if preset == "medium" {
                player
            } else {
                json!({"format":"ara.perf/1","player":player})
            };
            let bytes = serde_json::to_vec(&value).unwrap();
            fs::write(&source, &bytes).unwrap();
            sources.push((source, bytes));
            let subject = format!(
                "Outpost · {}",
                if preset == "low" { "Low" } else { "Medium" }
            );
            for (section, metric) in [
                ("GPU frame", "GPU frame p50"),
                ("GPU frame", "GPU frame p95"),
                ("Readiness", "Time to readiness"),
            ] {
                windows.push(json!({"section":section,"metric":metric,"subject":subject,"status":"invalid","measurement_window":{"start_unix_ms":start,"end_unix_ms":end,"basis":"fixture UTC window"}}));
            }
        }
        let original = json!({"format":"ara.d26/1","run":{"sourceHead":"original-engine-commit","createdAt":"fixture","profile":"perf"},"rows":windows});
        let samples:Vec<_>=(1..=12).map(|at|json!({"unix_ms":at*1000,"foreign_percent":if at<=5{30.}else{0.},"foreign":[{"pid":99,"birth":1,"name":"unknown-game.exe","percent":if at<=5{30.}else{0.},"known":false}]})).collect();
        let gpu = json!({"format":"ara.gpu-load/1","status":"invalid","reading":{"sampler":"windows_pdh","samples":samples,"contaminants":[],"errors":[],"quiet_wait_seconds":0.}});
        let original_bytes = serde_json::to_vec(&original).unwrap();
        fs::write(directory.join("report.json"), &original_bytes).unwrap();
        fs::write(directory.join("report.html"), "original HTML").unwrap();
        fs::write(
            directory.join("gpu-load.json"),
            serde_json::to_vec(&gpu).unwrap(),
        )
        .unwrap();
        for _ in 0..2 {
            let error = run(&root, &directory, Policy::default()).unwrap_err();
            assert!(crate::gpu_guard::is_invalid(&error));
            let report = json(&directory.join("report.json")).unwrap();
            assert_eq!(report["status"], "partially_invalid");
            assert_eq!(report["run"]["sourceHead"], "original-engine-commit");
            for row in report["rows"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["metric"] == "GPU frame p95")
            {
                match row["subject"].as_str().unwrap() {
                    "Outpost · Low" => assert_eq!(row["status"], "invalid"),
                    "Outpost · Medium" => {
                        assert_eq!(row["status"], "pass");
                        assert_eq!(row["value"], 1.);
                    }
                    _ => {}
                }
            }
            assert!(
                fs::read_to_string(directory.join("report.html"))
                    .unwrap()
                    .contains("INVALID rows")
            );
        }
        assert_eq!(
            fs::read(directory.join("report.json.before-reclassify")).unwrap(),
            original_bytes
        );
        for (path, bytes) in sources {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}
