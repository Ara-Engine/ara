//! GPU probes behind the hidden `--probe NAME` route: each opens a hidden SDL window
//! on the main thread (which Cocoa requires) and drives the window present path.
//! `tests/probes.rs` spawns the player for each, on macOS and Windows alike.
use super::*;

/// Runs the probe `name`; an unknown name lists the known ones.
pub(crate) fn run(name: &str) -> Result<()> {
    match name {
        "ui-error-teardown" => {
            native_game_ui_retires_before_the_device_when_a_frame_returns_an_error()
        }
        "frames-in-flight" => frames_in_flight_with_game_ui_and_swapchain_stay_validation_clean(),
        "unpresentable-pump" => an_unpresentable_window_never_blocks_the_event_pump(),
        "unpresentable-pump-2d" => an_unpresentable_2d_window_never_blocks_the_event_pump(),
        _ => anyhow::bail!(
            "unknown probe {name:?}; known: ui-error-teardown, frames-in-flight, unpresentable-pump, unpresentable-pump-2d"
        ),
    }
}

fn native_game_ui_retires_before_the_device_when_a_frame_returns_an_error() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/standalone-ui-error");
    std::fs::create_dir_all(&root)?;
    let result = (|| -> Result<()> {
        let (window, presenter) = crate::device::Windowed::open_offscreen(
            "UI error teardown",
            [160, 90],
            &root,
            ara_runtime_render::QualityChoice::Preset(ara_runtime_render::QualityPreset::Low),
            ara_runtime_render::RenderOptions { ray_tracing: false },
        )?;
        let mut owner = Presentation {
            window: Some(window),
            presenter2d: Presenter2d::new(&root),
            presenter,
        };
        let source = serde_json::from_value(json!({
            "markup":"<ui id=\"game\"><text id=\"caption\" text=\"Live scene\"/></ui>","stylesheet":"","bindings":[]
        }))?;
        let window = owner.window.as_mut().context("window")?;
        window.game_ui = Some(crate::game_ui::Surface::plain(&source, [160, 90], 1.0)?);
        window.present(&mut owner.presenter, &crate::device::warm_frame(), || {
            Ok(ara_runtime_render::fidelity::native::Content::default())
        })?;
        // Exercise the original early-return path, after the UI has borrowed the device.
        anyhow::bail!("injected frame error")
    })();
    assert_eq!(
        result.expect_err("injected error").to_string(),
        "injected frame error"
    );
    Ok(())
}

fn frames_in_flight_with_game_ui_and_swapchain_stay_validation_clean() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/standalone-in-flight");
    std::fs::create_dir_all(&root)?;
    let (window, presenter) = crate::device::Windowed::open_offscreen(
        "Frames in flight",
        [160, 90],
        &root,
        ara_runtime_render::QualityChoice::Preset(ara_runtime_render::QualityPreset::Low),
        ara_runtime_render::RenderOptions { ray_tracing: false },
    )?;
    let mut owner = Presentation {
        window: Some(window),
        presenter2d: Presenter2d::new(&root),
        presenter,
    };
    let source = serde_json::from_value(json!({
        "markup":"<ui id=\"game\"><text id=\"caption\" text=\"Live scene\"/></ui>","stylesheet":"","bindings":[]
    }))?;
    let window = owner.window.as_mut().context("window")?;
    // Scene, UI composite and swapchain blit all overlap the next frame here.
    for frame in 0..6 {
        if frame == 3 {
            window.game_ui = Some(crate::game_ui::Surface::plain(&source, [160, 90], 1.0)?);
        }
        window.present(&mut owner.presenter, &crate::device::warm_frame(), || {
            Ok(ara_runtime_render::fidelity::native::Content::default())
        })?;
    }
    assert!(
        owner.presenter.stats().gpu.is_some(),
        "the previous frame reported"
    );
    owner.presenter.set_frames_in_flight(false)?;
    let renderer = owner.presenter.renderer().context("renderer")?;
    renderer.gpu().wait_idle()?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

