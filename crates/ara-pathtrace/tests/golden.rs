#![allow(clippy::unwrap_used)]
use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_pathtrace::scenes;
use ara_render::capture::compare;
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use std::path::Path;

fn settings() -> Settings {
    Settings {
        render_size: [320, 180],
        display_size: [320, 180],
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Fsr1,
        probe_ray_budget: 16384,
        reflection_cutoff: 0.6,
        post: PostSettings {
            ev100: Some(3.),
            bloom: 0.,
            fog_density: 0.,
            ..PostSettings::default()
        },
    }
}
#[test]
fn goldens_reproduce_and_scene_updates_preserve_identity_and_lifetimes() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/test-cache"))?;
    let camera = scenes::camera();
    let mut request = FrameInput {
        camera: &camera,
        tick: 0,
        delta_seconds: 1. / 60.,
        reset: false,
        lighting_changed: false,
        deterministic: true,
    };
    for name in scenes::NAMES {
        let scene = scenes::scene(name, 0);
        let mut s = settings();
        if name == "furnace" {
            s.post.ev100 = Some(0.);
        }
        let mut view = FidelityView::new(&mut gpu, &scene, s, None)?;
        for tick in 0..96 {
            request.tick = tick;
            view.render(&mut gpu, &request)?;
        }
        let image = view.capture(&mut gpu)?;
        let golden = image::open(root.join(format!("tests/goldens/{name}.png")))?.to_rgba8();
        let comparison = compare(&image, &golden, 0.005)?;
        ensure!(
            comparison.passed,
            "{name}: golden 1-SSIM {}",
            comparison.score
        );
        let radiance = view.capture_radiance(&mut gpu)?;
        ensure!(
            radiance.iter().flatten().all(|v| v.is_finite()),
            "Nonfinite HDR"
        );
        if name == "furnace" {
            let mean = (75..105)
                .flat_map(|y| (145..175).map(move |x| (y * 320 + x) as usize))
                .map(|i| f64::from(radiance[i][0]))
                .sum::<f64>()
                / 900.;
            ensure!(
                (0.65..1.02).contains(&mean),
                "White furnace lost/gained energy: {mean}"
            );
        }
        if name == "glossy_interior" {
            let id = view.scene().instance_id(7);
            let mut next = scenes::scene(name, 0);
            next.lights[0].color_lumens[3] *= 2.;
            view.update_scene(&mut gpu, &next)?;
            let stats = view.scene().acceleration_update();
            ensure!(
                stats.rebuilt_blas == 0
                    && stats.reused_blas == next.meshes.len()
                    && !stats.tlas_rebuilt,
                "Light edit rebuilt AS"
            );
            for vertex in &mut next.meshes[6].vertices {
                vertex.position[0] += 0.4;
            }
            view.update_scene(&mut gpu, &next)?;
            let stats = view.scene().acceleration_update();
            ensure!(
                stats.rebuilt_blas == 1
                    && stats.refit_fallbacks == 1
                    && stats.tlas_rebuilt
                    && view.scene().instance_id(7) == id,
                "Incremental deformation policy/identity changed"
            );
            request.reset = true;
            view.render(&mut gpu, &request)?;
            view.set_instance_masks(&mut gpu, &[(7, 0)])?;
            view.render(&mut gpu, &request)?;
            let hidden = view.capture(&mut gpu)?;
            ensure!(
                compare(&image, &hidden, 0.)?.score > 0.001,
                "Mask did not hide instance"
            );
            view.resize(&mut gpu, [83, 47], [127, 71])?;
            view.render(&mut gpu, &request)?;
            ensure!(
                view.capture(&mut gpu)?.dimensions() == (127, 71),
                "Odd FSR viewport extent"
            );
            let contract = view.upscaler_input(true);
            ensure!(
                contract.reset && contract.exposure == 1. && contract.render_size == [83, 47],
                "External temporal contract mismatch"
            );
            request.reset = false;
        }
        view.destroy(&mut gpu)?;
    }
    gpu.assert_clean()?;
    Ok(())
}
