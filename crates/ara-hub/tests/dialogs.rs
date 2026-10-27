//! Real native input exercises the decisions users make in the Hub.
use ara_hub::{
    capture,
    connection::Event,
    model::{Call, Dialog, Hub, Intent, Prefs, Sort, View},
    native::App,
};
use ara_ui::{input::InputEvent, source::stable_id};
use serde_json::json;

#[derive(Default)]
struct TestClipboard(String);
impl ara_ui_text::Clipboard for TestClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}

fn text(app: &mut App, hub: &mut Hub, value: &str) {
    let mut composing = false;
    assert!(app.edit(
        &mut composing,
        ara_ui_text::EditEvent::ImeCommit(value.into()),
        &mut TestClipboard::default()
    ));
    for intent in app.actions() {
        hub.intent(intent);
    }
    app.rebuild(hub, capture::NOW)
        .expect("native committed text");
}

#[test]
fn native_text_filters_twelve_versions_and_survives_a_release_reply_and_reorder() {
    let (mut app, mut hub) = setup();
    let base = hub.engines[0].clone();
    hub.engines = (0..12)
        .map(|index| {
            let mut engine = base.clone();
            engine.version = format!("0.0.{index}-preview");
            engine
        })
        .collect();
    hub.create.engine = Some("0.0.0-preview".into());
    hub.view = View::Create;
    app.rebuild(&mut hub, capture::NOW)
        .expect("twelve engine versions");
    app.input
        .focus(&mut app.ui, stable_id("create-engine"))
        .expect("engine selector");
    text(&mut app, &mut hub, "0.0.11-preview");
    let select = app.select("create-engine").expect("retained select");
    assert_eq!(select.query(), "0.0.11-preview");
    assert_eq!(
        select.active().map(|option| &option.value),
        Some(&Intent::CreateEngine("0.0.11-preview".into()))
    );
    assert_eq!(
        hub.create.engine.as_deref(),
        Some("0.0.0-preview"),
        "filtering does not commit a version"
    );
    hub.event(Event::Reply {
        call: Call::Release {
            method: "hub.downloads",
            params: json!({}),
        },
        result: Ok(json!({"jobs":[]})),
    });
    app.rebuild(&mut hub, capture::NOW)
        .expect("background release reply");
    assert_eq!(
        app.select("create-engine").expect("select").query(),
        "0.0.11-preview"
    );
    hub.engines.reverse();
    app.rebuild(&mut hub, capture::NOW)
        .expect("engine order changed");
    assert_eq!(
        app.select("create-engine")
            .and_then(|select| select.active())
            .map(|option| &option.value),
        Some(&Intent::CreateEngine("0.0.11-preview".into()))
    );
    key(&mut app, &mut hub, 40);
    assert_eq!(hub.create.engine.as_deref(), Some("0.0.11-preview"));
    assert_eq!(app.focused(), Some("create-engine"));
}

#[test]
fn long_engine_version_labels_wrap_inside_the_shared_popup() {
    let (mut app, mut hub) = setup();
    let mut engine = hub.engines[0].clone();
    engine.version = format!("0.0.12-preview-{}", "long-channel-description-".repeat(6));
    let version = engine.version.clone();
    hub.engines.push(engine);
    hub.view = View::Create;
    app.rebuild(&mut hub, capture::NOW)
        .expect("long engine version");
    app.input
        .focus(&mut app.ui, stable_id("create-engine"))
        .expect("engine selector");
    key(&mut app, &mut hub, 81);
    key(&mut app, &mut hub, 77);
    app.frame().expect("long row layout");
    let popup = &app.ui.nodes[app
        .ui
        .find("hub-select-create-engine-popover")
        .expect("popup")];
    assert!(popup.rect.w <= 360.5 && popup.rect.x + popup.rect.w <= app.size[0] + 0.5);
    let row = app
        .ui
        .nodes
        .iter()
        .find(|node| {
            node.element.tag == "button" && node.element.attrs.get("label") == Some(&version)
        })
        .expect("long version option");
    assert!(
        row.rect.h > 34.,
        "long option text grows beyond the normal button height"
    );
    assert!(row.rect.intersects(row.clip));
}

