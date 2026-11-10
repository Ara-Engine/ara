//! C# in the studio beyond creating scripts (`asset_create`) and naming them
//! (`asset_ops::rename_script`): Reload Scripts while Play runs (every player reloads its
//! modules, no restart; the result shows in the status bar, its diagnostics in the
//! Console), and Open Scripts in IDE (the project's C# modules found off the UI thread,
//! the only one opened at once, else a chooser).
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ara_kit::icons;
use ara_kit::ui::*;
use ara_play_host::{ScriptRecoveryOutcome, ScriptRecoveryRequest, ScriptRecoveryTicket};
use stillkit::el::{Align, Axis, col, icon, row, text};

use super::Studio;

/// What module discovery gives back.
type Inventory = Result<Vec<PathBuf>, Vec<ara_diag::Diagnostic>>;

/// The window's C# requests in flight.
#[derive(Default)]
pub(crate) struct Scripts {
    /// A reload's ticket, its world, and when to read it next.
    reload: Option<(ScriptRecoveryTicket, String, Instant)>,
    /// Module discovery, running.
    modules: Option<JoinHandle<Inventory>>,
    /// The modules found, to choose one to open (project-relative).
    pub choice: Option<Vec<String>>,
}

/// Reloads the running world's C# modules on every player.
pub(super) fn reload(s: &mut Studio) -> ara_ui::Result<()> {
    let world = s
        .editor
        .play_world
        .clone()
        .ok_or("Start Play to reload scripts; saved scripts compile on their own")?;
    if s.editor.state.playing.is_none() {
        return Err("Wait for Play to finish starting before reloading scripts".into());
    }
    if s.scripts.reload.is_some() {
        return Err("A reload is already running".into());
    }
    let request = ScriptRecoveryRequest {
        session: s.editor.session.clone(),
        world: world.clone(),
        player: None,
        why: Some("Editor: Reload Scripts".into()),
    };
    let ticket = s
        .editor
        .script_recovery
        .request(request)
        .map_err(|diagnostics| {
            diagnostics.first().map_or_else(
                || "The reload was refused".to_owned(),
                |d| d.message.clone(),
            )
        })?;
    s.scripts.reload = Some((ticket, world, Instant::now()));
    super::note(s, "Reloading scripts…");
    Ok(())
}

/// Finds the project's C# modules, then opens the only one or offers the choice.
pub(super) fn solution(s: &mut Studio) -> ara_ui::Result<()> {
    if s.scripts.modules.is_some() {
        return Ok(());
    }
    let root = s.editor.host.root().to_owned();
    let worker = std::thread::Builder::new()
        .name("studio-script-modules".into())
        .spawn(move || ara_play_host::script_modules(&root))?;
    s.scripts.modules = Some(worker);
    Ok(())
}

/// Reads the reload (4 times a second) and the discovery; whether either changed.
pub(super) fn poll(s: &mut Studio) -> bool {
    let mut changed = false;
    if let Some((ticket, world, next)) = s.scripts.reload.take() {
        match (Instant::now() >= next)
            .then(|| s.editor.script_recovery.poll(ticket))
            .flatten()
        {
            Some(status) if status.finished => {
                let failed = status.players.iter().any(|p| {
                    matches!(
                        p.outcome,
                        ScriptRecoveryOutcome::Failed | ScriptRecoveryOutcome::Refused
                    )
                });
                let unchanged = status
                    .players
                    .iter()
                    .all(|p| matches!(p.outcome, ScriptRecoveryOutcome::Unchanged));
                let message = if status.players.is_empty() {
                    "Scripts reloaded; no player answered"
                } else if failed {
                    "Scripts did not reload on every player; see the Console"
                } else if unchanged {
                    "Scripts unchanged since the last reload"
                } else {
                    "Scripts reloaded"
                };
                super::note(s, message);
                changed = true;
            }
            _ => {
                let next = match Instant::now() >= next {
                    true => Instant::now() + Duration::from_millis(250),
                    false => next,
                };
                s.scripts.reload = Some((ticket, world, next));
            }
        }
    }
    if s.scripts
        .modules
        .as_ref()
        .is_some_and(JoinHandle::is_finished)
        && let Some(worker) = s.scripts.modules.take()
    {
        changed = true;
        let root = s.editor.host.root().to_owned();
        match worker.join() {
            Ok(Ok(paths)) => {
                let mut paths: Vec<String> = paths
                    .iter()
                    .map(|p| {
                        p.strip_prefix(&root)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .replace('\\', "/")
                    })
                    .collect();
                paths.sort();
                paths.dedup();
                match paths.as_slice() {
                    [] => s.editor.report_failure(&*anyhow::anyhow!(
                        "No C# modules yet: create a script first"
                    )),
                    [only] => super::asset_ops::open_in_ide(s, &only.clone(), None),
                    _ => s.scripts.choice = Some(paths),
                }
            }
            Ok(Err(diagnostics)) => {
                let message = diagnostics
                    .first()
                    .map_or("C# module discovery failed", |d| d.message.as_str())
                    .to_owned();
                s.editor.report_failure(&*anyhow::anyhow!(message));
            }
            Err(_) => s.editor.report_failure(&*anyhow::anyhow!(
                "C# module discovery stopped unexpectedly"
            )),
        }
    }
    changed
}

/// The modules to open, when there is more than one.
pub(super) fn chooser(s: &Studio) -> Option<stillkit::el::Overlay<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let paths = s.scripts.choice.as_ref()?;
    let hover = c.hover_surface.0;
    let mut list = col().gap(1.0).max_h(320.0).scroll(Axis::Vertical);
    for path in paths {
        let open = path.clone();
        list = list.child(
            row()
                .key(format!("scripts/module/{path}"))
                .h(super::style::ROW)
                .pad_x(8.0)
                .gap(8.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(path.clone())
                .hovered(move |e| e.fill(hover))
                .on_press(move |s: &mut Studio| {
                    s.scripts.choice = None;
                    super::asset_ops::open_in_ide(s, &open, None);
                })
                .child(
                    icon(icons::FILE_CODE)
                        .size(13.0)
                        .color(c.muted_foreground.0),
                )
                .child(
                    text(path.clone())
                        .text_size(12.0)
                        .family(mono())
                        .color(c.foreground.0)
                        .wrap(1),
                ),
        );
    }
    let close = |s: &mut Studio| s.scripts.choice = None;
    let panel = dialog_panel(
        l,
        Some((Status::Info, icons::FILE_CODE)),
        "Open scripts in the IDE",
        col()
            .gap(8.0)
            .child(
                text("Choose the C# module to open.")
                    .text_size(12.0)
                    .color(c.muted_foreground.0),
            )
            .child(list),
        vec![
            button(l, Tone::Ghost, Size::Md, None, "Cancel")
                .key("scripts/module/cancel")
                .on_press(close),
        ],
        close,
    )
    .w(460.0);
    Some(stillkit::el::dialog(panel).on_dismiss(close))
}
