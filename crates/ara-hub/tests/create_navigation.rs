//! Creation is a separate page; returning keeps the launcher where the user left it.
use ara_hub::{
    capture,
    connection::Event,
    model::{Busy, Call, Hub, Intent, Layout, Prefs, Sort, View},
    native::App,
};
use ara_ui::{input::InputEvent, source::stable_id, widgets::Widget};
use serde_json::json;

fn setup() -> (App, Hub) {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    let mut app = App::new([1280., 720.], Vec::new()).expect("native UI");
    app.rebuild(&mut hub, capture::NOW).expect("first page");
    (app, hub)
}

fn click(app: &mut App, hub: &mut Hub, id: &str) {
    let rect = app.ui.nodes[app.ui.find(id).expect(id)].rect;
    assert!(rect.x >= 0. && rect.y >= 0. && rect.x + rect.w <= app.size[0] + 0.5);
    let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
    for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
        app.dispatch_pointer(hub, event, capture::NOW)
            .expect("pointer input");
    }
}

fn key(app: &mut App, hub: &mut Hub, scancode: i32) {
    let mut intents = app.key(hub, scancode, true).expect("native key");
    intents.extend(app.actions());
    for intent in intents {
        hub.intent(intent);
    }
    app.rebuild(hub, capture::NOW).expect("key frame");
}

fn scroll(app: &App) -> [f32; 2] {
    match app.ui.widgets.get(&stable_id("scroll-projects")) {
        Some(Widget::Scroll(position)) => *position,
        _ => panic!("project scroll state"),
    }
}

#[test]
fn back_returns_to_the_origin_and_keeps_project_preferences_and_query() {
    let (_, mut hub) = setup();
    hub.intent(Intent::Search("cour".into()));
    hub.intent(Intent::Sort(Sort::Name));
    hub.intent(Intent::Layout(Layout::List));
    for origin in View::SIDEBAR {
        hub.intent(Intent::Navigate(origin));
        hub.intent(Intent::Navigate(View::Create));
        hub.intent(Intent::Navigate(View::Create));
        hub.intent(Intent::Back);
        assert_eq!(hub.view, origin, "reopening Create must keep its origin");
        assert_eq!(hub.search, "cour");
        assert_eq!(hub.prefs.sort, Sort::Name);
        assert_eq!(hub.prefs.layout, Layout::List);
    }
    hub.intent(Intent::Navigate(View::Learn));
    hub.intent(Intent::Template("blank".into()));
    hub.intent(Intent::Back);
    assert_eq!(
        hub.view,
        View::Learn,
        "template selection remembers its origin"
    );
}

#[test]
fn button_escape_and_mouse_back_restore_projects_scroll_and_search() {
    let (mut app, mut hub) = setup();
    let prototype = hub.projects[2].clone();
    hub.projects = (0..30)
        .map(|index| {
            let mut project = prototype.clone();
            project.id = format!("fixture-{index}");
            project.name = format!("Project {index}");
            project
        })
        .collect();
    hub.intent(Intent::Layout(Layout::List));
    app.rebuild(&mut hub, capture::NOW)
        .expect("long project list");
    click(&mut app, &mut hub, "search");
    let mut composing = false;
    assert!(app.edit(
        &mut composing,
        ara_ui_text::EditEvent::Insert("project".into()),
        &mut TestClipboard
    ));
    for intent in app.actions() {
        hub.intent(intent);
    }
    app.rebuild(&mut hub, capture::NOW).expect("search results");
    app.dispatch_pointer(
        &mut hub,
        InputEvent::Wheel {
            position: [800., 550.],
            delta: [0., 340.],
        },
        capture::NOW,
    )
    .expect("native wheel");
    app.frame().expect("scrolled frame");
    let position = scroll(&app);
    assert!(position[1] > 0., "the test must leave the top of Projects");

    for action in 0..3 {
        click(&mut app, &mut hub, "new-project");
        assert_eq!(hub.view, View::Create);
        assert!(app.ui.find("nav-projects").is_none());
        assert!(app.ui.find("create-back").is_some());
        match action {
            0 => click(&mut app, &mut hub, "create-back"),
            1 => key(&mut app, &mut hub, 41),
            _ => {
                for intent in app.back(&hub).expect("mouse Back") {
                    hub.intent(intent);
                }
                app.rebuild(&mut hub, capture::NOW)
                    .expect("mouse Back page");
            }
        }
        app.frame().expect("returned frame");
        assert_eq!(hub.view, View::Projects);
        assert_eq!(hub.search, "project");
        assert_eq!(
            scroll(&app),
            position,
            "Back restores the previous viewport"
        );
        assert_eq!(app.focused(), Some("new-project"));
    }
}

#[test]
fn engine_select_escape_closes_choices_before_leaving_creation() {
    let (mut app, mut hub) = setup();
    click(&mut app, &mut hub, "new-project");
    assert_eq!(
        app.ui.nodes[app.ui.find("create-engine").expect("engine selector")]
            .element
            .tag,
        "dropdown"
    );
    assert!(hub.engines.len() >= 2);
    click(&mut app, &mut hub, "create-engine");
    assert!(app.select("create-engine").expect("kit Select").is_open());
    key(&mut app, &mut hub, 41);
    assert_eq!(hub.view, View::Create);
    assert!(!app.select("create-engine").expect("kit Select").is_open());
    assert_eq!(app.focused(), Some("create-engine"));
    key(&mut app, &mut hub, 41);
    assert_eq!(hub.view, View::Projects);
}

#[test]
fn successful_creation_opens_the_created_project_through_the_existing_rpc_lane() {
    let (_, mut hub) = setup();
    hub.intent(Intent::Navigate(View::Create));
    hub.intent(Intent::CreateDestination(
        std::env::temp_dir().to_string_lossy().into_owned(),
    ));
    loop {
        let checks = hub.take_calls();
        if checks.is_empty() {
            break;
        }
        for call in checks {
            assert!(matches!(call, Call::Create { dry_run: true, .. }));
            hub.event(Event::Reply {
                call,
                result: Ok(json!({"valid":true})),
            });
        }
    }
    hub.intent(Intent::CreateSubmit);
    let calls = hub.take_calls();
    let create = calls.into_iter().next().expect("explicit create request");
    assert!(matches!(create, Call::Create { dry_run: false, .. }));
    hub.event(Event::Reply { call:create, result:Ok(json!({"id":"created-fixture","name":"New game","path":std::env::temp_dir().join("created-fixture"),"engineVersion":"0.0.1-alpha"})) });
    assert_eq!(hub.view, View::Projects);
    assert_eq!(hub.highlight.as_deref(), Some("created-fixture"));
    assert_eq!(hub.busy.get("created-fixture"), Some(&Busy::Opening));
    assert_eq!(
        hub.take_calls()
            .into_iter()
            .filter(|call| matches!(call, Call::Open { .. }))
            .collect::<Vec<_>>(),
        vec![Call::Open {
            project: "created-fixture".into(),
            headless: false
        }]
    );
    assert!(
        hub.take_effects().is_empty(),
        "the model never launches a process directly"
    );
}

struct TestClipboard;
impl ara_ui_text::Clipboard for TestClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