#[test]
fn choosing_a_project_engine_opens_confirmation_and_cancel_restores_the_pin_selector() {
    let (mut app, mut hub) = setup();
    hub.view = View::Installs;
    app.rebuild(&mut hub, capture::NOW).expect("engine pins");
    app.input
        .focus(&mut app.ui, stable_id("project-engine-project-3"))
        .expect("project engine selector");
    app.frame().expect("reveal selector");
    let original = hub.projects[2].engine_version.clone();
    key(&mut app, &mut hub, 81);
    key(&mut app, &mut hub, 77);
    key(&mut app, &mut hub, 40);
    assert!(matches!(hub.dialog, Some(Dialog::Confirm { .. })));
    assert_eq!(hub.projects[2].engine_version, original);
    click(&mut app, &mut hub, "dialog-cancel");
    assert_eq!(app.focused(), Some("project-engine-project-3"));
    assert_eq!(
        app.select("project-engine-project-3")
            .and_then(|select| select.selected()),
        Some(&Intent::ChooseEngine {
            project: "project-3".into(),
            version: original
        })
    );
    assert!(hub.take_calls().is_empty());
}

fn setup() -> (App, Hub) {
    let mut hub = Hub::new(Prefs::default(), "target/hub-dialog-tests".into(), None);
    capture::seed(&mut hub);
    let mut app = App::new([1280., 720.], vec![]).expect("native UI");
    app.rebuild(&mut hub, capture::NOW).expect("initial frame");
    (app, hub)
}

fn click(app: &mut App, hub: &mut Hub, id: &str) {
    let rect = app.ui.nodes[app.ui.find(id).expect(id)].rect;
    let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
    for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
        app.dispatch_pointer(hub, event, capture::NOW)
            .expect("pointer input");
    }
}

fn key(app: &mut App, hub: &mut Hub, scancode: i32) {
    let mut intents = app.key(hub, scancode, true).expect("key");
    intents.extend(app.actions());
    for intent in intents {
        hub.intent(intent);
    }
    app.rebuild(hub, capture::NOW).expect("key frame");
}

fn removal_ready(app: &mut App, hub: &mut Hub) {
    let calls = hub.take_calls();
    assert_eq!(
        calls.len(),
        1,
        "one read-only inspection starts when the dialog opens"
    );
    let call = calls.into_iter().next().expect("inspection");
    assert_eq!(call.method(), "hub.engine.uninstall.preflight");
    let version = call.params()["version"]
        .as_str()
        .expect("version")
        .to_owned();
    hub.event(Event::Reply { call, result: Ok(json!({"id":format!("uninstall-preflight-{version}"),"kind":"uninstall-preflight","version":version,"state":"complete","phase":"complete","result":{"version":version,"destination":format!(r"C:\Users\tester\AppData\Local\Ara\Engines\{version}"),"bytes":2_450_000_000_u64,"files":8500}})) });
    app.rebuild(hub, capture::NOW)
        .expect("inspected uninstall scope");
}

#[test]
fn update_now_later_skip_and_restart_are_separate_user_actions() {
    let (mut app, mut hub) = setup();
    hub.releases.jobs.clear();
    app.rebuild(&mut hub, capture::NOW)
        .expect("available update");
    click(&mut app, &mut hub, "update-available");
    assert_eq!(hub.dialog, Some(Dialog::Update));
    assert!(
        hub.take_calls().is_empty(),
        "opening release notes does not download"
    );
    click(&mut app, &mut hub, "dialog-later");
    assert!(hub.dialog.is_none());
    assert!(hub.take_calls().is_empty());
    click(&mut app, &mut hub, "update-available");
    click(&mut app, &mut hub, "skip-update");
    assert_eq!(hub.prefs.skipped_update.as_deref(), Some("0.1.0"));
    assert!(app.ui.find("update-available").is_none());
    hub.view = View::Settings;
    app.rebuild(&mut hub, capture::NOW)
        .expect("settings keeps skipped update discoverable");
    click(&mut app, &mut hub, "review-update");
    click(&mut app, &mut hub, "download-update");
    assert!(
        hub.take_calls()
            .iter()
            .any(|call| call.method() == "hub.update.prepare")
    );
    assert!(hub.take_effects().is_empty(), "Update now does not restart");
}

