//! Native compatibility consent preserves the requested mode and typed pin boundary.
use ara_hub::{
    capture,
    connection::Event,
    model::{Call, Dialog, Hub, Intent, Prefs},
    native::App,
};
use ara_ui::input::InputEvent;
use serde_json::{Value, json};

fn setup(headless: bool, available: bool) -> (App, Hub) {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    hub.status.remove("project-1");
    hub.intent(if headless {
        Intent::OpenHeadless("project-1".into())
    } else {
        Intent::Open("project-1".into())
    });
    let call = hub
        .take_calls()
        .into_iter()
        .find(|call| matches!(call, Call::Open { .. }))
        .expect("open attempt");
    hub.event(Event::Reply {
        call,
        result: Err(ara_diag::Diagnostic::error(
            ara_diag::codes::PROJECT_ENGINE_LAYOUT_UNSUPPORTED,
            "This project uses the `content/` layout and needs Ara preview 4 or newer",
        )
        .into()),
    });
    let call = hub
        .take_calls()
        .into_iter()
        .next()
        .expect("read-only engine check");
    assert_eq!(call.method(), "hub.project.currentEngine");
    assert_eq!(call.params()["headless"], headless);
    hub.event(Event::Reply {
        call,
        result: Ok(json!({"engineVersion":"0.0.1-alpha","available":available,"registered":false})),
    });
    let mut app = App::new([1280., 720.], Vec::new()).expect("native UI");
    app.rebuild(&mut hub, capture::NOW)
        .expect("compatibility modal");
    (app, hub)
}

fn click(app: &mut App, hub: &mut Hub, id: &str) {
    let node = &app.ui.nodes[app.ui.find(id).expect(id)];
    assert!(node.rect.intersects(node.clip), "{id} is visible");
    assert!(
        node.rect.x >= 0.
            && node.rect.y >= 0.
            && node.rect.x + node.rect.w <= 1280.5
            && node.rect.y + node.rect.h <= 720.5,
        "{id} fits the viewport"
    );
    let at = [
        node.rect.x + node.rect.w / 2.,
        node.rect.y + node.rect.h / 2.,
    ];
    for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
        app.dispatch_pointer(hub, event, capture::NOW)
            .expect("pointer");
    }
}

fn pin(state: &str) -> Value {
    json!({"id":"typed-pin","project":"project-1","kind":"pin","state":state,"phase":"Saving files and history","cancellable":false,"progress":0.,"result":if state=="complete"{json!({"engineVersion":"0.0.1-alpha","saved":true})}else{Value::Null},"diagnostics":[]})
}

#[test]
fn cancel_and_escape_never_pin_install_or_open() {
    for available in [true, false] {
        let (mut app, mut hub) = setup(false, available);
        assert_eq!(app.focused(), Some("engine-layout-cancel"));
        assert!(
            hub.take_calls().is_empty(),
            "a decision never prepares a pin or install"
        );
        click(&mut app, &mut hub, "engine-layout-cancel");
        assert!(hub.engine_layout.is_none() && hub.dialog.is_none());
        assert!(hub.take_calls().is_empty());
        let (mut app, mut hub) = setup(true, available);
        for intent in app.key(&hub, 41, true).expect("Escape") {
            hub.intent(intent);
        }
        assert!(hub.engine_layout.is_none() && hub.take_calls().is_empty());
    }
}

#[test]
fn explicit_current_choice_waits_for_persistence_and_resumes_the_original_mode() {
    for headless in [true, false] {
        let (mut app, mut hub) = setup(headless, true);
        click(&mut app, &mut hub, "engine-layout-use");
        let calls = hub.take_calls();
        assert_eq!(calls.len(), 1);
        let call = calls.into_iter().next().expect("typed choice");
        assert_eq!(call.method(), "hub.project.useCurrentEngine");
        assert_eq!(call.params()["headless"], headless);
        hub.event(Event::Reply {
            call,
            result: Ok(pin("running")),
        });
        app.rebuild(&mut hub, capture::NOW).expect("saving modal");
        for intent in app.key(&hub, 41, true).expect("Escape while saving") {
            hub.intent(intent);
        }
        assert!(matches!(hub.dialog, Some(Dialog::EngineLayout(_))));
        let pending = hub.take_calls();
        assert!(
            !pending
                .iter()
                .any(|call| matches!(call, Call::Open { .. } | Call::Pin { .. })),
            "there is no early open or direct marker-pin call"
        );
        assert_eq!(pending.len(), 1);
        let status = pending.into_iter().next().expect("physical receipt status");
        hub.event(Event::Reply {
            call: status,
            result: Ok(pin("complete")),
        });
        assert!(hub.engine_layout.is_none());
        assert_eq!(
            hub.take_calls()
                .into_iter()
                .filter(|call| matches!(call, Call::Open { .. }))
                .collect::<Vec<_>>(),
            [Call::Open {
                project: "project-1".into(),
                headless
            }]
        );
    }
}

