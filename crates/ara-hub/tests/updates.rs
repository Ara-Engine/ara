use ara_hub::{
    capture,
    connection::Event,
    model::{Call, Dialog, DownloadPolicy, Effect, Hub, Intent, Prefs, View},
    view::{self, Binding, Ctx},
};
use serde_json::json;

#[test]
fn install_and_open_waits_for_success_and_restart_uses_authenticated_staging() {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    hub.releases.jobs.clear();
    hub.projects[2].engine_version = "0.0.3-alpha".into();
    hub.take_calls();
    hub.intent(Intent::InstallAndOpen {
        project: "project-3".into(),
        headless: false,
    });
    let calls = hub.take_calls();
    assert!(
        calls
            .iter()
            .any(|c| c.method() == "hub.engine.install" && c.params()["version"] == "0.0.3-alpha")
    );
    assert!(!calls.iter().any(|c| c.method() == "hub.open"));
    hub.event(Event::Reply { call:Call::Release { method:"hub.downloads",params:json!({}) },result:Ok(json!({"jobs":[{"id":"engine-0.0.3-alpha","kind":"engine","version":"0.0.3-alpha","state":"complete","result":{"engine":{}}}]})) });
    assert!(
        hub.take_calls()
            .iter()
            .any(|c| c.method() == "hub.open" && c.params()["project"] == "project-3")
    );
    hub.event(Event::Reply { call:Call::Release { method:"hub.downloads",params:json!({}) },result:Ok(json!({"jobs":[{"id":"hub-0.1.0","kind":"hub","version":"0.1.0","state":"complete","result":{"archive":"target/test.zip","manifest":"signed bytes","signature":"detached"}}]})) });
    hub.intent(Intent::RestartUpdate);
    assert!(
        matches!(hub.take_effects().as_slice(),[Effect::ApplyUpdate{manifest,signature,..}] if manifest=="signed bytes"&&signature=="detached")
    );
}

#[test]
fn installs_settings_and_banner_bind_to_service_actions() {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    let ctx = Ctx {
        now_ms: capture::NOW,
        size: [1440., 900.],
        thumbnail: &|_| None,
        anchor: None,
        docs: &[],
        dialog_close_hint: false,
    };
    hub.view = View::Installs;
    let page = view::page(&hub, &ctx);
    assert!(page.bindings.values().any(
        |b| matches!(b,Binding::Intent(Intent::CancelDownload(id)) if id=="engine-0.0.3-alpha")
    ));
    assert!(
        page.bindings
            .values()
            .any(|b| matches!(b, Binding::Intent(Intent::Dialog(Dialog::Update))))
    );
    hub.view = View::Settings;
    let page = view::page(&hub, &ctx);
    assert!(
        page.bindings
            .values()
            .any(|b| matches!(b, Binding::Intent(Intent::CheckUpdates)))
    );
    assert!(
        page.markup.contains("label='Release channel'"),
        "the shared select owns channel choice actions"
    );
}

#[test]
fn discovering_an_update_never_downloads_and_skip_is_persisted_per_build() {
    let mut hub = Hub::new(Prefs::default(), "target/hub-tests".into(), None);
    hub.releases.can_self_update = true;
    let release = json!({"format":"ara.release/1","kind":"hub","version":"99.0.0","channel":"stable","minHubVersion":"0.0.1-alpha","platform":"windows-x64","notes":"A deliberate update","assets":[]});
    let call = Call::Release {
        method: "hub.updates",
        params: json!({}),
    };
    hub.event(Event::Reply {
        call: call.clone(),
        result: Ok(json!({"releases":[release.clone()]})),
    });
    assert!(
        hub.take_calls()
            .iter()
            .all(|call| call.method() != "hub.update.prepare")
    );
    assert!(hub.take_effects().is_empty());
    hub.intent(Intent::Dialog(Dialog::Update));
    hub.intent(Intent::DismissDialog);
    assert!(
        hub.take_calls().is_empty(),
        "Later does not start a transfer"
    );
    hub.intent(Intent::SkipUpdate);
    let prefs = serde_json::to_value(&hub.prefs).expect("persist preferences");
    let mut restored = Hub::new(
        serde_json::from_value(prefs).expect("load preferences"),
        "target/hub-tests".into(),
        None,
    );
    restored.event(Event::Reply {
        call,
        result: Ok(json!({"releases":[release]})),
    });
    assert_eq!(restored.prefs.skipped_update.as_deref(), Some("99.0.0"));
    restored.intent(Intent::PrepareUpdate);
    assert!(
        restored
            .take_calls()
            .iter()
            .any(|call| call.method() == "hub.update.prepare")
    );
    assert!(
        restored.take_effects().is_empty(),
        "download does not restart the Hub"
    );
}

#[test]
fn confirmation_runs_the_exact_selected_build_after_an_engine_list_refresh() {
    let mut hub = Hub::new(Prefs::default(), "target/hub-tests".into(), None);
    capture::seed(&mut hub);
    let engine = hub.engines[1].clone();
    hub.intent(Intent::Dialog(Dialog::Confirm {
        title: "Uninstall engine?".into(),
        detail: "Projects stay on disk.".into(),
        label: "Uninstall".into(),
        destructive: true,
        action: Box::new(Intent::RemoveEngineBuild(engine.clone())),
    }));
    assert!(hub.take_calls().is_empty());
    hub.engines.reverse();
    hub.intent(Intent::ConfirmDialog);
    let calls = hub.take_calls();
    assert!(
        calls
            .iter()
            .any(|call| call.method() == "hub.engine.uninstall"
                && call.params()["version"] == engine.version)
    );
}