#[test]
fn engine_uninstall_requires_confirmation_and_cancel_keeps_the_installation() {
    let (mut app, mut hub) = setup();
    hub.view = View::Installs;
    app.rebuild(&mut hub, capture::NOW).expect("installs");
    // Reveal the installed section the same way keyboard navigation reveals it.
    app.input
        .focus(&mut app.ui, stable_id("engine-remove-1"))
        .expect("focus uninstall");
    app.frame().expect("reveal uninstall");
    click(&mut app, &mut hub, "engine-remove-1");
    assert!(matches!(hub.dialog, Some(Dialog::UninstallEngine(_))));
    removal_ready(&mut app, &mut hub);
    assert!(hub.take_calls().is_empty());
    click(&mut app, &mut hub, "dialog-cancel");
    assert!(hub.take_calls().is_empty());
    click(&mut app, &mut hub, "engine-remove-1");
    removal_ready(&mut app, &mut hub);
    click(&mut app, &mut hub, "dialog-confirm");
    assert!(
        hub.take_calls()
            .iter()
            .any(|call| call.method() == "hub.engine.uninstall")
    );
}

#[test]
fn dropdown_keyboard_choices_scroll_and_restore_focus() {
    let (mut app, mut hub) = setup();
    app.input
        .focus(&mut app.ui, stable_id("project-sort"))
        .expect("sort selector");
    key(&mut app, &mut hub, 81); // Down opens choices.
    assert_eq!(
        app.select("project-sort")
            .and_then(|select| select.active())
            .map(|option| &option.value),
        Some(&Intent::Sort(Sort::Recent))
    );
    key(&mut app, &mut hub, 77); // End.
    assert_eq!(
        app.select("project-sort")
            .and_then(|select| select.active())
            .map(|option| &option.value),
        Some(&Intent::Sort(Sort::Name))
    );
    key(&mut app, &mut hub, 74); // Home.
    key(&mut app, &mut hub, 81); // Down.
    key(&mut app, &mut hub, 40); // Enter.
    assert_eq!(hub.prefs.sort, Sort::Name);
    assert_eq!(app.focused(), Some("project-sort"));

    let base = hub.engines[0].clone();
    hub.engines = (0..40)
        .map(|index| {
            let mut engine = base.clone();
            engine.version = format!("0.0.{index}-preview");
            engine
        })
        .collect();
    hub.view = View::Create;
    app.rebuild(&mut hub, capture::NOW).expect("create");
    app.input
        .focus(&mut app.ui, stable_id("create-engine"))
        .expect("engine selector");
    key(&mut app, &mut hub, 81);
    key(&mut app, &mut hub, 77);
    assert_eq!(
        app.select("create-engine")
            .and_then(|select| select.active())
            .map(|option| &option.value),
        Some(&Intent::CreateEngine("0.0.39-preview".into()))
    );
    app.frame().expect("scroll focused option");
    let menu = app.ui.nodes[app
        .ui
        .find("hub-select-create-engine-popover")
        .expect("popup")]
    .rect;
    assert!(
        menu.y >= 0. && menu.y + menu.h <= 720.5,
        "popup fits: {menu:?}"
    );
    let option = app
        .ui
        .nodes
        .iter()
        .find(|node| {
            node.element.tag == "button"
                && node.element.attrs.get("label").map(String::as_str) == Some("0.0.39-preview")
        })
        .expect("last version");
    assert!(
        option.rect.intersects(option.clip),
        "End reveals the last version"
    );
    key(&mut app, &mut hub, 82); // Up.
    assert_eq!(
        app.select("create-engine")
            .and_then(|select| select.active())
            .map(|option| &option.value),
        Some(&Intent::CreateEngine("0.0.38-preview".into()))
    );
    key(&mut app, &mut hub, 41); // Escape restores the selector.
    assert!(
        app.select("create-engine")
            .is_some_and(|select| !select.is_open())
    );
    assert_eq!(app.focused(), Some("create-engine"));
}

