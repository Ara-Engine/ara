//! Preview 4 flow evidence uses real input and offscreen GPU frames only.
use super::*;
use crate::model::Dialog;

pub(super) fn run(
    host: &mut Offscreen,
    app: &mut crate::native::App,
    folder: &Path,
    light: bool,
) -> anyhow::Result<()> {
    let theme = if light { "light" } else { "dark" };
    let mut hub = Hub::new(
        crate::model::Prefs {
            light,
            ..Default::default()
        },
        "target/hub-flows".into(),
        None,
    );
    seed(&mut hub);
    hub.releases.jobs.clear();
    hub.dialog = None;
    hub.view = View::Settings;
    hub.releases.checking = true;
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-update-checking"),
    )?;
    hub.releases.checking = false;
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-update-preferences"),
    )?;
    hub.view = View::Projects;
    for (name, action) in [
        ("update-available", None),
        ("update-modal", Some(Intent::Dialog(Dialog::Update))),
    ] {
        if let Some(action) = action {
            hub.intent(action);
        }
        save(host, app, &mut hub, folder, &format!("{theme}-{name}"))?;
    }
    hub.intent(Intent::PrepareUpdate);
    reply(
        &mut hub,
        "hub.update.prepare",
        json!({"id":"hub-0.1.0","kind":"hub","version":"0.1.0","state":"running","phase":"downloading","progress":{"received":7_020_000,"total":18_000_000},"disk":{"availableBytes":42_000_000_000_u64,"requiredBytes":160_000_000}}),
    );
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-update-progress"),
    )?;
    reply(
        &mut hub,
        "hub.downloads",
        json!({"jobs":[{"id":"hub-0.1.0","kind":"hub","version":"0.1.0","state":"running","phase":"verifying","progress":{"received":18_000_000,"total":18_000_000}}]}),
    );
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-update-verifying"),
    )?;
    reply(
        &mut hub,
        "hub.downloads",
        json!({"jobs":[{"id":"hub-0.1.0","kind":"hub","version":"0.1.0","state":"complete","phase":"complete","result":{"archive":"target/fixture.zip","manifest":"fixture","signature":"fixture"}}]}),
    );
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-update-restart"),
    )?;
    hub.intent(Intent::DismissDialog);
    hub.view = View::Installs;
    hub.intent(Intent::Dialog(Dialog::InstallEngine("0.0.3-alpha".into())));
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-install-engine"),
    )?;
    reply(
        &mut hub,
        "hub.engine.install",
        json!({"id":"engine-0.0.3-alpha","kind":"engine","version":"0.0.3-alpha","state":"running","phase":"extracting","progress":{"received":864_000_000,"total":864_000_000},"disk":{"availableBytes":42_000_000_000_u64,"requiredBytes":3_100_000_000_u64}}),
    );
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-install-progress"),
    )?;
    let diagnostic = ara_diag::Diagnostic::error(
        ara_update::IO,
        "There isn't enough disk space to install this engine. Free 3.1 GB on the installation drive, then retry.",
    );
    reply(
        &mut hub,
        "hub.downloads",
        json!({"jobs":[{"id":"engine-0.0.3-alpha","kind":"engine","version":"0.0.3-alpha","state":"failed","error":diagnostic}]}),
    );
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-install-error"),
    )?;
    hub.intent(Intent::DismissDialog);
    hub.intent(Intent::Dialog(Dialog::RepairEngine("0.0.2-nightly".into())));
    save(
        host,
        app,
        &mut hub,
        folder,
        &format!("{theme}-repair-engine"),
    )?;
    hub.intent(Intent::DismissDialog);
    if let Some(engine) = hub.engines.iter().find(|engine| engine.managed).cloned() {
        let version = engine.version.clone();
        hub.intent(Intent::Dialog(Dialog::UninstallEngine(engine)));
        save(
            host,
            app,
            &mut hub,
            folder,
            &format!("{theme}-uninstall-inspecting"),
        )?;
        reply(
            &mut hub,
            "hub.engine.uninstall.preflight",
            json!({"id":format!("uninstall-preflight-{version}"),"kind":"uninstall-preflight","version":version,"state":"complete","phase":"complete","result":{"version":version,"destination":format!(r"C:\Users\tester\AppData\Local\Ara\Engines\{version}"),"bytes":2_450_000_000_u64,"files":8500}}),
        );
        save(
            host,
            app,
            &mut hub,
            folder,
            &format!("{theme}-uninstall-engine"),
        )?;
    }
    Ok(())
}

fn reply(hub: &mut Hub, method: &'static str, value: Value) {
    hub.event(Event::Reply {
        call: Call::Release {
            method,
            params: json!({}),
        },
        result: Ok(value),
    });
}

fn save(
    host: &mut Offscreen,
    app: &mut crate::native::App,
    hub: &mut Hub,
    folder: &Path,
    name: &str,
) -> anyhow::Result<()> {
    let started = Instant::now();
    settle(host, app, hub)?;
    let path = folder.join(format!("{name}.png"));
    host.capture(app)?.save(&path)?;
    let receipt = json!({
        "format":"ara.hub.flow/1","scenario":name,"version":env!("CARGO_PKG_VERSION"),"build":crate::build_identity(),
        "proof":"Offscreen presentation fixture; native input assertions are in ara-hub/tests/dialogs.rs",
        "logicalSize":app.size,"physicalSize":host.physical(),"light":hub.prefs.light,"checking":hub.releases.checking,
        "dialog":format!("{:?}",hub.dialog),"preferences":hub.prefs,"readyToRestart":hub.releases.prepared().is_some(),
        "removalSummary":hub.releases.jobs.iter().find(|job| job.kind=="uninstall-preflight"&&job.state=="complete").and_then(|job|job.result.as_ref()),
        "jobs":hub.releases.jobs.iter().map(|job| json!({"id":job.id,"kind":job.kind,"version":job.version,"state":job.state,"phase":job.phase})).collect::<Vec<_>>(),
        "elapsedMs":started.elapsed().as_secs_f64()*1000.,"screenshot":path.file_name().and_then(|name|name.to_str())
    });
    std::fs::write(
        folder.join(format!("{name}.json")),
        ara_json::to_canonical_string(&receipt),
    )?;
    println!("{}", path.display());
    Ok(())
}
