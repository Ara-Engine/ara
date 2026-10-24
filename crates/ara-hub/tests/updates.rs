use ara_hub::{
    capture,
    connection::Event,
    model::{Call, Effect, Hub, Intent, Prefs, View},
    view::{self, Binding, Ctx},
};
use serde_json::json;

#[test]
fn install_and_open_waits_for_success_and_restart_uses_authenticated_staging() {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
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
    };
    hub.view = View::Installs;
    let page = view::page(&hub, &ctx);
    assert!(page.bindings.values().any(
        |b| matches!(b,Binding::Intent(Intent::CancelDownload(id)) if id=="engine-0.0.3-alpha")
    ));
    assert!(
        page.bindings
            .values()
            .any(|b| matches!(b, Binding::Intent(Intent::RestartUpdate)))
    );
    hub.view = View::Settings;
    let page = view::page(&hub, &ctx);
    assert!(
        page.bindings
            .values()
            .any(|b| matches!(b, Binding::Intent(Intent::CheckUpdates)))
    );
    assert!(
        page.bindings
            .values()
            .any(|b| matches!(b, Binding::Intent(Intent::ReleaseChannel(_))))
    );
}
