//! Every page parses, styles (theme tokens only) and lays out in both themes at
//! both capture sizes, with the capture fixture's data. No window or GPU.
use ara_hub::{
    capture,
    model::{Hub, Intent, Layout, Prefs, View},
    view::{self, Ctx},
};
use ara_ui::{Ui, theme::EditorTheme};

fn render(hub: &Hub, size: [f32; 2], light: bool) -> Ui {
    let mut text = view::text_system().expect("fonts");
    let ctx = Ctx {
        now_ms: capture::NOW,
        size,
        thumbnail: &|_| None,
        anchor: Some([1000., 300., 32., 32.]),
        docs: &[],
    };
    let page = view::page(hub, &ctx);
    let mut ui = Ui::editor(&page.markup, view::CSS, size).expect("markup and stylesheet");
    let theme = if light {
        EditorTheme::Light
    } else {
        EditorTheme::Dark
    };
    ui.switch_theme(theme).expect("theme");
    ui.frame(&mut text).expect("layout");
    ui
}

#[test]
fn every_view_lays_out_in_both_themes_and_sizes() {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    for size in [[1440., 900.], [1280., 720.]] {
        for light in [false, true] {
            for view in View::ALL {
                hub.intent(Intent::Navigate(view));
                hub.take_calls();
                let ui = render(&hub, size, light);
                let title = ui
                    .nodes
                    .iter()
                    .any(|n| n.element.attrs.get("text").map(String::as_str) == Some(view.title()));
                assert!(title, "{view:?} shows its title");
                assert!(ui.find(&format!("nav-{view:?}").to_lowercase()).is_some());
            }
            hub.intent(Intent::Navigate(View::Projects));
            hub.intent(Intent::Layout(Layout::List));
            render(&hub, size, light);
            hub.intent(Intent::Menu(Some("project-3".into())));
            let ui = render(&hub, size, light);
            assert!(ui.find("menu-remove").is_some(), "overflow menu");
            hub.intent(Intent::Menu(None));
            hub.intent(Intent::Layout(Layout::Grid));
        }
    }
}

#[test]
fn project_cards_show_state_badges_and_lifecycle_actions() {
    let mut hub = Hub::new(Prefs::default(), std::env::temp_dir(), None);
    capture::seed(&mut hub);
    let ui = render(&hub, [1440., 900.], false);
    let has_text = |text: &str| {
        ui.nodes
            .iter()
            .any(|n| n.element.attrs.get("text").map(String::as_str) == Some(text))
    };
    assert!(has_text("Open in editor"), "editor badge");
    assert!(has_text("Running headless"), "headless badge");
    assert!(
        ui.find("close-project-1").is_some(),
        "close an open project"
    );
    assert!(ui.find("open-project-3").is_some(), "open a closed project");
    // One action per card; headless is in the card's ⋯ menu.
    assert!(ui.find("headless-project-3").is_none(), "no second button");
    hub.menu = Some("project-3".into());
    let ui = render(&hub, [1440., 900.], false);
    assert!(ui.find("menu-headless").is_some(), "open headless");
}
