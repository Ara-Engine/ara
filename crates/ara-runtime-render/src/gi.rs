//! Probe GI acquisition for the raster path: the frame's native content is
//! ray-traced once by a temporary fidelity view (`bake_probe_surfaces`), and the
//! raster renderer relights the result every frame without rays.
use crate::fidelity::native::{self, Content, NativeConfig};
use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::look::gi::ProbeSurfaces;
use ara_render_fidelity::{
    FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler,
    environment::FrameEnvironment,
};
use ara_runtime::extract::RenderFrame;

/// Residency used only while baking (full-resolution albedo for the caches).
fn config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1024 * 1024 * 1024,
        upload_bytes: 64 * 1024 * 1024,
    }
}

/// Bakes the probe surfaces of `frame`'s content on a ray-tracing device.
pub(crate) fn bake(
    gpu: &mut VulkanDevice,
    frame: &RenderFrame,
    content: Content,
) -> Result<ProbeSurfaces> {
    let started = std::time::Instant::now();
    let prepared = native::prepare(frame, content, config())?;
    let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
    let mut view = FidelityView::new_native(
        gpu,
        &prepared.scene,
        prepared.textures.fork_sources(config())?,
        Settings {
            render_size: [64, 36],
            display_size: [64, 36],
            tier: Tier::Rt,
            mode: Mode::Hybrid,
            upscaler: Upscaler::Taau,
            probe_ray_budget: 64,
            reflection_cutoff: 1.,
            post: look.post(PostSettings::default()),
        },
        None,
    )?;
    let surfaces = (|| {
        view.set_environment(&look)?;
        for _ in 0..256 {
            if view.refine_native_textures(gpu)?.pending_textures == 0 {
                break;
            }
        }
        let camera = crate::camera(frame)?;
        view.render(
            gpu,
            &FrameInput {
                camera: &camera,
                tick: 0,
                delta_seconds: 1. / 60.,
                reset: true,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
        view.bake_probe_surfaces(gpu)
    })();
    view.destroy(gpu)?;
    let surfaces = surfaces?;
    ensure!(!surfaces.is_empty(), "empty probe bake");
    eprintln!(
        "ara-runtime-render: baked {} GI probes ({:?}) in {:.1} s",
        surfaces.len(),
        surfaces.counts,
        started.elapsed().as_secs_f64()
    );
    Ok(surfaces)
}
