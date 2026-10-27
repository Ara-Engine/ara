//! Dialog decisions keep their title, close affordance and destructive scope visible.
use ara_hub::{
    capture,
    model::{Dialog, Hub, Prefs},
    native::App,
};
use ara_ui::{input::InputEvent, paint::Box2};

fn contains(outer: Box2, inner: Box2) -> bool {
    inner.x >= outer.x - 0.5
        && inner.y >= outer.y - 0.5
        && inner.x + inner.w <= outer.x + outer.w + 0.5
        && inner.y + inner.h <= outer.y + outer.h + 0.5
}

#[test]
fn headers_keep_an_icon_only_accessible_close_inside_long_title_layouts() {
    for light in [false, true] {
        let mut hub = Hub::new(
            Prefs {
                light,
                ..Default::default()
            },
            "target/hub-header-tests".into(),
            None,
        );
        capture::seed(&mut hub);
        hub.releases.jobs.clear();
        let mut app = App::new([1280., 720.], vec![]).expect("UI");
        for (dialog, long) in [
            (Dialog::Update, false),
            (Dialog::Update, true),
            (
                Dialog::InstallEngine(format!("0.0.3-preview-{}", "long-version-label-".repeat(7))),
                true,
            ),
        ] {
            if long {
                hub.releases.hub.as_mut().expect("update").version =
                    format!("0.1.0-preview-{}", "long-version-label-".repeat(7));
            }
            hub.dialog = Some(dialog);
            app.rebuild(&mut hub, capture::NOW).expect("dialog layout");
            let close = &app.ui.nodes[app.ui.find("dialog-close").expect("close")];
            let header = &app.ui.nodes[close.element.parent.expect("header")];
            let card = &app.ui.nodes[app.ui.find("dialog-card").expect("dialog card")];
            let title = header
                .element
                .children
                .iter()
                .map(|index| &app.ui.nodes[*index])
                .find(|node| node.element.tag == "text")
                .expect("title");
            assert_eq!(
                close.element.attrs.get("label").map(String::as_str),
                Some("Close dialog")
            );
            assert!(
                close.text.is_none(),
                "the accessible name must not become painted button text"
            );
            assert_eq!(close.element.children.len(), 1);
            assert_eq!(app.ui.nodes[close.element.children[0]].element.tag, "image");
            assert!(close.rect.w <= 32.5 && close.rect.h <= 32.5);
            assert!(
                contains(card.rect, header.rect)
                    && contains(header.rect, close.rect)
                    && contains(header.rect, title.rect),
                "header {:?}, title {:?}, close {:?}, card {:?}",
                header.rect,
                title.rect,
                close.rect,
                card.rect
            );
            assert!(
                title.rect.x + title.rect.w <= close.rect.x + 0.5,
                "title cannot push over the close affordance"
            );
            if long {
                assert!(
                    title.rect.h > 24.,
                    "long titles wrap within the available header width"
                );
            }
        }
        let focused = app.focused().map(str::to_owned);
        let rect = app.ui.nodes[app.ui.find("dialog-close").expect("close")].rect;
        app.dispatch_pointer(
            &mut hub,
            InputEvent::PointerMove([rect.x + rect.w / 2., rect.y + rect.h / 2.]),
            capture::NOW,
        )
        .expect("hover close");
        let tooltip = &app.ui.nodes[app.ui.find("dialog-close-tooltip").expect("close hint")];
        assert!(tooltip.rect.w < 160., "the short hint fits its content");
        assert_eq!(
            app.focused(),
            focused.as_deref(),
            "the hint never steals focus"
        );
        app.dispatch_pointer(&mut hub, InputEvent::PointerMove([-1., -1.]), capture::NOW)
            .expect("leave close");
        assert!(app.ui.find("dialog-close-tooltip").is_none());
    }
}

#[test]
fn managed_removal_always_paints_its_destination_on_any_capture_host() {
    let mut hub = Hub::new(Prefs::default(), "target/hub-header-tests".into(), None);
    capture::seed(&mut hub);
    let engine = hub
        .engines
        .iter()
        .find(|engine| engine.managed)
        .expect("managed fixture")
        .clone();
    hub.dialog = Some(Dialog::UninstallEngine(engine));
    let mut app = App::new([1280., 720.], vec![]).expect("UI");
    app.rebuild(&mut hub, capture::NOW).expect("removal dialog");
    let destination = app
        .ui
        .nodes
        .iter()
        .find(|node| {
            node.element.attrs.get("text").map(String::as_str)
                == Some(r"C:\Users\tester\AppData\Local\Ara\Engines\0.0.2-nightly")
        })
        .expect("the exact installation directory must be visible");
    assert!(destination.rect.h > 0. && destination.rect.intersects(destination.clip));
}
