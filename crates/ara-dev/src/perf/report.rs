//! Numeric reports and compatible-baseline comparisons; missing measurements stay missing.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
struct Report {
    format: String,
    #[serde(default)]
    provenance: Value,
    identity: Value,
    build_seconds: f64,
    elapsed_seconds: f64,
    metrics: BTreeMap<String, f64>,
    deltas: BTreeMap<String, Delta>,
    player: Value,
}

#[derive(Serialize, Deserialize)]
struct Delta {
    before: f64,
    after: f64,
    change: f64,
    percent: Option<f64>,
}

fn numbers(value: &Value, prefix: &str, result: &mut BTreeMap<String, f64>) {
    match value {
        Value::Number(n) => {
            if let Some(n) = n.as_f64().filter(|n| n.is_finite()) {
                result.insert(prefix.into(), n);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "samples" | "frames" | "warmup" | "epoch" | "pid"
                ) {
                    continue;
                }
                numbers(
                    value,
                    &if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    },
                    result,
                );
            }
        }
        // Raw samples are retained in player; don't turn sample indices into table rows.
        _ => {}
    }
}

fn differences(
    before: &BTreeMap<String, f64>,
    after: &BTreeMap<String, f64>,
) -> BTreeMap<String, Delta> {
    after
        .iter()
        .filter_map(|(key, after)| {
            before.get(key).map(|before| {
                (
                    key.clone(),
                    Delta {
                        before: *before,
                        after: *after,
                        change: after - before,
                        percent: (*before != 0.).then(|| (after - before) / before * 100.),
                    },
                )
            })
        })
        .collect()
}

pub(super) struct Destination<'a> {
    pub root: &'a Path,
    pub base: &'a Path,
    pub directory: &'a Path,
    pub scene: &'a Path,
    pub options: &'a super::Options,
    pub executable: &'a Path,
}