#[test]
fn install_choice_uses_signed_confirmation_and_rechecks_capabilities_before_pinning() {
    let (mut app, mut hub) = setup(true, false);
    click(&mut app, &mut hub, "engine-layout-use");
    assert_eq!(
        hub.dialog,
        Some(Dialog::InstallCurrentEngine("0.0.1-alpha".into()))
    );
    let calls = hub.take_calls();
    assert!(
        calls
            .iter()
            .all(|call| call.method() == "hub.engine.preflight"),
        "the shared install confirmation only reads a space summary"
    );
    hub.intent(Intent::InstallCurrentEngine("0.0.1-alpha".into()));
    let calls = hub.take_calls();
    let install = calls
        .iter()
        .find(|call| call.method() == "hub.engine.installCurrent")
        .expect("explicit signed install")
        .clone();
    assert!(
        !calls
            .iter()
            .any(|call| call.method() == "hub.project.useCurrentEngine")
    );
    hub.event(Event::Reply{call:install,result:Ok(json!({"id":"engine-current","kind":"engine-current","version":"0.0.1-alpha","state":"running","phase":"downloading","progress":{"received":0,"total":1},"cancellable":true}))});
    hub.event(Event::Reply{call:Call::Release{method:"hub.downloads",params:json!({})},result:Ok(json!({"jobs":[{"id":"engine-current","kind":"engine-current","version":"0.0.1-alpha","state":"complete","phase":"complete","progress":{"received":1,"total":1},"cancellable":false}]}))});
    let calls = hub.take_calls();
    let check = calls
        .into_iter()
        .find(|call| call.method() == "hub.project.currentEngine")
        .expect("installed CLI declaration must be checked");
    hub.event(Event::Reply {
        call: check,
        result: Ok(json!({"engineVersion":"0.0.1-alpha","available":true,"registered":true})),
    });
    assert!(
        hub.take_calls()
            .iter()
            .any(|call| call.method() == "hub.project.useCurrentEngine")
    );
}

#[test]
fn late_readonly_check_after_cancel_cannot_resume_an_open() {
    let (_, mut hub) = setup(true, true);
    hub.intent(Intent::CancelEngineLayout);
    // A reply from the old request generation is ignored even if it says capable.
    hub.event(Event::Reply {
        call: Call::ProjectEngine {
            project: "project-1".into(),
            request: 1,
            method: "hub.project.currentEngine",
            params: json!({"headless":true}),
        },
        result: Ok(json!({"engineVersion":"0.0.1-alpha","available":true,"registered":true})),
    });
    assert!(hub.engine_layout.is_none() && hub.dialog.is_none() && hub.take_calls().is_empty());
}

#[test]
fn old_or_unconfirmed_install_completion_cannot_authorize_a_project_pin() {
    let (mut app, mut hub) = setup(true, false);
    click(&mut app, &mut hub, "engine-layout-use");
    hub.take_calls();
    hub.event(Event::Reply{call:Call::Release{method:"hub.downloads",params:json!({})},result:Ok(json!({"jobs":[{"id":"engine-current-old","kind":"engine-current","version":"0.0.1-alpha","state":"complete","phase":"complete","cancellable":false}]}))});
    assert!(
        !hub.take_calls().iter().any(|call| matches!(
            call,
            Call::ProjectEngine { .. } | Call::Open { .. } | Call::Pin { .. }
        )),
        "a different/older job cannot stand in for the explicit install confirmation"
    );
    hub.intent(Intent::DismissDialog);
    assert!(hub.engine_layout.is_none());
}
