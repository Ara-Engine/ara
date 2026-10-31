//! CPU control-path measurements. No device/window, render work or simulation.
use ara_graphics_settings::{Preset, Settings};
use ara_render_fidelity::presets::{
    Capabilities, Plan, QualityChoice, QualityPreset, RenderOptions,
};
use ara_runtime_render::{Presenter, graphics};
use std::{hint::black_box, time::Instant};

fn main() -> anyhow::Result<()> {
    const COUNT: u32 = 100_000;
    let caps = Capabilities {
        ray_tracing: false,
        device_local_mib: Some(12288),
    };
    let settings = Settings {
        preset: Preset::High,
        ..Default::default()
    };
    let baseline = Instant::now();
    for _ in 0..COUNT {
        black_box(Plan::resolve(
            black_box(QualityChoice::Preset(QualityPreset::High)),
            &caps,
            [1280, 720],
            RenderOptions::default(),
        )?);
    }
    let baseline = baseline.elapsed().as_nanos() as f64 / f64::from(COUNT);
    let changed = Instant::now();
    for _ in 0..COUNT {
        black_box(graphics::plan(
            black_box(&settings),
            &caps,
            [1280, 720],
            RenderOptions::default(),
        )?);
    }
    let changed = changed.elapsed().as_nanos() as f64 / f64::from(COUNT);
    let mut presenter =
        Presenter::new([1280, 720], Vec::new(), "target/graphics-control-benchmark");
    presenter.set_graphics_settings(&settings);
    let unchanged = Instant::now();
    for _ in 0..COUNT {
        black_box(&mut presenter).set_graphics_settings(black_box(&settings));
    }
    let unchanged = unchanged.elapsed().as_nanos() as f64 / f64::from(COUNT);
    println!(
        "{}",
        serde_json::json!({"iterations":COUNT,"baseline_plan_ns":baseline,
        "changed_settings_plan_ns":changed,"unchanged_preferences_ns":unchanged})
    );
    Ok(())
}
