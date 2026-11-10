//! The decisions people make in the Hub, through the real view on a headless stillkit
//! runtime: clicks and keys on keyed controls, checked against the model and the service
//! calls they queue.
use ara_hub::{
    capture::{self, Capture},
    connection::Event,
    model::{Call, Dialog, Hub, Intent, Prefs, State, View},
};
use serde_json::json;
use stillkit::el::Modifiers;

fn setup() -> (Capture, Hub) {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    hub.take_calls();
    (Capture::new([1440, 900], 1.0).expect("headless hub"), hub)
}

const ENTER: u16 = 0x0D;
const ESCAPE: u16 = 0x1B;
const F2: u16 = 0x71;

#[test]
fn every_page_lays_out_in_both_themes_and_sizes() {
    for size in [[1440, 900], [1280, 720], [960, 600]] {
        for light in [false, true] {
            let mut hub = Hub::new(
                Prefs {
                    light,
                    ..Default::default()
                },
                std::env::temp_dir(),
                None,
            );
            capture::seed(&mut hub);
            let mut cap = Capture::new(size, 1.0).expect("headless hub");
            for view in View::ALL {
                hub.view = view;
                assert!(
                    cap.shows(&mut hub, ara_kit::ui::TITLE_BAR),
                    "{view:?} has the title bar"
                );
                if view != View::Create {
                    assert!(
                        cap.shows(&mut hub, "sidebar"),
                        "{view:?} at {size:?} has the sidebar"
                    );
                }
            }
            let png = std::env::temp_dir().join(format!("hub-layout-{}-{light}.png", size[0]));
            cap.save(&mut hub, &png).expect("renders");
        }
    }
}

#[test]
fn a_projects_primary_action_follows_its_state() {
    let (mut cap, mut hub) = setup();
    // Outpost is open in the editor: its one action closes it.
    assert_eq!(hub.state("project-1"), State::Editor);
    cap.click(&mut hub, "open-project-1")
        .expect("Outpost's action");
    assert!(
        hub.take_calls()
            .iter()
            .any(|c| c.method() == "hub.close" && c.params()["project"] == "project-1")
    );
    // Courier Run is closed: Open opens it in the editor.
    cap.click(&mut hub, "open-project-3")
        .expect("Courier Run's action");
    assert!(
        hub.take_calls()
            .iter()
            .any(|c| c.method() == "hub.open" && c.params()["project"] == "project-3")
    );
}

#[test]
fn the_menu_opens_from_the_ellipsis_and_runs_one_action() {
    let (mut cap, mut hub) = setup();
    cap.click(&mut hub, "more-project-3").expect("⋯");
    assert_eq!(hub.menu.as_deref(), Some("project-3"));
    cap.click(&mut hub, "menu-headless").expect("Open headless");
    assert!(hub.menu.is_none(), "choosing closes the menu");
    let calls = hub.take_calls();
    assert!(
        calls
            .iter()
            .any(|c| c.method() == "hub.open" && c.params()["headless"] == true),
        "{calls:?}"
    );
}

#[test]
fn rename_edits_in_place_and_enter_saves_escape_drops() {
    let (mut cap, mut hub) = setup();
    cap.focus(&mut hub, "project-3");
    cap.key(&mut hub, F2, Modifiers::NONE);
    assert!(
        hub.rename.as_ref().is_some_and(|(id, _)| id == "project-3"),
        "F2 renames the focused project"
    );
    cap.key(&mut hub, b'A' as u16, Modifiers::CTRL);
    cap.typing(&mut hub, "Courier Run II");
    assert_eq!(
        hub.rename.as_ref().map(|(_, draft)| draft.as_str()),
        Some("Courier Run II")
    );
    cap.key(&mut hub, ENTER, Modifiers::NONE);
    assert!(hub.rename.is_none());
    assert!(
        hub.take_calls()
            .iter()
            .any(|c| c.method() == "hub.update" && c.params()["name"] == "Courier Run II")
    );
    hub.intent(Intent::Rename("project-5".into()));
    cap.focus(&mut hub, "rename-field");
    cap.key(&mut hub, ESCAPE, Modifiers::NONE);
    assert!(hub.rename.is_none(), "Escape drops the draft");
    assert!(!hub.take_calls().iter().any(|c| c.method() == "hub.update"));
}

#[test]
fn search_filters_and_clearing_restores_every_project() {
    let (mut cap, mut hub) = setup();
    cap.key(&mut hub, 0xBF, Modifiers::NONE); // "/" focuses search
    cap.typing(&mut hub, "night");
    assert_eq!(hub.search, "night");
    assert_eq!(hub.visible().len(), 1);
    assert!(!cap.shows(&mut hub, "project-1") && cap.shows(&mut hub, "project-4"));
    cap.click(&mut hub, "search/clear").expect("clear");
    assert!(hub.search.is_empty() && hub.visible().len() == 5);
}

