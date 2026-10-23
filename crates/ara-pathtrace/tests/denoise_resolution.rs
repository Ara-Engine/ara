use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_pathtrace::{Reference, metrics, robust_scenes};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};

#[test]
fn night_denoising_preserves_angular_footprint_at_higher_resolution() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/resolution-test-cache"))?;
    let scene = robust_scenes::night();
    let camera = robust_scenes::camera(false);
    let post = PostSettings {
        ev100: Some(5.),
        ..Default::default()
    };
    let mut reference = Reference::new(&mut gpu, &scene, [480, 270], 8192, 12, post)?;
    reference.converge(&mut gpu, &camera)?;
    let ground = reference.radiance(&mut gpu)?;
    reference.destroy(&mut gpu)?;
    let mut errors = Vec::new();
    for factor in [1, 2] {
        let size = [480 * factor, 270 * factor];
        let mut view = FidelityView::new(
            &mut gpu,
            &scene,
            Settings {
                render_size: size,
                display_size: size,
                tier: Tier::Rt,
                mode: Mode::Hybrid,
                upscaler: Upscaler::Taau,
                probe_ray_budget: 16384,
                reflection_cutoff: 0.6,
                post,
            },
            None,
        )?;
        robust_scenes::configure_view(&mut gpu, &mut view, false)?;
        for tick in 0..256 {
            view.render(
                &mut gpu,
                &FrameInput {
                    camera: &camera,
                    tick,
                    delta_seconds: 1. / 60.,
                    reset: false,
                    lighting_changed: false,
                    deterministic: false,
                },
            )?;
        }
        let image = view.capture_radiance(&mut gpu)?;
        view.destroy(&mut gpu)?;
        // Compare equal pixel areas; a higher-resolution capture must not hide
        // its remaining low-frequency noise behind a different display scale.
        let mut resolved = vec![[0.; 4]; 480 * 270];
        for y in 0..270 {
            for x in 0..480 {
                for dy in 0..factor {
                    for dx in 0..factor {
                        let sample =
                            image[((y * factor + dy) * size[0] + x * factor + dx) as usize];
                        for (out, value) in resolved[(y * 480 + x) as usize].iter_mut().zip(sample)
                        {
                            *out += value / (factor * factor) as f32;
                        }
                    }
                }
            }
        }
        let lower = 480 * 135;
        errors.push(metrics::radiance(&resolved[lower..], &ground[lower..])?.relative_rmse);
    }
    gpu.assert_clean()?;
    println!(
        "Equal-area night ROI error: 270p {}, 540p {}",
        errors[0], errors[1]
    );
    ara_gpu::cooked::write_json(
        &ara_pathtrace::artifacts::temporary_path(root, "measurements/denoiser-resolution.json")?,
        &serde_json::json!({"reference_samples":8192,"bounces":12,"sizes":[[480,270],[960,540]],"settle_frames":256,"roi":"lower half, resolved to matching 480x270 pixel areas","relative_rmse":errors,"maximum_error_ratio":1.05,"passed":errors[1]<=errors[0]*1.05}),
    )?;
    ensure!(
        errors[1] <= errors[0] * 1.05,
        "Higher resolution exposed low-frequency denoiser noise: {errors:?}"
    );
    Ok(())
}
