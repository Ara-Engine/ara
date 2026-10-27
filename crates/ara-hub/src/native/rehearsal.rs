//! Drives the real update model without creating a desktop window. Test builds only.
use super::*;
use crate::{
    connection::Event,
    model::{Call, DownloadPolicy, Intent},
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
    ensure!(
        hub.prefs.hub_update_downloads == DownloadPolicy::Ask,
        "rehearsal starts with Ask first"
    );
    hub.releases.can_self_update = true;
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut download_requested = false;
    let mut metadata_checks = 0_u32;
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
        metadata_checks += 1;
        let mut calls = hub.take_calls();
        ensure!(
            calls
                .iter()
                .all(|call| call.method() != "hub.update.prepare"),
            "Ask first metadata unexpectedly started an update download"
        );
        ensure!(
            hub.take_effects().is_empty(),
            "metadata unexpectedly requested an updater or restart"
        );
        if hub.releases.hub.is_some() && !download_requested {
            // The rehearsal deliberately chooses the same action as Update now.
            hub.intent(Intent::PrepareUpdate);
            download_requested = true;
            calls.extend(hub.take_calls());
        }
        for call in calls {
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
    ensure!(
        download_requested,
        "the verified update must follow an explicit PrepareUpdate action"
    );
    ensure!(
        hub.take_effects().is_empty(),
        "a verified-ready update applied before RestartUpdate"
    );
    let gate = restart_gate(config)?;
    let mut proof = json!({
        "oldBuild":crate::build_identity(),"oldPid":std::process::id(),"selected":&hub.releases.hub,
        "prepared":hub.releases.prepared(),"downloadPolicy":"ask","metadataChecks":metadata_checks,
        "metadataDidNotDownload":true,"downloadIntent":"PrepareUpdate","noApplyBeforeRestart":true,
        "restartRequested":false,"restartGate":gate
    });
    ara_update::write_json(report, &proof)?;
    if let Some(gate) = &gate {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match fs::read_to_string(gate) {
                Ok(action) => {
                    ensure!(
                        action.trim() == "restart",
                        "restart gate must contain the explicit action 'restart'"
                    );
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("read rehearsal restart gate"),
            }
            ensure!(
                hub.take_effects().is_empty(),
                "an update effect appeared while waiting for explicit restart consent"
            );
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for explicit rehearsal restart action"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    proof["restartRequested"] = json!(true);
    proof["restartIntent"] = json!("RestartUpdate");
    ara_update::write_json(report, &proof)?;
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

/// The rehearsal gate may only live under the already-isolated profile.
fn restart_gate(config: &Path) -> Result<Option<PathBuf>> {
    let Some(gate) = std::env::var_os("ARA_RELEASE_REHEARSAL_RESTART_GATE") else {
        return Ok(None);
    };
    let gate = PathBuf::from(gate);
    let parent = gate
        .parent()
        .context("rehearsal restart gate has no parent")?;
    ensure!(
        gate.is_absolute() && parent.canonicalize()?.starts_with(config.canonicalize()?),
        "rehearsal restart gate must be inside its isolated config directory"
    );
    Ok(Some(gate))
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
