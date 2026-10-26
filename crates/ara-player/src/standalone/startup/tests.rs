use super::*;

#[test]
#[ignore = "hidden SDL Vulkan diagnostic; run explicitly on the GPU host"]
fn no_present_renders_loading_and_game_without_a_swapchain() -> Result<()> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/renderer-unification/no-present");
    let (mut window, extensions, size) = device::Windowed::open_unprepared_with_mode(
        "No present test",
        [160, 90],
        true,
        ara_platform::GameWindowMode::Borderless,
    )?;
    window.disable_presentation();
    let (mut loading, gpu, _) = device::Loading::for_game(&extensions, &root, false)?;
    assert!(window.present_loading(&mut loading, 0.5)?);
    assert!(loading.swapchain().is_none());
    let mut presenter = Presenter::with_device(size, root.join("game"), gpu);
    presenter.set_probe_gi(false);
    presenter.prepare(&device::warm_frame())?;
    window.finish_loading(loading);
    window.present(&mut presenter, &device::warm_frame(), || {
        Ok(ara_runtime_render::fidelity::native::Content::default())
    })?;
    assert!(presenter.output().is_some());
    assert!(window.swapchain().is_none());
    window.shutdown(&presenter)?;
    presenter
        .renderer()
        .context("renderer")?
        .gpu()
        .assert_clean()?;
    Ok(())
}

#[test]
#[ignore = "hidden SDL presentation probe; run explicitly on the GPU host"]
fn loading_can_present_while_presenter_warmup_is_blocked_and_then_hands_off_its_gpu() -> Result<()>
{
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/loading-worker");
    let (mut window, extensions, size) =
        device::Windowed::open_unprepared("Loading test", [320, 180], true)?;
    let (mut loading, gpu, parallel) = device::Loading::for_game(&extensions, &root, false)?;
    let (started, wait_started) = std::sync::mpsc::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let cache = root.join("game");
    let worker = graphics::Graphics::spawn(move || {
        started.send(thread::current().id())?;
        wait_release.recv()?;
        let mut presenter = Presenter::with_device(size, cache, gpu);
        presenter.set_probe_gi(false);
        presenter.prepare(&device::warm_frame())?;
        Ok(presenter)
    })?;
    assert_ne!(wait_started.recv()?, thread::current().id());
    assert!(window.present_loading(&mut loading, 0.25)?);
    let swapchain = loading.swapchain().context("loading swapchain")?;
    let device = loading.native_device();
    assert!(
        !worker.is_finished(),
        "the first presented frame does not await warmup"
    );
    release.send(())?;
    if parallel {
        while !worker.is_finished() {
            window.present_loading(&mut loading, 0.5)?;
        }
    }
    let (mut presenter, _) = worker.finish()?;
    assert_eq!(
        presenter
            .renderer()
            .context("renderer")?
            .gpu()
            .context()
            .device
            .handle(),
        device
    );
    window.finish_loading(loading);
    window.present(&mut presenter, &device::warm_frame(), || {
        Ok(ara_runtime_render::fidelity::native::Content::default())
    })?;
    assert_eq!(window.swapchain(), Some(swapchain));
    window.shutdown(&presenter)?;
    presenter
        .renderer()
        .context("warmed renderer")?
        .gpu()
        .assert_clean()?;
    assert!(presenter.output().is_some());
    Ok(())
}

#[test]
fn prepared_runtime_moves_to_owner_and_continues_its_same_ecs_physics_world() -> Result<()> {
    let scene: ara_play::PlayScene = serde_json::from_value(json!({
        "format":1,"scene":"sc_startup1","types":[],"seed":7,"tick_hz":60,
        "assets":{},"cache":"","nav_cache":null,
        "documents":[{"path":"main.scene","kind":"scene","id":"sc_startup1","records":{
            "e_body":{"Transform":{"position":[0,2,0]},
                "RigidBody":{"motion":"dynamic","mass":1},
                "Collider":{"kind":"sphere","radius":0.25}}
        }}]
    }))?;
    let worker = thread::spawn(move || {
        World::prepare_packed(
            &scene,
            Arc::new(ara_runtime::assets::MemoryAssets::default()),
        )
    });
    let (prepared, build_ms) = worker
        .join()
        .map_err(|_| anyhow::anyhow!("world worker panicked"))??;
    let mut world = prepared.map_runtime(|runtime| Ok(crate::net::Play::Local(runtime)))?;
    world.start(true, build_ms)?;
    world.runtime.step(6, &[])?;
    assert_eq!(world.runtime.tick(), 6);
    let height = world::read_path(
        world.runtime.play().session(),
        "e_body.Transform.position.y",
    )?;
    assert!(
        height
            .context("body position")?
            .as_f64()
            .context("body height")?
            < 2.
    );
    Ok(())
}