#[test]
fn long_release_notes_scroll_without_pushing_actions_below_the_window() {
    let (mut app, mut hub) = setup();
    hub.releases.jobs.clear();
    hub.releases.hub.as_mut().expect("release").notes =
        "A detailed release note with real changes. ".repeat(300);
    hub.intent(Intent::Dialog(Dialog::Update));
    app.rebuild(&mut hub, capture::NOW).expect("long notes");
    for id in [
        "dialog-card",
        "skip-update",
        "dialog-later",
        "download-update",
    ] {
        let rect = app.ui.nodes[app.ui.find(id).expect(id)].rect;
        assert!(
            rect.x >= 0. && rect.y >= 0. && rect.x + rect.w <= 1280.5 && rect.y + rect.h <= 720.5,
            "{id} fits: {rect:?}"
        );
    }
}

fn failure(hub: &mut Hub, call: Call, message: &str) {
    hub.event(Event::Reply {
        call,
        result: Err(ara_diag::Diagnostic::error(ara_update::IO, message).into()),
    });
}

#[test]
fn installation_error_replaces_the_progress_scope_and_retry_reopens_valid_controls() {
    let (mut app, mut hub) = setup();
    hub.releases.jobs.clear();
    hub.view = View::Installs;
    app.rebuild(&mut hub, capture::NOW).expect("installs");
    click(&mut app, &mut hub, "install-0.0.3-alpha");
    assert_eq!(app.focused(), Some("dialog-cancel"));
    click(&mut app, &mut hub, "confirm-install-engine");
    let call = hub
        .take_calls()
        .into_iter()
        .find(|call| call.method() == "hub.engine.install")
        .expect("install request");
    failure(
        &mut hub,
        call,
        "The engine download was interrupted. Check your connection and retry.",
    );
    app.rebuild(&mut hub, capture::NOW)
        .expect("error replaces install dialog");
    assert!(matches!(hub.dialog, Some(Dialog::Error(_))));
    assert_eq!(app.focused(), Some("dialog-error-dismiss"));
    click(&mut app, &mut hub, "dialog-retry-operation");
    assert!(matches!(hub.dialog, Some(Dialog::InstallEngine(_))));
    assert_eq!(app.focused(), Some("dialog-cancel"));
    key(&mut app, &mut hub, 41);
    assert!(hub.dialog.is_none());
    assert_eq!(app.focused(), Some("install-0.0.3-alpha"));
}

#[test]
fn consecutive_errors_keep_valid_default_actions_and_keyboard_dismissal() {
    let (mut app, mut hub) = setup();
    click(&mut app, &mut hub, "update-available");
    failure(
        &mut hub,
        Call::Close {
            project: "project-3".into(),
        },
        "The project could not be closed.",
    );
    app.rebuild(&mut hub, capture::NOW).expect("first error");
    assert_eq!(app.focused(), Some("dialog-error-dismiss"));
    let first = hub.dialog.clone();
    failure(
        &mut hub,
        Call::Release {
            method: "hub.update.prepare",
            params: json!({"version":"0.1.0"}),
        },
        "The update could not be downloaded.",
    );
    app.rebuild(&mut hub, capture::NOW).expect("second error");
    assert_ne!(hub.dialog, first);
    assert_eq!(app.focused(), Some("dialog-error-dismiss"));
    key(&mut app, &mut hub, 40); // Enter acknowledges the newer error.
    assert_eq!(
        hub.dialog, first,
        "the earlier unacknowledged error stays available"
    );
    assert_eq!(app.focused(), Some("dialog-error-dismiss"));
    key(&mut app, &mut hub, 41); // Escape acknowledges the remaining error.
    assert!(hub.dialog.is_none());
    assert_eq!(app.focused(), Some("update-available"));
}

