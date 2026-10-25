//! Actual input injection and GPU readback, without a native window or service.
use super::*;
use anyhow::Context as _;
use ara_ui::input::InputEvent;

fn click(app: &mut crate::native::App, hub: &mut Hub, id: &str) -> anyhow::Result<()> {
    let index = app
        .ui
        .find(id)
        .ok_or_else(|| anyhow::anyhow!("capture control {id} is missing"))?;
    let rect = app.ui.nodes[index].rect;
    let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
    for event in [
        InputEvent::PointerMove(at),
        InputEvent::PointerDown(at),
        InputEvent::PointerUp(at),
    ] {
        app.dispatch_pointer(hub, event, NOW)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    anyhow::ensure!(
        app.frame().map_err(|e| anyhow::anyhow!("{e}"))?,
        "{id} did not request presentation"
    );
    Ok(())
}

fn save(
    host: &mut Offscreen,
    app: &crate::native::App,
    folder: &Path,
    name: &str,
) -> anyhow::Result<()> {
    let path = folder.join(format!("{name}.png"));
    host.capture(app)?.save(&path)?;
    println!("{}", path.display());
    Ok(())
}

pub(super) fn run(
    host: &mut Offscreen,
    app: &mut crate::native::App,
    folder: &Path,
    light: bool,
) -> anyhow::Result<()> {
    let theme = if light { "light" } else { "dark" };
    let mut hub = Hub::new(
        crate::model::Prefs {
            light,
            ..Default::default()
        },
        "target/hub-captures".into(),
        None,
    );
    seed(&mut hub);
    settle(host, app, &mut hub)?;
    click(app, &mut hub, "more-project-3")?;
    anyhow::ensure!(
        app.ui.find("menu-rename").is_some(),
        "menu opens on first click"
    );
    save(host, app, folder, &format!("{theme}-click-menu"))?;
    click(app, &mut hub, "menu-rename")?;
    anyhow::ensure!(hub.rename.is_some(), "first menu-item click starts rename");
    save(host, app, folder, &format!("{theme}-click-rename"))?;
    // SDL TEXT_INPUT commits use this same windowless delivery path.
    let mut composing = false;
    let mut clipboard = crate::native::OsClipboard;
    anyhow::ensure!(
        crate::native::deliver_edit(
            &mut composing,
            &mut app.ui,
            &mut app.input,
            ara_ui_text::EditEvent::ImeCommit("Preview 3 typing".into()),
            &mut clipboard,
        ),
        "ordinary typing must reach the field without an active composition"
    );
    for intent in app.actions() {
        hub.intent(intent);
    }
    app.rebuild(&mut hub, NOW)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    anyhow::ensure!(
        app.frame().map_err(|e| anyhow::anyhow!("{e}"))?,
        "typed text presents"
    );
    let field = ara_ui::source::stable_id("rename-field");
    anyhow::ensure!(
        matches!(app.ui.widgets.get(&field),
        Some(ara_ui::widgets::Widget::Input(input)) if input.value() == "Preview 3 typing"),
        "typed value survives the model echo"
    );
    save(host, app, folder, &format!("{theme}-typed-rename"))?;
    hub.intent(Intent::RenameCancel);
    hub.event(Event::Reply {
        call: Call::Remove {
            project: "project-4".into(),
        },
        result: Ok(json!({"removed":"project-4"})),
    });
    hub.event(Event::Reply {
        call: Call::Close {
            project: "project-3".into(),
        },
        result: Err(ara_diag::Diagnostic::error(
            ara_diag::codes::IO,
            "Capture fixture: the project could not be closed.",
        )
        .into()),
    });
    hub.event(Event::Offline {
        message: "Capture fixture: service unavailable".into(),
        log: "target/hub-captures/hub-start.log".into(),
    });
    settle(host, app, &mut hub)?;
    app.frame().map_err(|e| anyhow::anyhow!("{e}"))?;
    save(host, app, folder, &format!("{theme}-notifications-before"))?;
    let ids: Vec<_> = hub.notices.iter().map(|n| n.id).collect();
    for id in ids {
        click(app, &mut hub, &format!("dismiss-{id}"))?;
    }
    click(app, &mut hub, "dismiss-offline")?;
    click(app, &mut hub, "dismiss-update")?;
    anyhow::ensure!(
        hub.notices.is_empty(),
        "every toast dismisses on its first click"
    );
    anyhow::ensure!(
        !app.frame().map_err(|e| anyhow::anyhow!("{e}"))?,
        "idle UI must not redraw"
    );
    save(host, app, folder, &format!("{theme}-notifications-after"))?;
    hub.event(Event::ServiceUpdatePending);
    settle(host, app, &mut hub)?;
    save(
        host,
        app,
        folder,
        &format!("{theme}-service-update-pending"),
    )?;
    let id = hub.notices.last().context("service restart notice")?.id;
    click(app, &mut hub, &format!("dismiss-{id}"))?;
    anyhow::ensure!(hub.notices.is_empty(), "service restart notice dismisses");
    Ok(())
}
