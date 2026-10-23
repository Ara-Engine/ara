use anyhow::Result;
use ara_gpu::vulkan::VulkanDevice;
use ara_pathtrace::{Reference, scenes};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use std::path::Path;
mod content;
mod corridor;
mod lag;
mod large;
mod lighting_sequence;
mod material_import;
mod quality;
mod robust;
mod verification;

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(root.join("captures"))?;
    std::fs::create_dir_all(root.join("measurements"))?;
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/pipeline-cache"))?;
    let command = std::env::args().nth(1).unwrap_or_else(|| "smoke".into());
    match command.as_str() {
        "sequences" => verification::sequences(&mut gpu, root)?,
        "quality" => quality::run(&mut gpu, root)?,
        "content" => content::run(&mut gpu, root)?,
        "sponza-reference" => large::reference(&mut gpu, root)?,
        "sponza-quality" => large::quality(&mut gpu, root)?,
        "corridor" => corridor::run(&mut gpu, root)?,
        "lighting-sequence" => lighting_sequence::run(&mut gpu, root)?,
        "robust-quality" => robust::quality(&mut gpu, root)?,
        "robust-timings" => robust::timings(&mut gpu, root)?,
        "capture" => verification::captures(&mut gpu, root)?,
        "timings" => verification::timings(&mut gpu, root)?,
        "smoke" => {
            let scene = scenes::scene("glossy_interior", 0);
            let camera = scenes::camera();
            let mut view =
                FidelityView::new(&mut gpu, &scene, settings([160, 90], [160, 90]), None)?;
            for tick in 0..16 {
                view.render(&mut gpu, &input(&camera, tick, false))?;
            }
            view.capture(&mut gpu)?
                .save(root.join("captures/smoke-hybrid.png"))?;
            view.set_post(PostSettings {
                ev100: None,
                bloom: 0.08,
                fog_density: 0.02,
                quality: ara_render_fidelity::Quality::Cinematic,
                aperture: 4.,
                ..post()
            })?;
            let moving_camera = ara_render::scene::CameraSpec::LookAt {
                position: glam::Vec3::new(0.4, 1.6, 6.5),
                target: glam::Vec3::new(0., 1.2, -0.4),
                vertical_fov: 0.9,
            };
            view.render(&mut gpu, &input(&moving_camera, 16, false))?;
            view.capture(&mut gpu)?
                .save(root.join("captures/smoke-cinematic.png"))?;
            let bake = view.bake_snapshot(&mut gpu)?;
            view.destroy(&mut gpu)?;
            let mut fallback = settings([160, 90], [160, 90]);
            fallback.tier = Tier::Raster;
            let mut view = FidelityView::new(&mut gpu, &scene, fallback, Some(&bake))?;
            view.render(&mut gpu, &input(&camera, 0, true))?;
            view.capture(&mut gpu)?
                .save(root.join("captures/smoke-raster.png"))?;
            view.destroy(&mut gpu)?;
            let mut reference = Reference::new(&mut gpu, &scene, [160, 90], 16, 5, post())?;
            reference.converge(&mut gpu, &camera)?;
            reference
                .capture(&mut gpu)?
                .save(root.join("captures/smoke-reference.png"))?;
            reference.destroy(&mut gpu)?;
            println!("RT hybrid, baked Raster, reference: validation and sync validation clean");
        }
        _ => anyhow::bail!("Usage: ara-pathtrace [smoke|capture|timings|quality|content]"),
    }
    gpu.assert_clean()?;
    Ok(())
}
fn post() -> PostSettings {
    PostSettings {
        ev100: Some(3.),
        bloom: 0.,
        fog_density: 0.,
        ..PostSettings::default()
    }
}
fn settings(render_size: [u32; 2], display_size: [u32; 2]) -> Settings {
    Settings {
        render_size,
        display_size,
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Fsr1,
        probe_ray_budget: 16384,
        reflection_cutoff: 0.6,
        post: post(),
    }
}
fn input(camera: &ara_render::scene::CameraSpec, tick: u32, reset: bool) -> FrameInput<'_> {
    FrameInput {
        camera,
        tick,
        delta_seconds: 1. / 60.,
        reset,
        lighting_changed: false,
        deterministic: true,
    }
}
