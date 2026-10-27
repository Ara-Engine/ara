use anyhow::{Result, ensure};
use ara_gpu::GpuDevice;
use ara_pathtrace::scenes;
use ara_render_fidelity::{
    AutoExposure, FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, ToneMapper,
    Upscaler,
};

#[test]
fn histogram_ev_limits_manual_compensation_and_simulation_time_adaptation() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new(&[], root.join(".sn/exposure-test-cache"))?;
    let camera = scenes::camera();
    let settings = Settings {
        render_size: [160, 90],
        display_size: [160, 90],
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Taau,
        probe_ray_budget: 1024,
        reflection_cutoff: 0.6,
        post: PostSettings {
            ev100: None,
            tone_mapper: ToneMapper::PbrNeutral,
            adaptation: 2.,
            auto_exposure: AutoExposure {
                minimum_ev100: 3.,
                maximum_ev100: 6.,
                ..AutoExposure::default()
            },
            bloom: 0.,
            fog_density: 0.,
            ..PostSettings::default()
        },
    };
    let mut view = FidelityView::new(&mut gpu, &scenes::scene("furnace", 0), settings, None)?;
    ensure!(view.capture_direct_radiance(&mut gpu).is_err());
    ensure!(view.capture_incident_radiance(&mut gpu).is_err());
    let mut input = FrameInput {
        camera: &camera,
        tick: 0,
        delta_seconds: 0.1,
        reset: false,
        lighting_changed: false,
        deterministic: true,
    };
    view.render(&mut gpu, &input)?;
    let initial = view.capture_exposure(&mut gpu)?;
    ensure!(
        (initial.target_ev100 - 3.).abs() < 1e-5 && (initial.multiplier - 1. / 9.6).abs() < 1e-5,
        "Histogram/EV clamp: {initial:?}"
    );
    let manual = PostSettings {
        ev100: Some(5.),
        compensation: 1.,
        ..settings.post
    };
    view.set_post(manual)?;
    input.tick = 1;
    view.render(&mut gpu, &input)?;
    let fixed = view.capture_exposure(&mut gpu)?;
    ensure!(
        (fixed.multiplier - 1. / 19.2).abs() < 1e-5,
        "Manual exposure compensation: {fixed:?}"
    );
    let auto = PostSettings {
        auto_exposure: AutoExposure {
            minimum_ev100: 10.,
            maximum_ev100: 11.,
            ..AutoExposure::default()
        },
        ..settings.post
    };
    view.set_post(auto)?;
    input.tick = 2;
    view.render(&mut gpu, &input)?;
    let adapted = view.capture_exposure(&mut gpu)?;
    let expected =
        fixed.multiplier + (1. / (1.2 * 1024.) - fixed.multiplier) * (1. - (-0.2_f32).exp());
    ensure!(
        (adapted.target_ev100 - 10.).abs() < 1e-5 && (adapted.multiplier - expected).abs() < 1e-5,
        "Adaptation must use simulation delta: {adapted:?}"
    );
    view.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
