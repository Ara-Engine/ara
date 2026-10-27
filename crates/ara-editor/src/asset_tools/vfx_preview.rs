//! Offline preview cook/capture worker. Last-good UI images stay live while it runs.
use super::studio::MASTER;
use ara_gpu::cooked::{RenderState, Stage};
use ara_render::{
    capture::{CaptureRequest, ViewMode},
    scene::CameraSpec,
};
use glam::Vec3;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
pub(super) fn cooked(root: &Path, effect: &ara_vfx::Effect) -> anyhow::Result<PathBuf> {
    let hash = effect
        .content_hash()
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let bundled = ara_gpu::cooked::resource_dir(
        "ara-vfx-gpu",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ara-vfx-gpu/cooked"),
    )
    .join(&effect.name);
    let matches = |path: &Path| {
        std::fs::read_to_string(path.join("effect.json"))
            .ok()
            .and_then(|s| ara_json::parse(&s).ok())
            .is_some_and(|v| v["hash"] == hash)
    };
    let cooked = if matches(&bundled) {
        bundled
    } else {
        let base = root.join(".ara/cache/vfx-previews");
        let cooked = base.join(&hash);
        if !matches(&cooked) {
            let source = base.join("sources").join(&hash);
            std::fs::create_dir_all(&source)?;
            std::fs::write(
                source.join("effect.slang"),
                ara_vfx_gpu::cook_source(effect, None)?,
            )?;
            let mut manifest = BTreeMap::new();
            for entry in ara_vfx_gpu::COMPUTE_ENTRIES {
                manifest.insert((*entry).to_owned(),serde_json::json!({"source":"effect.slang","entries":BTreeMap::from([(*entry,Stage::Compute)]),"state":RenderState::default()}));
            }
            for (name, fragment, formats, blend) in [
                ("backdrop", "backdropFragment", vec![109, 100], false),
                ("particles", "particleFragment", vec![109, 100], true),
                ("tonemap", "tonemapFragment", vec![37], false),
            ] {
                let vertex = if name == "particles" {
                    "particleVertex"
                } else {
                    "screenVertex"
                };
                manifest.insert(name.into(),serde_json::json!({"source":"effect.slang","entries":BTreeMap::from([(vertex,Stage::Vertex),(fragment,Stage::Fragment)]),"state":RenderState{color_formats:formats,blend,..Default::default()}}));
            }
            let file = source.join("pipelines.json");
            std::fs::write(
                &file,
                ara_json::to_canonical_string(&ara_json::to_value(&manifest)?),
            )?;

            ara_shader_cook::cook(&file, &cooked, &ara_shader_cook::Toolchain::discover())?;
            std::fs::write(
                cooked.join("effect.json"),
                ara_json::to_canonical_string(&serde_json::json!({"hash":hash})),
            )?;
            if !base.join("atlas.rgba").exists() {
                std::fs::write(base.join("atlas.rgba"), ara_vfx_gpu::cook_atlas())?;
            }
        }
        cooked
    };
    Ok(cooked)
}
/// A camera fitted to where the effect's particles actually go. Emitter culling
/// bounds are deliberately loose, so the CPU reference simulation runs for three
/// seconds and the 2–98 % extents of live particles (grown by their size) are framed
/// from a raised front-right 3/4 view, matching model thumbnails.
pub(crate) fn framing(effect: &ara_vfx::Effect) -> CameraSpec {
    let fallback = ara_vfx_gpu::View::default().camera;
    let Ok(mut system) = ara_vfx::CpuSystem::new(effect) else {
        return fallback;
    };
    let hz = effect.hz.max(1);
    let mut axes = [Vec::new(), Vec::new(), Vec::new()];
    let mut sizes = Vec::new();
    for tick in 0..hz * 3 {
        if system.step(None).is_err() {
            return fallback;
        }
        if tick < hz / 2 || tick % 6 != 0 {
            continue;
        }
        for soa in system.emitters() {
            for particle in soa.alive.iter().filter_map(|slot| soa.particle(*slot)) {
                for (axis, value) in axes.iter_mut().zip(particle.position.to_array()) {
                    axis.push(value);
                }
                sizes.push(particle.size);
            }
        }
    }
    if sizes.is_empty() {
        return fallback;
    }
    let percentile = |values: &mut Vec<f32>, p: f32| {
        values.sort_by(f32::total_cmp);
        values[((values.len() - 1) as f32 * p).round() as usize]
    };
    let grow = percentile(&mut sizes, 0.9) * 0.5;
    let mut min = Vec3::ZERO;
    let mut max = Vec3::ZERO;
    for (axis, values) in axes.iter_mut().enumerate() {
        min[axis] = percentile(values, 0.02) - grow;
        max[axis] = percentile(values, 0.98) + grow;
    }
    // Keep the floor contact in view for effects that start at the ground.
    min.y = min.y.min(0.);
    CameraSpec::FrameBounds {
        min,
        max: max.max(min + Vec3::splat(0.2)),
        direction: Vec3::new(0.53, 0.41, 0.74),
        padding: 0.06,
        vertical_fov: 0.6,
    }
}

pub(super) fn capture(root: &Path, effect: &ara_vfx::Effect) -> anyhow::Result<image::RgbaImage> {
    let cooked = cooked(root, effect)?;
    let mut gpu = ara_gpu::GpuDevice::new(&[], root.join(".ara/cache/vfx-preview-gpu"))?;
    let mut system = ara_vfx_gpu::GpuSystem::new(&mut gpu, effect, &cooked, [MASTER, MASTER])?;
    let request = CaptureRequest {
        camera: framing(effect),
        size: [MASTER, MASTER],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 120,
        seed: u64::from(effect.seed),
        marks: None,
    };
    let result = system.capture(&mut gpu, &request);
    system.retire(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(result?.image)
}