impl Destination<'_> {
    pub(super) fn publish(
        &self,
        player: Value,
        build_seconds: f64,
        elapsed_seconds: f64,
    ) -> Result<()> {
        let Self {
            root,
            base,
            directory,
            scene,
            options,
            executable,
        } = *self;
        ensure!(
            player["format"] == "ara.player.perf/1",
            "unsupported player metrics format"
        );
        ensure!(
            player["validation"] == false,
            "perf requires an optimized player with validation disabled"
        );
        ensure!(
            player["frames"].as_u64() == Some(u64::from(options.frames)),
            "incomplete perf measurement window"
        );
        ensure!(
            player["gpu_passes"]
                .as_object()
                .is_some_and(|passes| !passes.is_empty()),
            "missing GPU timestamp measurements"
        );
        let launch: Value = serde_json::from_slice(&fs::read(scene.join("ara-launch.json"))?)?;
        let pack = scene.join(launch["pack"].as_str().context("launch pack path")?);
        let pack_hash = crate::git::hash_file(root, &pack)?;
        let preset = player
            .get("quality")
            .and_then(Value::as_str)
            .unwrap_or(options.preset.name());
        let identity = serde_json::json!({"pack":pack_hash,"preset": preset,"width":options.width,"height":options.height,"profile":options.profile,"camera":player.get("camera")});
        let previous = base.join(format!(
            "previous-{}-{}x{}-{}.json",
            preset, options.width, options.height, options.profile
        ));
        let baseline = options.baseline.as_ref().unwrap_or(&previous);
        let mut metrics = BTreeMap::new();
        numbers(&player, "", &mut metrics);
        let deltas = if options.scoreboard {
            BTreeMap::new()
        } else if baseline.is_file() {
            let before: Report = serde_json::from_slice(&fs::read(baseline)?)?;
            if options.baseline.is_some() {
                ensure!(
                    before.identity == identity,
                    "baseline pack, preset, resolution, camera or profile differs"
                );
            }
            if before.identity == identity {
                differences(&before.metrics, &metrics)
            } else {
                eprintln!("Previous run has a different scene/settings; starting a new baseline.");
                BTreeMap::new()
            }
        } else {
            ensure!(options.baseline.is_none(), "named baseline does not exist");
            BTreeMap::new()
        };
        let report = Report {
            format: "ara.perf/1".into(),
            provenance: serde_json::json!({
                "executable":executable,
                "binary_hash":crate::git::hash_file(root, executable)?,
                "source_head":crate::git::text(root, ["rev-parse", "HEAD"])?,
                "source_dirty":!crate::git::text(root, ["status", "--porcelain"])?.is_empty()
            }),
            identity,
            build_seconds,
            elapsed_seconds,
            metrics,
            deltas,
            player,
        };
        let bytes = serde_json::to_vec_pretty(&report)?;
        fs::write(directory.join("report.json"), &bytes)?;
        if options.scoreboard {
            return Ok(());
        }
        fs::write(&previous, &bytes)?;
        if options.json {
            if options.keep_alive {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!("{}", String::from_utf8(bytes)?);
            }
        } else {
            println!(
                "{} {}x{} | build {:.2}s | total {:.2}s",
                report.identity["preset"].as_str().unwrap_or("unknown"),
                options.width,
                options.height,
                build_seconds,
                elapsed_seconds
            );
            for (group, heading) in [
                ("cpu", "CPU phases / frame time (ms), FPS"),
                ("gpu_passes", "GPU timestamps per pass (ms)"),
                (
                    "gpu_other",
                    "GPU frame accounting (ms, bytes, pending textures)",
                ),
            ] {
                println!("{heading}");
                println!(
                    "{:<57} {:>9} {:>9} {:>9} {:>10}",
                    "Phase / pass", "p50", "p95", "max", "delta p50"
                );
                if let Some(rows) = report.player[group].as_object() {
                    for (name, stats) in rows {
                        let value = |key: &str| {
                            stats[key]
                                .as_f64()
                                .map(|value| format!("{value:.3}"))
                                .unwrap_or_else(|| "-".into())
                        };
                        let change = report
                            .deltas
                            .get(&format!("{group}.{name}.p50"))
                            .map(|delta| format!("{:+.3}", delta.change))
                            .unwrap_or_else(|| "-".into());
                        println!(
                            "{name:<57} {:>9} {:>9} {:>9} {change:>10}",
                            value("p50"),
                            value("p95"),
                            value("max")
                        );
                    }
                }
            }
            if let Some(coverage) = report.player.get("gpu_coverage") {
                println!("GPU coverage: {coverage}");
            }
            println!("Startup (ms): {}", report.player["startup"]);
            if let Some(error) = report.player["shaders"]["last_error"].as_str() {
                eprintln!("Shader reload retained last good pipelines: {error}");
            }
            println!("JSON: {}", directory.join("report.json").display());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diffs_match_names_and_keep_absent_and_zero_baselines_honest() {
        let before = BTreeMap::from([
            ("gpu_other.raster_ms.p50".into(), 4.),
            ("cpu".into(), 0.),
            ("removed".into(), 1.),
        ]);
        let after = BTreeMap::from([
            ("gpu_other.raster_ms.p50".into(), 3.),
            ("cpu".into(), 1.),
            ("new".into(), 2.),
        ]);
        let diff = differences(&before, &after);
        assert_eq!(diff.len(), 2);
        assert_eq!(diff["gpu_other.raster_ms.p50"].change, -1.);
        assert_eq!(diff["gpu_other.raster_ms.p50"].percent, Some(-25.));
        assert_eq!(diff["cpu"].percent, None);
        let mut metrics = BTreeMap::new();
        numbers(
            &serde_json::json!({"passes":{"ReSTIR.Trace":{"p50":3.}},"gpu_other":{"raster_ms":{"p50":4.},"texture_upload_ms":{"p50":2.5},"landscape_ms":null},"samples":[1,2],"missing":null}),
            "",
            &mut metrics,
        );
        assert_eq!(
            metrics,
            BTreeMap::from([
                ("passes.ReSTIR.Trace.p50".into(), 3.),
                ("gpu_other.raster_ms.p50".into(), 4.),
                ("gpu_other.texture_upload_ms.p50".into(), 2.5),
            ])
        );
    }
}
