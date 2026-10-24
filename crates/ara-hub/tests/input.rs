//! Pointer and keyboard input through the window's retained `App` (no GPU): a
//! click raises the bound intent, and typing in search survives the rebuilds
//! each keystroke causes.
use ara_hub::{
    capture,
    model::{Hub, Intent, Prefs, State},
    native::App,
};
use ara_ui::input::InputEvent;
use ara_ui_text::{Clipboard, EditEvent};

/// Tests never touch the OS clipboard.
struct NoClipboard;
impl Clipboard for NoClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

fn click(app: &mut App, id: &str) -> Vec<Intent> {
    let index = app
        .ui
        .find(id)
        .unwrap_or_else(|| panic!("{id} is on screen"));
    let r = app.ui.nodes[index].rect;
    let at = [r.x + r.w / 2., r.y + r.h / 2.];
    app.pointer(InputEvent::PointerMove(at)).expect("move");
    app.pointer(InputEvent::PointerDown(at)).expect("down");
    app.pointer(InputEvent::PointerUp(at)).expect("up");
    app.actions()
}

fn setup() -> (App, Hub) {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    let mut app = App::new([1440., 900.], Vec::new()).expect("app");
    app.rebuild(&mut hub, capture::NOW).expect("rebuild");
    (app, hub)
}

#[test]
fn clicks_raise_the_bound_intents() {
    let (mut app, mut hub) = setup();
    assert_eq!(
        click(&mut app, "open-project-3"),
        [Intent::Open("project-3".into())]
    );
    assert_eq!(
        click(&mut app, "close-project-1"),
        [Intent::Close("project-1".into())]
    );
    // The overflow menu anchors under its button and its items work.
    for intent in click(&mut app, "more-project-4") {
        hub.intent(intent);
    }
    app.rebuild(&mut hub, capture::NOW).expect("menu");
    assert_eq!(
        click(&mut app, "menu-headless"),
        [Intent::OpenHeadless("project-4".into())]
    );
    // Lifecycle in flight: the card shows progress and its buttons disable.
    hub.intent(Intent::OpenHeadless("project-4".into()));
    app.rebuild(&mut hub, capture::NOW).expect("busy");
    assert!(
        click(&mut app, "open-project-4").is_empty(),
        "disabled while starting"
    );
    assert_eq!(hub.state("project-4"), State::Closed);
}

#[test]
fn typing_in_search_keeps_focus_and_caret_across_rebuilds() {
    let (mut app, mut hub) = setup();
    click(&mut app, "search");
    let mut clipboard = NoClipboard;
    for letter in ["c", "o", "u"] {
        let event = InputEvent::Edit(EditEvent::Insert(letter.into()));
        app.input
            .event(&mut app.ui, event, &mut clipboard)
            .expect("type");
        for intent in app.actions() {
            hub.intent(intent);
        }
        app.rebuild(&mut hub, capture::NOW).expect("rebuild");
    }
    assert_eq!(hub.search, "cou");
    assert_eq!(hub.visible().len(), 1, "only Courier Run matches");
    let field = app.ui.find("search").expect("search field");
    assert!(app.ui.nodes[field].state.focus, "focus survives rebuilds");
    let event = InputEvent::Edit(EditEvent::Backspace);
    app.input
        .event(&mut app.ui, event, &mut clipboard)
        .expect("backspace");
    assert_eq!(app.actions(), [Intent::Search("co".into())]);
}

#[test]
fn model_echo_keeps_mid_string_caret_and_undo_history() {
    let (mut app, mut hub) = setup();
    hub.intent(Intent::Search("cour".into()));
    app.rebuild(&mut hub, capture::NOW).expect("initial text");
    click(&mut app, "search");
    let mut clipboard = NoClipboard;
    for _ in 0..2 {
        app.input
            .event(
                &mut app.ui,
                InputEvent::Edit(EditEvent::Move {
                    forward: false,
                    select: false,
                }),
                &mut clipboard,
            )
            .expect("move caret");
    }
    app.input
        .event(
            &mut app.ui,
            InputEvent::Edit(EditEvent::Insert("x".into())),
            &mut clipboard,
        )
        .expect("insert in middle");
    for intent in app.actions() {
        hub.intent(intent);
    }
    assert_eq!(hub.search, "coxur");
    // The model echoes the edited value into markup; that is not a new external value.
    app.rebuild(&mut hub, capture::NOW).expect("model echo");
    app.input
        .event(
            &mut app.ui,
            InputEvent::Edit(EditEvent::Undo),
            &mut clipboard,
        )
        .expect("undo after echo");
    assert_eq!(app.actions(), [Intent::Search("cour".into())]);
    app.input
        .event(
            &mut app.ui,
            InputEvent::Edit(EditEvent::Insert("i".into())),
            &mut clipboard,
        )
        .expect("caret restored by undo");
    assert_eq!(app.actions(), [Intent::Search("coiur".into())]);
}
