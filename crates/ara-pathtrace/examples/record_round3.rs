//! Assemble reproducible measurements without copying HDR caches or asset inputs.
use anyhow::Result;
use ara_pathtrace::artifacts::output_path;
use serde_json::{Value, json};
use std::path::Path;

fn read(root: &Path, name: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join(name))?)?)
}

fn load_samples(root: &Path) -> Result<Vec<Value>> {
    let data = std::fs::read_to_string(root.join(".sn/round3-shared-gpu-load.csv"))?;
    let mut samples = Vec::new();
    for row in data.lines().skip(1) {
        let columns = row.split(',').map(str::trim).collect::<Vec<_>>();
        // A stopped line-oriented sampler can leave one partial final row.
        if columns.len() != 7 {
            continue;
        }
        let number = |i: usize| -> Result<f64> {
            Ok(columns[i]
                .split_whitespace()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Missing GPU telemetry field"))?
                .parse()?)
        };
        samples.push(json!({"local_timestamp":columns[0],"gpu_percent":number(1)?,"memory_percent":number(2)?,"used_mib":number(3)?,"watts":number(4)?,"sm_mhz":number(5)?,"memory_mhz":number(6)?}));
    }
    Ok(samples)
}
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let prior = read(root, "measurements/lumen-round2.json")?;
    let mut after = serde_json::Map::new();
    for (key, file) in [
        ("sponza_quality", "sponza-quality.json"),
        ("sponza_reference", "sponza-reference.json"),
        ("sponza_timings", "sponza-timings.json"),
        ("lighting_stability", "lighting-stability.json"),
        ("robust_quality", "robust-quality.json"),
        ("robust_timings", "robust-timings.json"),
        ("night_noise", "night-noise.json"),
        ("resolution_noise", "denoiser-resolution.json"),
        ("many_quality", "many_lights-quality.json"),
        ("many_timings", "many_lights-timings.json"),
        ("many_noise", "many-lights-noise.json"),
        ("beauty_quality", "beauty_cc0-quality.json"),
        ("beauty_timings", "beauty_cc0-timings.json"),
        ("diagnostic_quality", "quality-targets.json"),
        ("validation", "validation.json"),
    ] {
        after.insert(
            key.into(),
            serde_json::from_slice(&std::fs::read(output_path(
                root,
                format!("measurements/{file}"),
            )?)?)?,
        );
    }
    let mut trials = serde_json::Map::new();
    for file in [
        "round3-fullpath-quality.json",
        "round3-fullpath-timings.json",
        "round3-coarse-quality.json",
        "round3-coarse-timings.json",
    ] {
        if root.join(".sn").join(file).exists() {
            trials.insert(file.into(), read(root, &format!(".sn/{file}"))?);
        }
    }
    let mut gates = serde_json::Map::new();
    for stage in [
        "robust-quality",
        "night-noise",
        "sponza-reference",
        "sponza-quality",
        "lighting-sequence",
        "many-lights",
        "many-noise",
        "beauty",
        "quality",
        "capture",
        "content",
        "robust-timings",
        "many-timings",
        "beauty-timings",
        "tests",
        "clippy",
        "fmt",
    ] {
        let status = std::fs::read_to_string(
            root.join(format!(".sn/round3-resolution-final-{stage}.exit")),
        )?;
        let status = status.trim().parse::<i32>()?;
        anyhow::ensure!(
            status == 0 || matches!(stage, "content" | "robust-timings"),
            "Verification gate failed: {stage}"
        );
        gates.insert(stage.into(), json!(status));
    }
    let result = json!({"before_commit":"8a40d3d","before":prior["after"],"after":after,"retained_failed_trials":trials,
        "method":"linear HDR relative RMSE; independent unfiltered path references; frame GPU pass-sum p95, no async overlap; shared RTX 4070 SUPER, background snapshots retained",
        "limits":{"night_lower_half_rmse":0.08,"night_upscaled_extra_ms":1.,"night_upscaled_baseline_ms":3.533664,"sponza_native_ms":10.,"sponza_upscaled_ms":5.},
        "content":"CC0 Poly Haven PBR furniture and textures, pinned manifest; main beauty golden beauty_cc0; 500 points plus 500 emissive panels in many_lights",
        "reference_caveat":"Outdoor/beauty surface paths are independent; sky LUT and froxel display composition are shared, HDR RMSE is before fog. No independent volumetric reference claim. Round 3 also fixes correlated emitter-position/roulette RNG; references were regenerated. Before/after references therefore differ in estimator correctness and sample count.",
        "verification_exit_codes":gates,
        "benchmark_status":"SHARED GPU: report median, p95 and observed maxima without claiming quiet-machine budget compliance. Parent explicitly deferred the quiet-machine benchmark sweep until in-flight tracks are merged.",
        "gpu_load_samples":{"period_ms":2000,"timezone":"Asia/Qatar UTC+03:00","scope":"whole device, including Ara and other agents; samples do not isolate external GPU work","samples":load_samples(root)?}});
    ara_gpu::cooked::write_json(
        &output_path(root, "measurements/fidelity-round3.json")?,
        &result,
    )?;
    Ok(())
}
