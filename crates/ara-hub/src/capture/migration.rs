//! Migration consent evidence uses service-shaped replies and real pointer input.
use super::*;
use ara_ui::input::InputEvent;

/// Prepare an old project's read-only move preview without touching any project.
pub(super) fn seed_dialog(hub: &mut Hub) {
    hub.dialog = None;
    hub.status.remove("project-1");
    hub.intent(Intent::Open("project-1".into()));
    for call in hub.take_calls() {
        if matches!(call, Call::Open { .. }) {
            hub.event(Event::Reply {
                call,
                result: Err(ara_diag::Diagnostic::error(
                    ara_diag::codes::PROJECT_MIGRATION_REQUIRED,
                    "This project needs the current folder layout before it can open.",
                )
                .into()),
            });
        }
    }
    let plan = json!({"needsMigration":true,"fromVersion":3,"toVersion":4,"authoredRoot":"content","planToken":"fixture-readonly-plan","operations":[
        {"op":"move","from":"game/levels/","to":"content/levels/"},{"op":"move","from":"game/materials/","to":"content/materials/"},{"op":"move","from":"game/scripts/","to":"content/scripts/"},{"op":"move","from":"game/ui/","to":"content/ui/"},{"op":"rewrite","path":"ara.project.json"},{"op":"rewrite","path":"settings/project.json"},{"op":"rewrite","path":"settings/builds/windows.export.json"}
    ],"previewTree":["ara.project.json","content/","  levels/","    outpost.scene","  materials/","  scripts/","    editor/","  ui/","settings/","  project.json","  builds/","    windows.export.json","AGENTS.md",".mcp.json"]});
    for call in hub.take_calls() {
        if call.method() == "hub.project.migration.plan" {
            hub.event(Event::Reply {
                call,
                result: Ok(job(
                    "plan",
                    "complete",
                    false,
                    Some(plan.clone()),
                    "Preview ready",
                    1.,
                )),
            });
        }
    }
}

fn job(
    kind: &str,
    state: &str,
    cancellable: bool,
    result: Option<Value>,
    phase: &str,
    progress: f32,
) -> Value {
    json!({"id":"migration-fixture","project":"project-1","kind":kind,"state":state,"phase":phase,"cancellable":cancellable,"progress":progress,"result":result,"diagnostics":[]})
}

fn reply(hub: &mut Hub, value: Value) {
    for call in hub.take_calls() {
        if matches!(call, Call::Migration { .. }) {
            hub.event(Event::Reply {
                call,
                result: Ok(value.clone()),
            });
        }
    }
}

fn click(app: &mut crate::native::App, hub: &mut Hub, id: &str) -> anyhow::Result<()> {
    let index = app
        .ui
        .find(id)
        .ok_or_else(|| anyhow::anyhow!("missing migration control {id}"))?;
    let rect = app.ui.nodes[index].rect;
    let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
    for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
        app.dispatch_pointer(hub, event, NOW)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
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
        "target/hub-flows".into(),
        None,
    );
    seed(&mut hub);
    seed_dialog(&mut hub);
    settle(host, app, &mut hub)?;
    host.capture(app)?
        .save(folder.join(format!("{theme}-migration-modal.png")))?;
    click(app, &mut hub, "migration-cancel")?;
    anyhow::ensure!(
        hub.migration.is_none()
            && !hub
                .take_calls()
                .iter()
                .any(|call| call.method() == "hub.open"),
        "Cancel keeps the old project closed"
    );
    host.capture(app)?
        .save(folder.join(format!("{theme}-migration-cancelled.png")))?;
    seed_dialog(&mut hub);
    settle(host, app, &mut hub)?;
    click(app, &mut hub, "migration-submit")?;
    reply(
        &mut hub,
        job(
            "migrate",
            "running",
            true,
            None,
            "Preparing authored files and reference updates",
            0.35,
        ),
    );
    settle(host, app, &mut hub)?;
    host.capture(app)?
        .save(folder.join(format!("{theme}-migration-progress.png")))?;
    reply(
        &mut hub,
        job(
            "migrate",
            "running",
            false,
            None,
            "Saving project files and history",
            0.9,
        ),
    );
    settle(host, app, &mut hub)?;
    host.capture(app)?
        .save(folder.join(format!("{theme}-migration-committing.png")))?;
    Ok(())
}