#[test]
fn background_errors_wait_for_the_active_destructive_decision() {
    let (mut app, mut hub) = setup();
    hub.view = View::Installs;
    app.rebuild(&mut hub, capture::NOW).expect("installs");
    app.input
        .focus(&mut app.ui, stable_id("engine-remove-1"))
        .expect("uninstall control");
    app.frame().expect("reveal uninstall");
    click(&mut app, &mut hub, "engine-remove-1");
    assert_eq!(app.focused(), Some("dialog-cancel"));
    let decision = hub.dialog.clone();
    let inspection = hub.take_calls();
    assert!(
        inspection
            .iter()
            .all(|call| call.method() == "hub.engine.uninstall.preflight")
    );
    failure(
        &mut hub,
        Call::Close {
            project: "project-3".into(),
        },
        "A background project could not be closed.",
    );
    app.rebuild(&mut hub, capture::NOW)
        .expect("background error during confirmation");
    assert_eq!(hub.dialog, decision);
    assert_eq!(app.focused(), Some("dialog-cancel"));
    click(&mut app, &mut hub, "dialog-cancel");
    assert!(matches!(hub.dialog, Some(Dialog::Error(_))));
    assert_eq!(app.focused(), Some("dialog-error-dismiss"));
    assert!(
        hub.take_calls().is_empty(),
        "the cancelled uninstall never runs"
    );
    key(&mut app, &mut hub, 40);
    assert!(hub.dialog.is_none());
    assert_eq!(app.focused(), Some("engine-remove-1"));
}

#[test]
fn transfer_progress_preserves_focus_and_failures_choose_a_new_safe_action() {
    for (kind, version, method, dialog, cancel, restored) in [
        (
            "hub",
            "0.1.0",
            "hub.update.prepare",
            Dialog::Update,
            "cancel-hub-update",
            "dialog-later",
        ),
        (
            "engine",
            "0.0.3-alpha",
            "hub.engine.install",
            Dialog::InstallEngine("0.0.3-alpha".into()),
            "cancel-engine-install",
            "dialog-cancel",
        ),
    ] {
        let (mut app, mut hub) = setup();
        hub.releases.jobs.clear();
        hub.intent(Intent::Dialog(dialog));
        app.rebuild(&mut hub, capture::NOW)
            .expect("transfer decision");
        let job = json!({"id":format!("{kind}-{version}"),"kind":kind,"version":version,"state":"running","phase":"downloading","progress":{"received":100,"total":1000}});
        hub.event(Event::Reply {
            call: Call::Release {
                method,
                params: json!({"version":version}),
            },
            result: Ok(job.clone()),
        });
        app.rebuild(&mut hub, capture::NOW)
            .expect("started transfer");
        assert_eq!(
            app.focused(),
            Some("dialog-later"),
            "a removed Install button gets a safe replacement"
        );
        app.input
            .focus(&mut app.ui, stable_id(cancel))
            .expect("cancel transfer");
        let mut progress = job.clone();
        progress["progress"]["received"] = json!(400);
        hub.event(Event::Reply {
            call: Call::Release {
                method: "hub.downloads",
                params: json!({}),
            },
            result: Ok(json!({"jobs":[progress]})),
        });
        app.rebuild(&mut hub, capture::NOW)
            .expect("progress refresh");
        assert_eq!(
            app.focused(),
            Some(cancel),
            "ordinary progress preserves focus"
        );
        let mut failed = job;
        failed["state"] = json!("failed");
        failed["error"] = serde_json::to_value(ara_diag::Diagnostic::error(
            ara_update::IO,
            "The transfer failed; check your connection and retry.",
        ))
        .expect("error");
        hub.event(Event::Reply {
            call: Call::Release {
                method: "hub.downloads",
                params: json!({}),
            },
            result: Ok(json!({"jobs":[failed]})),
        });
        app.rebuild(&mut hub, capture::NOW)
            .expect("error replaces progress");
        assert_eq!(app.focused(), Some("dialog-error-dismiss"));
        click(&mut app, &mut hub, "dialog-retry");
        assert_eq!(app.focused(), Some(restored));
        key(&mut app, &mut hub, 41);
        assert!(hub.dialog.is_none());
    }
}