#[test]
fn background_downloads_require_a_persisted_opt_in_and_never_restart_the_hub() {
    let mut hub = Hub::new(Prefs::default(), "target/hub-tests".into(), None);
    capture::seed(&mut hub);
    hub.releases.jobs.clear();
    assert_eq!(hub.prefs.hub_update_downloads, DownloadPolicy::Ask);
    hub.intent(Intent::HubUpdateDownloads(DownloadPolicy::Background));
    assert_eq!(
        hub.take_calls()
            .iter()
            .filter(|call| call.method() == "hub.update.prepare")
            .count(),
        1
    );
    assert!(hub.take_effects().is_empty());
    let persisted = serde_json::to_value(&hub.prefs).expect("persist policy");
    let loaded: Prefs = serde_json::from_value(persisted).expect("load policy");
    assert_eq!(loaded.hub_update_downloads, DownloadPolicy::Background);
    for state in ["running", "complete"] {
        hub.event(Event::Reply {
            call: Call::Release { method: "hub.downloads", params: json!({}) },
            result: Ok(json!({"jobs":[{"id":"hub-0.1.0","kind":"hub","version":"0.1.0","state":state,"result":{"archive":"target/test.zip","manifest":"signed bytes","signature":"detached"}}]})),
        });
        assert!(
            hub.take_effects().is_empty(),
            "a service snapshot cannot restart the GUI"
        );
    }
    hub.intent(Intent::Dialog(Dialog::Update));
    hub.intent(Intent::DismissDialog);
    assert!(
        hub.take_effects().is_empty(),
        "closing Ready does not apply the update"
    );
    hub.intent(Intent::HubUpdateDownloads(DownloadPolicy::Ask));
    assert!(hub.take_effects().is_empty());
    hub.intent(Intent::RestartUpdate);
    assert!(matches!(
        hub.take_effects().as_slice(),
        [Effect::ApplyUpdate { .. }]
    ));
}

#[test]
fn pending_install_is_deduplicated_and_cancel_is_forwarded_when_the_job_answers() {
    let mut hub = Hub::new(Prefs::default(), "target/hub-tests".into(), None);
    capture::seed(&mut hub);
    hub.releases.jobs.clear();
    hub.intent(Intent::InstallEngine("0.0.3-alpha".into()));
    hub.intent(Intent::InstallEngine("0.0.3-alpha".into()));
    let calls = hub.take_calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.method() == "hub.engine.install")
            .count(),
        1
    );
    hub.intent(Intent::CancelPending {
        kind: "engine".into(),
        version: "0.0.3-alpha".into(),
    });
    assert!(
        hub.take_calls().is_empty(),
        "wait for the actual job identity before cancelling"
    );
    hub.event(Event::Reply {
        call: calls.into_iter().next().expect("install request"),
        result: Ok(json!({"id":"engine-0.0.3-alpha","kind":"engine","version":"0.0.3-alpha","state":"running"})),
    });
    assert!(
        hub.take_calls()
            .iter()
            .any(|call| call.method() == "hub.download.cancel"
                && call.params()["job"] == "engine-0.0.3-alpha")
    );
    assert!(hub.take_effects().is_empty());
}

#[test]
fn cancelling_install_and_open_reuses_the_transfer_but_never_opens_on_a_late_completion() {
    let mut hub = Hub::new(Prefs::default(), "target/hub-tests".into(), None);
    capture::seed(&mut hub);
    hub.projects[2].engine_version = "0.0.3-alpha".into();
    hub.intent(Intent::InstallAndOpen {
        project: "project-3".into(),
        headless: false,
    });
    assert!(
        hub.take_calls().is_empty(),
        "the existing engine transfer is reused"
    );
    hub.intent(Intent::CancelDownload("engine-0.0.3-alpha".into()));
    hub.take_calls();
    hub.event(Event::Reply {
        call: Call::Release { method: "hub.downloads", params: json!({}) },
        result: Ok(json!({"jobs":[{"id":"engine-0.0.3-alpha","kind":"engine","version":"0.0.3-alpha","state":"complete"}]})),
    });
    assert!(
        hub.take_calls()
            .iter()
            .all(|call| call.method() != "hub.open")
    );
}

#[test]
fn one_failed_engine_does_not_discard_other_projects_waiting_for_a_different_engine() {
    let mut hub = Hub::new(Prefs::default(), "target/hub-tests".into(), None);
    capture::seed(&mut hub);
    hub.releases.jobs.clear();
    hub.projects[2].engine_version = "0.0.3-alpha".into();
    hub.projects[3].engine_version = "0.0.4-alpha".into();
    for project in ["project-3", "project-4"] {
        hub.intent(Intent::InstallAndOpen {
            project: project.into(),
            headless: false,
        });
    }
    let calls = hub.take_calls();
    let failed = calls
        .into_iter()
        .find(|call| call.params()["version"] == "0.0.3-alpha")
        .expect("first engine install");
    hub.event(Event::Reply {
        call: failed,
        result: Err(ara_diag::Diagnostic::error(
            ara_update::IO,
            "The first engine could not be downloaded.",
        )
        .into()),
    });
    hub.event(Event::Reply {
        call: Call::Release { method: "hub.downloads", params: json!({}) },
        result: Ok(json!({"jobs":[{"id":"engine-0.0.4-alpha","kind":"engine","version":"0.0.4-alpha","state":"complete"}]})),
    });
    let calls = hub.take_calls();
    assert!(
        calls
            .iter()
            .any(|call| call.method() == "hub.open" && call.params()["project"] == "project-4")
    );
    assert!(
        calls
            .iter()
            .all(|call| call.method() != "hub.open" || call.params()["project"] != "project-3")
    );
}
