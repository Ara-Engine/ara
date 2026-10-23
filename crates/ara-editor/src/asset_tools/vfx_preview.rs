//! Offline preview cook/capture worker. Last-good UI images stay live while it runs.
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
pub(super) fn capture(root: &Path, effect: &ara_vfx::Effect) -> anyhow::Result<image::RgbaImage> {
    let cooked = cooked(root, effect)?;
    let mut gpu = ara_gpu::vulkan::VulkanDevice::new(&[], root.join(".ara/cache/vfx-preview-gpu"))?;
    let mut system = ara_vfx_gpu::GpuSystem::new(&mut gpu, effect, &cooked, [384, 384])?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(4., 2.8, 5.),
            target: Vec3::new(0., 1.1, 0.),
            vertical_fov: 0.75,
        },
        size: [384, 384],
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
    gpu.context().assert_clean()?;
    Ok(result?.image)
}