#[test]
fn technical_release_settings_are_disclosed_and_repair_keeps_project_pins() {
    let (mut app, mut hub) = setup();
    hub.view = View::Settings;
    app.rebuild(&mut hub, capture::NOW).expect("settings");
    click(&mut app, &mut hub, "hub-update-downloads");
    key(&mut app, &mut hub, 77);
    key(&mut app, &mut hub, 40);
    assert_eq!(
        hub.prefs.hub_update_downloads,
        ara_hub::model::DownloadPolicy::Background
    );
    assert!(
        hub.take_effects().is_empty(),
        "a download preference never applies an update"
    );
    assert!(
        app.ui.find("release-repo").is_none(),
        "source overrides stay out of normal settings"
    );
    click(&mut app, &mut hub, "release-advanced");
    assert!(app.ui.find("release-repo").is_some());
    click(&mut app, &mut hub, "release-advanced");
    hub.view = View::Installs;
    app.rebuild(&mut hub, capture::NOW).expect("installs");
    app.input
        .focus(&mut app.ui, stable_id("engine-repair-1"))
        .expect("repair control");
    app.frame().expect("reveal repair");
    let pins: Vec<_> = hub
        .projects
        .iter()
        .map(|project| project.engine_version.clone())
        .collect();
    click(&mut app, &mut hub, "engine-repair-1");
    assert!(matches!(hub.dialog, Some(Dialog::RepairEngine(_))));
    assert_eq!(app.focused(), Some("dialog-cancel"));
    click(&mut app, &mut hub, "confirm-install-engine");
    assert_eq!(
        hub.take_calls()
            .iter()
            .filter(|call| call.method() == "hub.engine.repair")
            .count(),
        1
    );
    assert_eq!(
        pins,
        hub.projects
            .iter()
            .map(|project| project.engine_version.clone())
            .collect::<Vec<_>>()
    );
    assert!(hub.take_effects().is_empty());
    click(&mut app, &mut hub, "cancel-pending-transfer");
    assert!(hub.releases.cancelling("repair", "0.0.2-nightly"));
}

#[test]
fn install_preflight_never_downloads_and_cancelled_summary_cannot_reopen_an_error() {
    let (mut app, mut hub) = setup();
    hub.releases.jobs.clear();
    hub.releases.plans.clear();
    hub.view = View::Installs;
    app.rebuild(&mut hub, capture::NOW).expect("installs");
    click(&mut app, &mut hub, "install-0.0.3-alpha");
    let calls = hub.take_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method(), "hub.engine.preflight");
    click(&mut app, &mut hub, "confirm-install-engine");
    assert!(
        hub.take_calls().is_empty(),
        "Install waits for the destination/space summary"
    );
    click(&mut app, &mut hub, "dialog-cancel");
    failure(
        &mut hub,
        calls.into_iter().next().expect("preflight"),
        "The installation drive could not be checked.",
    );
    app.rebuild(&mut hub, capture::NOW)
        .expect("late preflight failure");
    assert!(
        hub.dialog.is_none(),
        "closing a read-only summary cancels its error presentation"
    );
    assert_eq!(app.focused(), Some("install-0.0.3-alpha"));
}
