use anyhow::{Result, ensure};
use ara_gpu::GpuDevice;
use ara_pathtrace::scenes;
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};

#[test]
fn feature_hdr_binds_filter_slot_and_reuses_bloom_post() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new(&[], root.join(".sn/feature-contract-cache"))?;
    let scene = scenes::scene("glossy_interior", 0);
    let settings = Settings {
        render_size: [65, 37],
        display_size: [65, 37],
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 0.6,
        post: PostSettings {
            ev100: Some(3.),
            bloom: 0.12,
            ..Default::default()
        },
    };
    let camera = scenes::camera();
    let frame = FrameInput {
        camera: &camera,
        tick: 0,
        delta_seconds: 1. / 60.,
        reset: true,
        lighting_changed: false,
        deterministic: true,
    };
    for reservoirs in [false, true] {
        let mut view = FidelityView::new(&mut gpu, &scene, settings, None)?;
        view.set_reservoir_sampling(&mut gpu, reservoirs)?;
        view.render(&mut gpu, &frame)?;
        let before = view.capture(&mut gpu)?;
        let inputs = view.feature_inputs()?;
        view.post_process(&mut gpu, &frame, inputs.radiance)?;
        let after = view.capture(&mut gpu)?;
        ensure!(
            before
                .as_raw()
                .iter()
                .zip(after.as_raw())
                .all(|(a, b)| a.abs_diff(*b) <= 1),
            "Feature composition sampled a different HDR slot or changed bloom semantics"
        );
        ensure!(
            view.timings()
                .iter()
                .any(|p| p.pass == "Feature.Bloom.Down.1"),
            "Feature post did not execute the shared bloom pipeline"
        );
        view.destroy(&mut gpu)?;
    }
    gpu.assert_clean()?;
    Ok(())
}
