//! Assemble reproducible measurements without copying HDR caches or asset inputs.
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

fn read(root: &Path, name: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join(name))?)?)
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
        ("many_quality", "many_lights-quality.json"),
        ("many_timings", "many_lights-timings.json"),
        ("many_noise", "many-lights-noise.json"),
        ("beauty_quality", "beauty_cc0-quality.json"),
        ("beauty_timings", "beauty_cc0-timings.json"),
        ("diagnostic_quality", "quality-targets.json"),
        ("validation", "validation.json"),
    ] {
        after.insert(key.into(), read(root, &format!("measurements/{file}"))?);
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
    let result = json!({"before_commit":"8a40d3d","before":prior["after"],"after":after,"retained_failed_trials":trials,
        "method":"linear HDR relative RMSE; independent unfiltered path references; frame GPU pass-sum p95, no async overlap; shared RTX 4070 SUPER, background snapshots retained",
        "limits":{"night_lower_half_rmse":0.08,"night_upscaled_extra_ms":1.,"night_upscaled_baseline_ms":3.533664,"sponza_native_ms":10.,"sponza_upscaled_ms":5.},
        "content":"CC0 Poly Haven PBR furniture and textures, pinned manifest; main beauty golden beauty_cc0; 500 points plus 500 emissive panels in many_lights",
        "reference_caveat":"Outdoor/beauty surface paths are independent; sky LUT and froxel display composition are shared, HDR RMSE is before fog. No independent volumetric reference claim."});
    ara_gpu::cooked::write_json(&root.join("measurements/fidelity-round3.json"), &result)?;
    Ok(())
}
