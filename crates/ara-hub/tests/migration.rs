//! Consent is exercised through the same native controls used by the Hub window.
use ara_hub::{
    capture,
    connection::Event,
    model::{Call, Dialog, Hub, Intent, Prefs},
    native::App,
};
use ara_ui::input::InputEvent;
use serde_json::{Value, json};

fn setup() -> (App, Hub) {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    capture::seed_migration(&mut hub);
    let mut app = App::new([1280., 720.], Vec::new()).expect("native UI");
    app.rebuild(&mut hub, capture::NOW)
        .expect("migration preview");
    (app, hub)
}

fn click(app: &mut App, hub: &mut Hub, id: &str) {
    let node = &app.ui.nodes[app.ui.find(id).expect(id)];
    assert!(
        node.rect.intersects(node.clip),
        "{id} must be visible without scrolling the dialog"
    );
    let at = [
        node.rect.x + node.rect.w / 2.,
        node.rect.y + node.rect.h / 2.,
    ];
    for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
        app.dispatch_pointer(hub, event, capture::NOW)
            .expect("native pointer");
    }
}

fn snapshot(state: &str, cancellable: bool) -> Value {
    json!({"id":"typed-migration","project":"project-1","kind":"migrate","state":state,"phase":"Saving files and history","cancellable":cancellable,"progress":0.8,"result":if state=="complete"{json!({"restartRequired":true})}else{Value::Null},"diagnostics":[]})
}

#[test]
fn preview_never_migrates_and_cancel_keeps_the_project_closed() {
    let (mut app, mut hub) = setup();
    assert_eq!(app.focused(), Some("migration-cancel"));
    assert!(
        hub.take_calls().is_empty(),
        "preview cannot queue a mutation"
    );
    click(&mut app, &mut hub, "migration-cancel");
    assert!(hub.migration.is_none() && hub.dialog.is_none());
    assert!(
        hub.take_calls().is_empty(),
        "Cancel neither migrates nor opens"
    );
}

#[test]
fn explicit_migrate_waits_for_physical_completion_before_opening() {
    let (mut app, mut hub) = setup();
    click(&mut app, &mut hub, "migration-submit");
    let calls = hub.take_calls();
    assert_eq!(calls.len(), 1);
    let call = calls.into_iter().next().expect("explicit typed request");
    assert_eq!(call.method(), "hub.project.migrate");
    assert_eq!(call.params()["planToken"], "fixture-readonly-plan");
    hub.event(Event::Reply {
        call,
        result: Ok(snapshot("running", false)),
    });
    app.rebuild(&mut hub, capture::NOW).expect("committing");
    for intent in app.key(&hub, 41, true).expect("Escape during commit") {
        hub.intent(intent);
    }
    assert!(
        matches!(hub.dialog, Some(Dialog::Migration(_))),
        "a committed migration must finish"
    );
    let pending = hub.take_calls();
    assert!(!pending.iter().any(|call| call.method() == "hub.open"));
    for call in pending {
        hub.event(Event::Reply {
            call,
            result: Ok(snapshot("complete", false)),
        });
    }
    assert!(hub.migration.is_none());
    assert_eq!(
        hub.take_calls()
            .into_iter()
            .filter(|call| matches!(call, Call::Open { .. }))
            .collect::<Vec<_>>(),
        [Call::Open {
            project: "project-1".into(),
            headless: false
        }]
    );
}

#[test]
fn cancelled_readonly_reply_cannot_reopen_the_dialog_or_start_a_migration() {
    let (_, mut hub) = setup();
    hub.intent(Intent::CancelMigration);
    hub.intent(Intent::ReviewMigration {
        project: "project-1".into(),
        headless: false,
    });
    let call = hub
        .take_calls()
        .into_iter()
        .next()
        .expect("readonly request");
    hub.intent(Intent::CancelMigration);
    hub.event(Event::Reply {
        call,
        result: Err(
            ara_diag::Diagnostic::error(ara_diag::codes::IO, "late inspection error").into(),
        ),
    });
    assert!(hub.dialog.is_none() && hub.migration.is_none());
    assert!(hub.take_calls().is_empty());
}

#[test]
fn both_themes_keep_consent_buttons_bounded_and_preview_scrollable() {
    for light in [false, true] {
        let (mut app, mut hub) = setup();
        hub.prefs.light = light;
        app.rebuild(&mut hub, capture::NOW).expect("theme");
        for id in ["migration-submit", "migration-cancel", "dialog-close"] {
            let node = &app.ui.nodes[app.ui.find(id).expect(id)];
            assert!(
                node.rect.x >= 0.
                    && node.rect.y >= 0.
                    && node.rect.x + node.rect.w <= 1280.5
                    && node.rect.y + node.rect.h <= 720.5,
                "{id} stays inside the window: {:?}",
                node.rect
            );
            assert!(node.rect.intersects(node.clip), "{id} remains visible");
        }
        assert!(app.ui.find("migration-details-scroll").is_some());
        assert!(app.ui.nodes.iter().any(|node| {
            node.element
                .attrs
                .get("text")
                .is_some_and(|text| text.contains("content/levels/"))
        }));
    }
}
