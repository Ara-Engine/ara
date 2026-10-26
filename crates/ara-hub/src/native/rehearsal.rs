//! Drives the real update model without creating a desktop window. Test builds only.
use super::*;
use crate::{
    connection::Event,
    model::{Call, Intent},
};
use anyhow::{Result, ensure};
use ara_proto::Endpoint;
use serde_json::json;
use std::{fs, thread, time::Instant};

pub(super) fn run(options: &Options, directory: &Path, config: &Path) -> Result<()> {
    ensure!(
        directory.join("hub.json").is_file(),
        "rehearsal needs an installed standalone Hub"
    );
    if let Some(marker) = &options.update_health {
        return health(marker, directory, config);
    }
    let report = options
        .rehearse_update
        .as_ref()
        .context("rehearsal builds never open desktop windows; use --rehearse-update")?;
    let ara = updates::service(directory, options.ara.as_deref())?;
    let mut client =
        ara_hub_service::connect_or_start(config, &ara).map_err(|e| anyhow::anyhow!("{e}"))?;
    client
        .call("hub.releases.configure", json!({"channel":"preview"}))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut hub = Hub::new(Prefs::default(), config.to_path_buf(), None);
    hub.releases.can_self_update = true;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        for method in ["hub.updates", "hub.downloads"] {
            let call = Call::Release {
                method,
                params: json!({}),
            };
            let result = client.call(call.method(), call.params());
            if let Ok(value) = &result {
                ensure!(
                    value["status"]["state"] != "error",
                    "rehearsal feed failed: {}",
                    value["status"]
                );
            }
            hub.event(Event::Reply { call, result });
        }
        for call in hub.take_calls() {
            let result = client.call(call.method(), call.params());
            if let Err(error) = &result {
                anyhow::bail!("rehearsal update call {} failed: {error}", call.method());
            }
            hub.event(Event::Reply { call, result });
        }
        if hub.releases.prepared().is_some() {
            break;
        }
        ensure!(
            !hub.releases.jobs.iter().any(|job| job.state == "failed"),
            "rehearsal Hub download failed"
        );
        ensure!(
            Instant::now() < deadline,
            "older Hub never discovered/prepared a newer release"
        );
        thread::sleep(Duration::from_millis(50));
    }
    ara_update::write_json(
        report,
        &json!({"oldBuild":crate::build_identity(),"selected":&hub.releases.hub,"prepared":hub.releases.prepared()}),
    )?;
    hub.intent(Intent::RestartUpdate);
    for effect in hub.take_effects() {
        if let Effect::ApplyUpdate {
            archive,
            manifest,
            signature,
        } = effect
        {
            updates::launch(config, &ara, archive, manifest, signature)?;
            return Ok(());
        }
    }
    anyhow::bail!("Restart to update did not produce an updater request")
}

fn health(marker: &Path, directory: &Path, config: &Path) -> Result<()> {
    let mut offscreen = Offscreen::new([960, 600], 1.)?;
    let mut app = App::new(offscreen.logical(), os::docs(directory)).map_err(err)?;
    let mut hub = Hub::new(Prefs::default(), config.to_path_buf(), None);
    app.rebuild(&mut hub, now_ms()).map_err(err)?;
    app.frame().map_err(err)?;
    offscreen
        .capture(&app)?
        .save(config.join("rehearsal-updated-hub.png"))?;
    ara_update::write_json(
        &config.join("rehearsal-health.json"),
        &json!({"build":crate::build_identity(),"pid":std::process::id()}),
    )?;
    fs::write(marker, b"ready")?;
    // The production updater must observe readiness before this bounded host exits.
    let deadline = Instant::now() + Duration::from_secs(60);
    while marker.exists() {
        ensure!(
            Instant::now() < deadline,
            "updater did not acknowledge the offscreen frame"
        );
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}
