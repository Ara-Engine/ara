use super::*;
use clap::Parser;

#[test]
#[ignore = "hidden SDL GPU ownership probe; run explicitly on the GPU host"]
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
        window.game_ui = Some(crate::game_ui::Surface::plain(&source, [160, 90])?);
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

#[test]
#[ignore = "hidden SDL GPU frames-in-flight probe; run explicitly (one test thread) on the GPU host"]
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
            window.game_ui = Some(crate::game_ui::Surface::plain(&source, [160, 90])?);
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

#[test]
fn exported_games_default_to_borderless_with_an_explicit_windowed_override() -> Result<()> {
    let args = Args::try_parse_from(["ara-player"])?;
    assert!(!args.standalone.windowed);
    let args = Args::try_parse_from(["ara-player", "--windowed"])?;
    assert!(args.standalone.windowed);
    assert!(!args.standalone.no_present);
    Ok(())
}

#[test]
fn no_present_is_an_explicit_hidden_perf_diagnostic() -> Result<()> {
    assert!(Args::try_parse_from(["ara-player", "--no-present"]).is_err());
    assert!(
        Args::try_parse_from([
            "ara-player",
            "--no-present",
            "--perf-output",
            "x",
            "--headless"
        ])
        .is_err()
    );
    let args = Args::try_parse_from(["ara-player", "--no-present", "--perf-output", "x"])?;
    assert!(args.standalone.no_present);
    let mut perf = perf::Window::new(args.standalone.perf, false, Instant::now());
    perf.configure_presentation(args.standalone.no_present, false);
    assert!(perf.offscreen());
    Ok(())
}
