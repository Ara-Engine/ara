//! Installed live startup, drawn headlessly instead of in a desktop window: how soon the
//! first frame exists, that input works while the service connects, and when it is online.
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
    let mut cap = Capture::new(options.size, options.dpi.unwrap_or(1.0))?;
    cap.settle(&mut hub);
    let first = now_ms();
    ara_hub_service::startup::record("ui.first_draw", phase.elapsed());
    // The same input path a person uses, while the service is still connecting.
    let phase = Instant::now();
    for key in ["theme", "nav-settings"] {
        cap.click(&mut hub, key)?;
    }
    ensure!(
        hub.prefs.light && hub.view == View::Settings,
        "connecting UI did not accept input"
    );
    let input_ms = phase.elapsed().as_secs_f64() * 1000.0;
    cap.save(&mut hub, &output.join("first-frame.png"))?;
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
                cap.settle(&mut hub);
                break now_ms();
            }
        }
    };
    connection.stop();
    cap.save(&mut hub, &output.join("connected.png"))?;
    ara_update::write_json(
        &output.join("startup.json"),
        &json!({"firstFrameUnixMs":first,"connectedUnixMs":connected,"probeMs":started.elapsed().as_secs_f64()*1000.,"inputWhileConnectingMs":input_ms,"build":crate::build_identity(),"pid":std::process::id()}),
    )?;
    Ok(())
}