fn an_unpresentable_window_never_blocks_the_event_pump() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/standalone-pump");
    std::fs::create_dir_all(&root)?;
    let (window, presenter) = crate::device::Windowed::open_offscreen(
        "Pump",
        [160, 90],
        &root,
        ara_runtime_render::QualityChoice::Preset(ara_runtime_render::QualityPreset::Low),
        ara_runtime_render::RenderOptions { ray_tracing: false },
    )?;
    let mut owner = Presentation {
        window: Some(window),
        presenter2d: Presenter2d::new(&root),
        presenter,
    };
    let window = owner.window.as_mut().context("window")?;
    let content = || Ok(ara_runtime_render::fidelity::native::Content::default());
    for _ in 0..3 {
        window.present(&mut owner.presenter, &crate::device::warm_frame(), content)?;
    }
    // A minimized window (stood in for by the override): every present returns
    // promptly and submits nothing, with frames still in flight from before.
    window.override_presentable(Some(false));
    window.take_stats();
    for _ in 0..5 {
        let started = Instant::now();
        window.present(&mut owner.presenter, &crate::device::warm_frame(), content)?;
        let took = started.elapsed();
        ensure!(
            took < std::time::Duration::from_millis(100),
            "present blocked for {took:?}"
        );
    }
    let (fps, _) = window.take_stats();
    ensure!(
        fps == 0.,
        "an unpresentable window presented {fps} frames per second"
    );
    // Shown again, it renders and reports its frames.
    window.override_presentable(None);
    for _ in 0..3 {
        window.present(&mut owner.presenter, &crate::device::warm_frame(), content)?;
    }
    ensure!(owner.presenter.stats().gpu.is_some(), "presenting resumed");
    owner.presenter.set_frames_in_flight(false)?;
    let renderer = owner.presenter.renderer().context("renderer")?;
    renderer.gpu().wait_idle()?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

/// The 2D window path (`prepare_2d`/`present_2d`): its renderer waits for its own
/// submissions, so an unpresentable window must keep it from submitting at all.
fn an_unpresentable_2d_window_never_blocks_the_event_pump() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/standalone-pump-2d");
    std::fs::create_dir_all(&root)?;
    let (window, presenter) = crate::device::Windowed::open_offscreen(
        "Pump 2D",
        [160, 90],
        &root,
        ara_runtime_render::QualityChoice::Preset(ara_runtime_render::QualityPreset::Low),
        ara_runtime_render::RenderOptions { ray_tracing: false },
    )?;
    let mut owner = Presentation {
        window: Some(window),
        presenter2d: Presenter2d::new(&root),
        presenter,
    };
    let mut world = ara_sim::bevy_ecs::world::World::new();
    world.spawn(ara_2d::Camera2d::default());
    let window = owner.window.as_mut().context("window")?;
    let mut frame =
        |window: &mut crate::device::Windowed, tick: u64| -> Result<std::time::Duration> {
            let started = Instant::now();
            window.prepare_2d(&mut owner.presenter2d, &mut world, tick)?;
            window.present_2d(&mut owner.presenter2d)?;
            Ok(started.elapsed())
        };
    for tick in 0..3 {
        frame(window, tick)?;
    }
    // A minimized window (the override): every frame returns promptly, renders and
    // presents nothing.
    window.override_presentable(Some(false));
    window.take_stats();
    for tick in 3..8 {
        let took = frame(window, tick)?;
        ensure!(
            took < std::time::Duration::from_millis(100),
            "2D frame blocked for {took:?}"
        );
    }
    let (fps, _) = window.take_stats();
    ensure!(
        fps == 0.,
        "an unpresentable 2D window presented {fps} frames per second"
    );
    // Shown again, it presents.
    window.override_presentable(None);
    window.take_stats();
    for tick in 8..11 {
        frame(window, tick)?;
    }
    let (fps, _) = window.take_stats();
    ensure!(fps > 0., "presenting resumed");
    window.shutdown_2d(&owner.presenter2d)?;
    Ok(())
}
