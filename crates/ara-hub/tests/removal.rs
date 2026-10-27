//! Managed deletion requires a completed read-only scope check and explicit confirmation.
use ara_hub::{
    capture,
    connection::Event,
    model::{Call, Dialog, Hub, Intent, Prefs, View},
    native::App,
};
use ara_ui::{input::InputEvent, source::stable_id};
use serde_json::{Value, json};

const VERSION: &str = "0.0.2-nightly";
const JOB: &str = "uninstall-preflight-0.0.2-nightly";

fn setup() -> (App, Hub) {
    let mut hub = Hub::new(Prefs::default(), "target/hub-removal-tests".into(), None);
    capture::seed(&mut hub);
    hub.view = View::Installs;
    let mut app = App::new([1280., 720.], vec![]).expect("UI");
    app.rebuild(&mut hub, capture::NOW).expect("installs");
    app.input
        .focus(&mut app.ui, stable_id("engine-remove-1"))
        .expect("uninstall control");
    app.frame().expect("reveal uninstall");
    (app, hub)
}

fn click(app: &mut App, hub: &mut Hub, id: &str) {
    let rect = app.ui.nodes[app.ui.find(id).expect(id)].rect;
    let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
    for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
        app.dispatch_pointer(hub, event, capture::NOW)
            .expect("click");
    }
}

fn inspection(hub: &mut Hub) -> Call {
    let calls = hub.take_calls();
    assert_eq!(calls.len(), 1);
    let call = calls.into_iter().next().expect("inspection request");
    assert_eq!(call.method(), "hub.engine.uninstall.preflight");
    call
}

fn job(state: &str) -> Value {
    json!({"id":JOB,"kind":"uninstall-preflight","version":VERSION,"state":state,"phase":if state=="running" {"inspecting"}else{state},"cancellable":true,
        "result":(state=="complete").then(||json!({"version":VERSION,"destination":r"C:\Users\tester\AppData\Local\Ara\Engines\0.0.2-nightly","bytes":2_450_000_000_u64,"files":8500}))})
}

fn reply(app: &mut App, hub: &mut Hub, call: Call, value: Value) {
    hub.event(Event::Reply {
        call,
        result: Ok(value),
    });
    app.rebuild(hub, capture::NOW).expect("inspection reply");
}

#[test]
fn uninstall_waits_for_scope_and_space_then_for_the_explicit_confirming_click() {
    let (mut app, mut hub) = setup();
    click(&mut app, &mut hub, "engine-remove-1");
    let call = inspection(&mut hub);
    assert!(
        app.ui.nodes[app.ui.find("dialog-confirm").expect("confirm")]
            .state
            .disabled
    );
    click(&mut app, &mut hub, "dialog-confirm");
    hub.intent(Intent::ConfirmDialog);
    assert!(
        hub.take_calls().is_empty(),
        "even the model cannot uninstall before inspection completes"
    );
    assert!(matches!(hub.dialog, Some(Dialog::UninstallEngine(_))));
    reply(&mut app, &mut hub, call, job("complete"));
    assert_eq!(
        hub.releases.removal(VERSION).expect("summary").bytes,
        2_450_000_000
    );
    assert!(
        !app.ui.nodes[app.ui.find("dialog-confirm").expect("confirm")]
            .state
            .disabled
    );
    assert!(
        hub.take_calls().is_empty(),
        "completing a read-only check never starts deletion"
    );
    click(&mut app, &mut hub, "dialog-cancel");
    assert!(hub.take_calls().is_empty());
    click(&mut app, &mut hub, "engine-remove-1");
    let call = inspection(&mut hub);
    reply(&mut app, &mut hub, call, job("complete"));
    click(&mut app, &mut hub, "dialog-confirm");
    assert_eq!(
        hub.take_calls()
            .iter()
            .filter(|call| call.method() == "hub.engine.uninstall")
            .count(),
        1
    );
}

#[test]
fn cancelling_a_busy_scope_check_cannot_delete_or_reopen_a_stale_error() {
    let (mut app, mut hub) = setup();
    click(&mut app, &mut hub, "engine-remove-1");
    let call = inspection(&mut hub);
    click(&mut app, &mut hub, "dialog-cancel");
    assert!(hub.dialog.is_none());
    assert!(
        hub.take_calls().is_empty(),
        "pending inspection cancellation waits for its job identity"
    );
    reply(&mut app, &mut hub, call, job("running"));
    let cancels = hub.take_calls();
    assert_eq!(cancels.len(), 1);
    assert_eq!(cancels[0].method(), "hub.download.cancel");
    assert_eq!(cancels[0].params()["job"], JOB);
    let mut failed = job("failed");
    failed["error"] = serde_json::to_value(ara_diag::Diagnostic::error(
        ara_update::IO,
        "Late inspection failure",
    ))
    .expect("error");
    reply(
        &mut app,
        &mut hub,
        Call::Release {
            method: "hub.downloads",
            params: json!({}),
        },
        json!({"jobs":[failed]}),
    );
    hub.event(Event::Reply {
        call: cancels.into_iter().next().expect("cancel"),
        result: Err(
            ara_diag::Diagnostic::error(ara_update::IO, "Inspection already finished").into(),
        ),
    });
    app.rebuild(&mut hub, capture::NOW)
        .expect("late cancellation result");
    assert!(hub.dialog.is_none() && hub.notices.is_empty());
    assert!(hub.engines.iter().any(|engine| engine.version == VERSION));
    assert!(
        hub.take_calls()
            .iter()
            .all(|call| call.method() != "hub.engine.uninstall")
    );
}

#[test]
fn reopening_during_cancellation_starts_a_fresh_check_before_enabling_delete() {
    let (mut app, mut hub) = setup();
    click(&mut app, &mut hub, "engine-remove-1");
    let old = inspection(&mut hub);
    click(&mut app, &mut hub, "dialog-cancel");
    click(&mut app, &mut hub, "engine-remove-1");
    assert!(
        hub.take_calls().is_empty(),
        "wait for the cancelled check before reusing its job identity"
    );
    reply(&mut app, &mut hub, old, job("running"));
    assert!(
        hub.take_calls()
            .iter()
            .all(|call| call.method() == "hub.download.cancel")
    );
    reply(
        &mut app,
        &mut hub,
        Call::Release {
            method: "hub.downloads",
            params: json!({}),
        },
        json!({"jobs":[job("cancelled")]}),
    );
    let fresh = inspection(&mut hub);
    assert!(
        app.ui.nodes[app.ui.find("dialog-confirm").expect("confirm")]
            .state
            .disabled
    );
    reply(&mut app, &mut hub, fresh, job("complete"));
    assert!(
        !app.ui.nodes[app.ui.find("dialog-confirm").expect("confirm")]
            .state
            .disabled
    );
    assert!(hub.dialog.is_some() && hub.notices.is_empty());
}