#[test]
fn new_project_is_a_full_page_that_back_and_escape_leave() {
    let (mut cap, mut hub) = setup();
    cap.key(&mut hub, b'N' as u16, Modifiers::CTRL);
    assert_eq!(hub.view, View::Create);
    assert!(!cap.shows(&mut hub, "sidebar"), "creation takes the window");
    cap.key(&mut hub, ESCAPE, Modifiers::NONE);
    assert_eq!(hub.view, View::Projects);
    cap.click(&mut hub, "new-project").expect("New project");
    cap.click(&mut hub, "create-back").expect("Back");
    assert_eq!(hub.view, View::Projects);
}

#[test]
fn removal_is_one_click_with_undo_in_a_toast() {
    let (mut cap, mut hub) = setup();
    cap.click(&mut hub, "more-project-5").expect("⋯");
    cap.click(&mut hub, "menu-remove").expect("Remove");
    let removed = hub
        .take_calls()
        .into_iter()
        .find(|c| c.method() == "hub.remove")
        .expect("removes");
    hub.event(Event::Reply {
        call: removed,
        result: Ok(json!({})),
    });
    let undo = hub
        .notices
        .iter()
        .find(|n| n.undo.is_some())
        .map(|n| n.id)
        .expect("an undo notice");
    cap.click(&mut hub, &format!("undo-{undo}"))
        .expect("Undo in the toast");
    assert!(
        hub.take_calls()
            .iter()
            .any(|c| c.method() == "hub.register"),
        "Undo registers it again"
    );
}

#[test]
fn dialogs_confirm_cancel_and_escape() {
    let (mut cap, mut hub) = setup();
    hub.view = View::Installs;
    let engine = hub.engines[0].clone();
    hub.intent(Intent::Dialog(Dialog::Confirm {
        title: "Remove Ara 0.0.1-alpha from Hub?".into(),
        detail: "The files stay on disk.".into(),
        label: "Remove build".into(),
        destructive: false,
        action: Box::new(Intent::RemoveEngineBuild(engine)),
    }));
    assert!(cap.shows(&mut hub, "hub-dialog"));
    cap.key(&mut hub, ESCAPE, Modifiers::NONE);
    assert!(hub.dialog.is_none(), "Escape dismisses");
    assert!(
        !hub.take_calls()
            .iter()
            .any(|c| c.method() == "hub.engine.remove")
    );
    hub.intent(Intent::Dialog(Dialog::Update));
    cap.click(&mut hub, "dialog-later")
        .or_else(|_| cap.click(&mut hub, "dialog-cancel"))
        .expect("a safe action");
    assert!(hub.dialog.is_none());
}

#[test]
fn the_palette_finds_and_runs_with_the_keyboard() {
    let (mut cap, mut hub) = setup();
    cap.key(&mut hub, b'K' as u16, Modifiers::CTRL);
    assert!(cap.shows(&mut hub, "palette"));
    cap.typing(&mut hub, "courier");
    cap.key(&mut hub, ENTER, Modifiers::NONE);
    assert!(!cap.shows(&mut hub, "palette"), "running closes it");
    assert!(
        hub.take_calls()
            .iter()
            .any(|c| c.method() == "hub.open" && c.params()["project"] == "project-3")
    );
    cap.click(&mut hub, "launcher")
        .expect("the title bar launcher");
    cap.typing(&mut hub, "installs");
    cap.key(&mut hub, ENTER, Modifiers::NONE);
    assert_eq!(hub.view, View::Installs);
}

#[test]
fn the_theme_toggle_switches_scheme_and_back() {
    let (mut cap, mut hub) = setup();
    cap.click(&mut hub, "theme").expect("theme toggle");
    assert!(hub.prefs.light);
    cap.click(&mut hub, "theme").expect("theme toggle");
    assert!(!hub.prefs.light);
}

#[test]
fn a_slow_service_never_blocks_input() {
    let (mut cap, mut hub) = setup();
    cap.click(&mut hub, "open-project-3").expect("Open");
    let pending: Vec<Call> = hub.take_calls();
    assert!(!pending.is_empty(), "the open is in flight, unanswered");
    cap.click(&mut hub, "nav-installs")
        .expect("navigation still works");
    assert_eq!(hub.view, View::Installs);
    cap.click(&mut hub, "theme").expect("and the theme");
    assert!(hub.prefs.light);
}

#[test]
fn an_idle_window_draws_nothing() {
    let (mut cap, mut hub) = setup();
    cap.settle(&mut hub);
    let rt = cap.runtime();
    // The harness swaps the fixture back out after settling; that one frame lands first.
    rt.frame(std::time::Duration::from_millis(16));
    let runs = rt.view_runs();
    for _ in 0..10 {
        assert!(
            !rt.frame(std::time::Duration::from_millis(16)),
            "no frame paints while idle"
        );
    }
    assert!(
        !rt.animating() && rt.view_runs() == runs,
        "no animation or view run while idle"
    );
}
