//! Installed live startup with a GPU target instead of a desktop window.
use super::*;
use crate::{
    connection::Event,
    model::{Link, View},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Instant;

pub(super) fn run(options: &Options, output: &Path, directory: &Path, config: &Path) -> Result<()> {
    let started = Instant::now();
    ensure!(
        directory.join("hub.json").is_file(),
        "startup probe requires an installed standalone Hub"
    );
    // A diagnostic may start a service, so require the same explicit isolated profile as tests.
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .canonicalize()?;
    ensure!(
        config.canonicalize()?.starts_with(&target),
        "startup probe config must be under target"
    );
    ensure!(
        output.canonicalize()?.starts_with(&target),
        "startup probe output must be under target"
    );
    let mut hub = Hub::new(
        load_prefs(config),
        config.to_owned(),
        engines::installed(directory),
    );
    let mut connection = updates::connect(
        directory.to_owned(),
        config.to_owned(),
        options.ara.clone(),
        || {},
    );
    let phase = Instant::now();
    let mut surface = Offscreen::new(options.size, options.dpi.unwrap_or(1.))?;
    ara_hub_service::startup::record("ui.gpu_images", phase.elapsed());
    let phase = Instant::now();
    let mut app = App::new(surface.logical(), os::docs(directory)).map_err(err)?;
    app.rebuild(&mut hub, now_ms()).map_err(err)?;
    app.frame().map_err(err)?;
    surface.render(&app)?;
    let first = now_ms();
    let first_images = surface.builtins.loaded();
    ara_hub_service::startup::record("ui.first_draw", phase.elapsed());
    let first_pixels = surface.ui_gpu.pixels()?;
    // Use the same retained input path while service connection is pending.
    let phase = Instant::now();
    for id in ["theme", "nav-settings"] {
        let rect = app.ui.nodes[app.ui.find(id).context("startup control")?].rect;
        let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
        app.dispatch_pointer(&mut hub, InputEvent::PointerDown(at), now_ms())
            .map_err(err)?;
        app.dispatch_pointer(&mut hub, InputEvent::PointerUp(at), now_ms())
            .map_err(err)?;
    }
    ensure!(
        hub.prefs.light && hub.view == View::Settings,
        "connecting UI did not accept input"
    );
    let input_ms = phase.elapsed().as_secs_f64() * 1000.;
    // Save after first-draw timing. Filesystem/PNG work is outside both measured milestones.
    let deadline = Instant::now() + Duration::from_secs(30);
    let connected = loop {
        ensure!(
            Instant::now() < deadline,
            "Hub did not connect within 30 seconds"
        );
        if let Some(event) = connection.wait(Duration::from_millis(2)) {
            if let Event::Offline { ref message, .. } = event {
                anyhow::bail!("startup: {message}")
            }
            hub.event(event);
            if hub.link == Link::Online {
                app.rebuild(&mut hub, now_ms()).map_err(err)?;
                app.frame().map_err(err)?;
                surface.render(&app)?;
                break now_ms();
            }
        }
    };
    connection.stop();
    first_pixels.save(output.join("first-frame.png"))?;
    surface
        .ui_gpu
        .pixels()?
        .save(output.join("connected.png"))?;
    ara_update::write_json(
        &output.join("startup.json"),
        &json!({"firstFrameUnixMs":first,"connectedUnixMs":connected,"firstFrameBuiltinImages":first_images,"probeMs":started.elapsed().as_secs_f64()*1000.,"inputWhileConnectingMs":input_ms,"build":crate::build_identity(),"pid":std::process::id()}),
    )?;
    Ok(())
}
