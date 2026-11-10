//! Decisions and errors share one focused dialog surface: a heading, the decision's facts,
//! and its actions bottom-right (the safe action first). stillkit's dialog traps focus,
//! closes on Escape or the backdrop (where the decision allows) and returns focus after.
use ara_diag::FixAction;
use ara_kit::icons;
use ara_kit::look::Look;
use ara_kit::ui::*;
use stillkit::el::{Align, Axis, El, col, dialog, el, icon, row, text};
use stillkit::icons::Icon;

use super::installs::{bytes, transfer};
use super::{HubApp, settings, shell};
use crate::model::{Dialog, Intent, State, updates};

/// The open dialog, over everything.
pub(super) fn layer(s: &HubApp, root: El<HubApp>) -> El<HubApp> {
    let Some(d) = &s.hub.dialog else { return root };
    let (panel, closable) = match d {
        Dialog::EngineLayout(project) => engine_layout(s, project),
        Dialog::Migration(project) => migration(s, project),
        Dialog::Update => update(s),
        Dialog::InstallEngine(v) => (install(s, v, Kind::Install), true),
        Dialog::InstallCurrentEngine(v) => (install(s, v, Kind::Current), true),
        Dialog::RepairEngine(v) => (install(s, v, Kind::Repair), true),
        Dialog::UninstallEngine(engine) => (uninstall(s, engine), true),
        Dialog::Confirm {
            title,
            detail,
            label,
            destructive,
            ..
        } => (confirm(s, title, detail, label, *destructive), true),
        Dialog::Error(id) => (error(s, *id), true),
    };
    let mut overlay = dialog(panel);
    if closable {
        overlay = overlay.on_dismiss(|s: &mut HubApp| s.intent(Intent::DismissDialog));
    }
    root.overlay(overlay)
}

/// A dialog panel: optional status icon, title, body column and actions.
fn panel(
    l: &Look,
    status: Option<(Status, Icon)>,
    title: &str,
    body: Vec<El<HubApp>>,
    actions: Vec<El<HubApp>>,
    closable: bool,
) -> El<HubApp> {
    let close = move |s: &mut HubApp| {
        if closable {
            s.intent(Intent::DismissDialog);
        }
    };
    let content = col()
        .gap(12.0)
        .max_h(520.0)
        .scroll(Axis::Vertical)
        .children(body);
    let mut p = dialog_panel(l, status, title, content, actions, close)
        .key("hub-dialog")
        .w(500.0);
    if !closable {
        p = p.label(format!("{title} (in progress)"));
    }
    p
}

fn copy(l: &Look, s: &str) -> El<HubApp> {
    dialog_text(l, s)
}

fn dismiss(l: &Look, label: &str) -> El<HubApp> {
    secondary(l, label)
        .key("dialog-cancel")
        .on_press(|s: &mut HubApp| s.intent(Intent::DismissDialog))
}

fn later(l: &Look) -> El<HubApp> {
    primary(l, "Continue in background")
        .key("dialog-later")
        .on_press(|s: &mut HubApp| s.intent(Intent::DismissDialog))
}

fn waiting(l: &Look, message: &str) -> El<HubApp> {
    row()
        .gap(10.0)
        .align(Align::Center)
        .child(spinner(l, 14.0))
        .child(
            text(message.to_owned())
                .text_size(12.5)
                .color(l.c.foreground.0),
        )
}

fn confirm(s: &HubApp, title: &str, detail: &str, label: &str, destructive: bool) -> El<HubApp> {
    let l = &s.look;
    let go = button(
        l,
        if destructive {
            Tone::Danger
        } else {
            Tone::Primary
        },
        Size::Md,
        None,
        label,
    )
    .key("dialog-confirm")
    .on_press(|s: &mut HubApp| s.intent(Intent::ConfirmDialog));
    let status = if destructive {
        (Status::Danger, icons::TRASH)
    } else {
        (Status::Accent, icons::REFRESH_CW)
    };
    panel(
        l,
        Some(status),
        title,
        vec![copy(l, detail)],
        vec![dismiss(l, "Cancel"), go],
        true,
    )
}

fn error(s: &HubApp, id: u64) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let Some(notice) = hub.notices.iter().find(|n| n.id == id) else {
        return panel(
            l,
            None,
            "Error",
            Vec::new(),
            vec![dismiss(l, "Close")],
            true,
        );
    };
    let mut body = Vec::new();
    let mut fix_index = 0;
    for diagnostic in &notice.diagnostics {
        body.push(copy(l, &diagnostic.message));
        body.push(row().child(chip(l, Status::Danger, &diagnostic.code.to_string(), false)));
        for fix in &diagnostic.fixes {
            let index = fix_index;
            fix_index += 1;
            match &fix.action {
                FixAction::Call { method, .. } if method.starts_with("hub.") => {
                    body.push(
                        row().child(
                            button(l, Tone::Secondary, Size::Sm, Some(icons::CHECK), &fix.title)
                                .key(format!("dialog-fix-{index}"))
                                .on_press(move |s: &mut HubApp| {
                                    s.intent(Intent::Fix {
                                        notice: id,
                                        fix: index,
                                    })
                                }),
                        ),
                    );
                }
                FixAction::Cli { args } => {
                    body.push(
                        text(fix.title.clone())
                            .text_size(12.0)
                            .color(c.muted_foreground.0),
                    );
                    body.push(shell::code(
                        s,
                        &format!("dialog-copy-fix-{index}"),
                        &format!("ara {}", args.join(" ")),
                    ));
                }
                _ => body.push(
                    text(fix.title.clone())
                        .text_size(12.0)
                        .color(c.muted_foreground.0),
                ),
            }
        }
    }
    if let Some(detail) = &notice.detail {
        body.push(copy(l, detail));
    }
    let mut actions = Vec::new();
    if let Some(retry) = &notice.retry {
        let r = (**retry).clone();
        actions.push(
            button(
                l,
                Tone::Secondary,
                Size::Md,
                Some(icons::REFRESH_CW),
                "Retry",
            )
            .key("dialog-retry-operation")
            .on_press(move |s: &mut HubApp| s.intent(r.clone())),
        );
    }
    if let Some(log) = &notice.log {
        let log = log.clone();
        actions.push(
            button(
                l,
                Tone::Secondary,
                Size::Md,
                Some(icons::FILE_TEXT),
                "Open log",
            )
            .key("dialog-open-log")
            .on_press(move |s: &mut HubApp| s.intent(Intent::OpenPath(log.clone()))),
        );
    }
    if notice.retry.is_none()
        && let Some(job) = hub.releases.jobs.iter().find(|j| {
            j.state == "failed"
                && j.error
                    .as_ref()
                    .is_some_and(|e| notice.diagnostics.contains(e))
        })
    {
        let retry = if job.kind == "uninstall" {
            Intent::RetryUninstall(job.version.clone())
        } else {
            Intent::Dialog(match job.kind.as_str() {
                "hub" => Dialog::Update,
                "repair" => Dialog::RepairEngine(job.version.clone()),
                _ => Dialog::InstallEngine(job.version.clone()),
            })
        };
        actions.push(
            button(
                l,
                Tone::Secondary,
                Size::Md,
                Some(icons::REFRESH_CW),
                "Retry",
            )
            .key("dialog-retry")
            .on_press(move |s: &mut HubApp| s.intent(retry.clone())),
        );
    }
    actions.push(
        primary(l, "Got it")
            .key("dialog-error-dismiss")
            .on_press(move |s: &mut HubApp| s.intent(Intent::Dismiss(id))),
    );
    panel(
        l,
        Some((Status::Danger, icons::CIRCLE_ALERT)),
        &notice.title,
        body,
        actions,
        true,
    )
}

fn identity(l: &Look, release: &ara_update::Manifest) -> El<HubApp> {
    let channel = if release.channel == ara_update::Channel::Preview {
        "Preview"
    } else {
        "Stable"
    };
    let mut r = row()
        .gap(6.0)
        .child(chip(l, Status::Neutral, channel, false));
    if let Some(build) = &release.build {
        r = r.child(chip(
            l,
            Status::Neutral,
            &format!("build {}", build.commit.chars().take(8).collect::<String>()),
            false,
        ));
    }
    r
}

/// Release notes (Markdown) in a bounded, scrolling well.
fn notes(l: &Look, notes: &str) -> El<HubApp> {
    col()
        .max_h(180.0)
        .scroll(Axis::Vertical)
        .pad(12.0)
        .radius(l.radius.sm)
        .fill(l.c.muted.0)
        .border(1.0, l.c.border.0)
        .child(super::markdown::render(l, notes))
}

fn update(s: &HubApp) -> (El<HubApp>, bool) {
    let l = &s.look;
    let hub = &s.hub;
    let Some(release) = &hub.releases.hub else {
        return (
            panel(
                l,
                Some((Status::Success, icons::CIRCLE_CHECK)),
                "You're up to date",
                vec![copy(
                    l,
                    "This is the newest Ara Hub for your release channel.",
                )],
                vec![
                    primary(l, "Done")
                        .key("dialog-done")
                        .on_press(|s: &mut HubApp| s.intent(Intent::DismissDialog)),
                ],
                true,
            ),
            true,
        );
    };
    let title = format!("Ara Hub {}", release.version);
    let mut body = vec![
        identity(l, release),
        label(l, "What's new"),
        notes(
            l,
            if release.notes.trim().is_empty() {
                "This release includes improvements to Ara Hub."
            } else {
                &release.notes
            },
        ),
    ];
    let actions;
    if hub.releases.starting("hub", &release.version) {
        return (pending(s, &title, "hub", &release.version, body), true);
    } else if let Some(job) = hub.releases.running("hub", &release.version) {
        body.push(transfer(s, job));
        body.push(copy(
            l,
            "You can keep using Ara while this downloads. Restart is always your choice.",
        ));
        let id = job.id.clone();
        actions = vec![
            enabled(
                secondary(l, "Cancel download")
                    .key("cancel-hub-update")
                    .on_press(move |s: &mut HubApp| s.intent(Intent::CancelDownload(id.clone()))),
                job.cancellable,
            ),
            later(l),
        ];
    } else if hub.releases.prepared().is_some() {
        body.push(banner(l, Status::Success, icons::CIRCLE_CHECK, "Verified and ready to install. Ara Hub will close and reopen; your projects and the Hub service keep running.", Vec::new()));
        actions = vec![
            dismiss(l, "Later"),
            button(
                l,
                Tone::Primary,
                Size::Md,
                Some(icons::REFRESH_CW),
                "Restart and update",
            )
            .key("restart-update")
            .on_press(|s: &mut HubApp| s.intent(Intent::RestartUpdate)),
        ];
    } else {
        if hub
            .releases
            .jobs
            .iter()
            .rev()
            .any(|j| j.kind == "hub" && j.version == release.version && j.state == "cancelled")
        {
            body.push(copy(
                l,
                "Download cancelled. You can download this update again when ready.",
            ));
        }
        body.push(copy(
            l,
            "The update downloads first. Ara Hub asks you to restart when it is ready.",
        ));
        body.push(settings::advanced(s, false));
        if hub.prefs.skipped_update.as_deref() == Some(updates::identity(release).as_str()) {
            body.push(copy(
                l,
                "You skipped this release. You can still update here.",
            ));
        }
        if !hub.releases.can_self_update {
            body.push(banner(l, Status::Info, icons::INFO, "This Hub is bundled with an engine. Use Ara Hub Setup to update the standalone Hub.", Vec::new()));
        }
        actions = vec![
            button(l, Tone::Ghost, Size::Md, None, "Skip this version")
                .key("skip-update")
                .on_press(|s: &mut HubApp| s.intent(Intent::SkipUpdate)),
            el().grow(1.0),
            dismiss(l, "Later"),
            enabled(
                button(
                    l,
                    Tone::Primary,
                    Size::Md,
                    Some(icons::DOWNLOAD),
                    "Update now",
                )
                .key("download-update")
                .on_press(|s: &mut HubApp| s.intent(Intent::PrepareUpdate)),
                hub.releases.can_self_update,
            ),
        ];
    }
    (
        panel(
            l,
            Some((Status::Accent, icons::SPARKLES)),
            &title,
            body,
            actions,
            true,
        ),
        true,
    )
}

/// A transfer requested and not yet answered: cancel it, or let it run in the background.
fn pending(
    s: &HubApp,
    title: &str,
    kind: &str,
    version: &str,
    mut body: Vec<El<HubApp>>,
) -> El<HubApp> {
    let l = &s.look;
    let cancelling = s.hub.releases.cancelling(kind, version);
    body.push(waiting(
        l,
        if cancelling {
            "Cancelling the request…"
        } else {
            "Preparing your download…"
        },
    ));
    let (k, v) = (kind.to_owned(), version.to_owned());
    let cancel = enabled(
        secondary(l, "Cancel")
            .key("cancel-pending-transfer")
            .on_press(move |s: &mut HubApp| {
                s.intent(Intent::CancelPending {
                    kind: k.clone(),
                    version: v.clone(),
                })
            }),
        !cancelling,
    );
    panel(
        l,
        Some((Status::Accent, icons::DOWNLOAD)),
        title,
        body,
        vec![cancel, later(l)],
        true,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Install,
    Current,
    Repair,
}

fn install(s: &HubApp, version: &str, kind: Kind) -> El<HubApp> {
    let l = &s.look;
    let hub = &s.hub;
    let title = format!(
        "{} Ara {version}",
        if kind == Kind::Repair {
            "Repair"
        } else {
            "Install"
        }
    );
    let job_kind = match kind {
        Kind::Current => "engine-current",
        Kind::Repair => "repair",
        Kind::Install => "engine",
    };
    let plan = hub.releases.plans.get(version);
    let destination = plan
        .map(|p| p.destination.clone())
        .or_else(|| hub.releases.install_root.as_ref().map(|r| r.join(version)));
    let mut body = Vec::new();
    if let Some(d) = destination {
        body.push(
            col().gap(4.0).child(label(l, "Install location")).child(
                text(crate::model::text::path(&d))
                    .text_size(12.0)
                    .family(mono())
                    .color(l.c.foreground.0),
            ),
        );
    }
    if hub.releases.starting(job_kind, version) {
        return pending(s, &title, job_kind, version, body);
    }
    if let Some(job) = hub.releases.running(job_kind, version) {
        body.push(transfer(s, job));
        body.push(copy(
            l,
            "Your other engine versions remain available while this installs.",
        ));
        let id = job.id.clone();
        return panel(
            l,
            Some((Status::Accent, icons::DOWNLOAD)),
            &title,
            body,
            vec![
                enabled(
                    secondary(l, "Cancel installation")
                        .key("cancel-engine-install")
                        .on_press(move |s: &mut HubApp| {
                            s.intent(Intent::CancelDownload(id.clone()))
                        }),
                    job.cancellable,
                ),
                later(l),
            ],
            true,
        );
    }
    let complete = |k: &str| {
        hub.releases
            .jobs
            .iter()
            .any(|j| j.kind == k && j.version == version && j.state == "complete")
    };
    let done = match kind {
        Kind::Install => hub.engines.iter().any(|e| e.version == version),
        Kind::Current => complete("engine-current"),
        Kind::Repair => complete("repair"),
    };
    if done {
        body.push(banner(
            l,
            Status::Success,
            icons::CIRCLE_CHECK,
            if kind == Kind::Repair {
                "Repaired and ready for your projects."
            } else {
                "Installed and ready for your projects."
            },
            Vec::new(),
        ));
        return panel(
            l,
            Some((Status::Success, icons::CIRCLE_CHECK)),
            &title,
            body,
            vec![
                primary(l, "Done")
                    .key("dialog-done")
                    .on_press(|s: &mut HubApp| s.intent(Intent::DismissDialog)),
            ],
            true,
        );
    }
    body.push(copy(l, match kind {
        Kind::Current => "Install the signed current editor and tools. A same-version managed installation is replaced only after verification; external files stay in place. Close projects using this version first. Your project opens after its engine choice has been saved.",
        Kind::Repair => "Download a fresh copy of the editor and tools. Your projects and engine version pins stay unchanged. Close projects using this version before repairing.",
        Kind::Install => "Install the editor and tools side by side. Existing projects keep their selected engine version.",
    }));
    if let Some(release) = hub.releases.available.iter().find(|r| r.version == version) {
        body.push(identity(l, release));
        if !release.notes.is_empty() {
            body.push(notes(l, &release.notes));
        }
    }
    match plan {
        Some(p) => body.push(shell::facts(
            l,
            &[
                ("Download", bytes(p.download_bytes)),
                ("Estimated required space", bytes(p.required_bytes)),
                ("Available space", bytes(p.available_bytes)),
            ],
        )),
        None => body.push(waiting(
            l,
            "Checking the installation location and available space…",
        )),
    }
    let in_use = kind != Kind::Install
        && hub
            .projects
            .iter()
            .any(|p| p.engine_version == version && hub.state(&p.id) != State::Closed);
    if in_use {
        body.push(banner(
            l,
            Status::Warning,
            icons::TRIANGLE_ALERT,
            "Close projects using this engine first.",
            Vec::new(),
        ));
    }
    if plan.is_some_and(|p| p.available_bytes < p.required_bytes) {
        body.push(banner(
            l,
            Status::Danger,
            icons::HARD_DRIVE,
            "Free space on the installation drive, then try again.",
            Vec::new(),
        ));
    }
    body.push(settings::advanced(s, false));
    let failed = hub.releases.jobs.iter().any(|j| {
        j.kind == job_kind
            && j.version == version
            && matches!(j.state.as_str(), "failed" | "cancelled")
    });
    if hub
        .releases
        .jobs
        .iter()
        .any(|j| j.kind == job_kind && j.version == version && j.state == "cancelled")
    {
        body.push(copy(l, "Installation cancelled. Retry when you're ready."));
    }
    let label = match (kind, failed) {
        (Kind::Current, false) => "Install current engine",
        (Kind::Repair, false) => "Repair engine",
        (Kind::Install, false) => "Install engine",
        (Kind::Repair, true) => "Retry repair",
        _ => "Retry installation",
    };
    let intent = match kind {
        Kind::Current => Intent::InstallCurrentEngine(version.into()),
        Kind::Repair => Intent::RepairEngine(version.into()),
        Kind::Install => Intent::InstallEngine(version.into()),
    };
    let ok = plan.is_some_and(|p| p.available_bytes >= p.required_bytes) && !in_use;
    let go = enabled(
        button(l, Tone::Primary, Size::Md, Some(icons::DOWNLOAD), label)
            .key("confirm-install-engine")
            .on_press(move |s: &mut HubApp| s.intent(intent.clone())),
        ok,
    );
    panel(
        l,
        Some((Status::Accent, icons::HARD_DRIVE)),
        &title,
        body,
        vec![dismiss(l, "Cancel"), go],
        true,
    )
}

fn uninstall(s: &HubApp, engine: &crate::model::Engine) -> El<HubApp> {
    let l = &s.look;
    let hub = &s.hub;
    let summary = hub.releases.removal(&engine.version);
    let mut body = vec![copy(
        l,
        "This removes the installed editor and tools. Your project files stay on disk.",
    )];
    let destination = summary.map_or_else(
        || crate::model::text::executable_directory(&engine.executable),
        |s| crate::model::text::path(&s.destination),
    );
    let mut facts = col()
        .gap(6.0)
        .child(label(l, "Installation to remove"))
        .child(
            text(destination)
                .text_size(12.0)
                .family(mono())
                .color(l.c.foreground.0),
        );
    facts = match summary {
        Some(sm) => facts.child(shell::facts(
            l,
            &[("Approximate space freed", bytes(sm.bytes))],
        )),
        None => facts.child(waiting(l, "Calculating space…")),
    };
    body.push(facts);
    let projects: Vec<_> = hub
        .projects
        .iter()
        .filter(|p| p.engine_version == engine.version)
        .collect();
    let in_use = projects.iter().any(|p| hub.state(&p.id) != State::Closed);
    if projects.is_empty() {
        body.push(copy(l, "No registered projects use this engine version."));
    } else {
        body.push(label(l, "Projects using this version"));
        body.push(
            row().gap(6.0).children(
                projects
                    .iter()
                    .take(8)
                    .map(|p| chip(l, Status::Neutral, &p.name, false)),
            ),
        );
        body.push(copy(
            l,
            "These projects keep their engine pin and will need this version installed again.",
        ));
    }
    if in_use {
        body.push(banner(
            l,
            Status::Warning,
            icons::TRIANGLE_ALERT,
            "Close projects using this engine before uninstalling it.",
            Vec::new(),
        ));
    }
    let go = enabled(
        button(
            l,
            Tone::Danger,
            Size::Md,
            Some(icons::TRASH),
            "Uninstall engine",
        )
        .key("dialog-confirm")
        .on_press(|s: &mut HubApp| s.intent(Intent::ConfirmDialog)),
        !in_use && summary.is_some(),
    );
    panel(
        l,
        Some((Status::Danger, icons::TRASH)),
        &format!("Uninstall Ara {}?", engine.version),
        body,
        vec![dismiss(l, "Cancel"), go],
        true,
    )
}

fn migration(s: &HubApp, project: &str) -> (El<HubApp>, bool) {
    use crate::model::migration::{Operation, Phase};
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let Some(flow) = hub.migration.as_ref().filter(|f| f.project == project) else {
        return (
            panel(
                l,
                None,
                "Update folder layout",
                Vec::new(),
                vec![dismiss(l, "Close")],
                true,
            ),
            true,
        );
    };
    let name = hub.project(project).map_or("project", |p| p.name.as_str());
    let title = format!("Update {name}'s folder layout");
    let mut body = Vec::new();
    match flow.phase {
        Phase::Checking => {
            body.push(waiting(l, "Checking what needs to move…"));
            body.push(copy(
                l,
                "This check only reads the project. Nothing moves until you choose Migrate.",
            ));
        }
        Phase::Decision => {
            body.push(copy(l, "This project uses an older folder layout. Migrate it before opening in this preview. Asset IDs and references are preserved, and the migration is recorded as one undoable change."));
            if let Some(plan) = &flow.plan {
                let moves: Vec<_> = plan
                    .operations
                    .iter()
                    .filter_map(|op| match op {
                        Operation::Move { from, to } => Some(format!("{from} → {to}")),
                        _ => None,
                    })
                    .collect();
                body.push(label(l, "What moves"));
                if moves.is_empty() {
                    body.push(copy(
                        l,
                        "Project settings and the authored-content root are updated.",
                    ));
                } else {
                    body.push(
                        col()
                            .max_h(140.0)
                            .scroll(Axis::Vertical)
                            .pad(10.0)
                            .radius(l.radius.sm)
                            .fill(c.code_background.0)
                            .border(1.0, c.border.0)
                            .gap(2.0)
                            .children(moves.into_iter().map(|m| {
                                text(m)
                                    .text_size(11.5)
                                    .family(mono())
                                    .color(c.code_foreground.0)
                            })),
                    );
                }
                let rewrites = plan
                    .operations
                    .iter()
                    .filter(|op| matches!(op, Operation::Rewrite { .. }))
                    .count();
                if rewrites > 0 {
                    body.push(copy(
                        l,
                        &format!("References and settings are updated in {rewrites} files."),
                    ));
                }
                body.push(label(l, "After migration"));
                let mut tree = col()
                    .max_h(160.0)
                    .scroll(Axis::Vertical)
                    .pad(10.0)
                    .radius(l.radius.sm)
                    .fill(c.code_background.0)
                    .border(1.0, c.border.0)
                    .gap(1.0)
                    .children(plan.preview_tree.iter().take(160).map(|line| {
                        text(line.clone())
                            .text_size(11.5)
                            .family(mono())
                            .color(c.code_foreground.0)
                    }));
                if plan.preview_tree.len() > 160 {
                    tree = tree.child(
                        text(format!("…and {} more paths", plan.preview_tree.len() - 160))
                            .text_size(11.0)
                            .color(c.muted_foreground.0),
                    );
                }
                body.push(tree);
                body.push(copy(l, &format!("Authored files live in {}/. Cancel keeps the project closed and leaves its files unchanged.", plan.authored_root.trim_end_matches('/'))));
            }
        }
        Phase::Migrating | Phase::Cancelling => {
            let phase = flow
                .job
                .as_ref()
                .map(|j| j.phase.as_str())
                .filter(|p| !p.is_empty())
                .unwrap_or("Preparing migration…");
            body.push(waiting(l, phase));
            body.push(progress(
                l,
                flow.job.as_ref().map_or(0.0, |j| j.progress),
                Status::Accent,
            ));
            body.push(copy(l, if flow.phase == Phase::Cancelling { "Requesting cancellation. If saving has already begun, the migration will finish safely." } else if flow.cancellable() { "The project opens once its files and history have finished saving. Cancel is available before the commit begins." } else { "Finishing the project files and history. The project will open when everything has been saved." }));
        }
    }
    let mut actions = vec![enabled(
        secondary(
            l,
            if flow.phase == Phase::Cancelling {
                "Cancelling…"
            } else {
                "Cancel"
            },
        )
        .key("migration-cancel")
        .on_press(|s: &mut HubApp| s.intent(Intent::CancelMigration)),
        flow.cancellable(),
    )];
    if flow.phase == Phase::Decision {
        actions.push(
            primary(l, "Migrate")
                .key("migration-submit")
                .on_press(|s: &mut HubApp| s.intent(Intent::MigrateProject)),
        );
    }
    let closable = flow.cancellable();
    (
        panel(
            l,
            Some((Status::Accent, icons::FOLDER_TREE)),
            &title,
            body,
            actions,
            closable,
        ),
        closable,
    )
}

fn engine_layout(s: &HubApp, project: &str) -> (El<HubApp>, bool) {
    use crate::model::engine_layout::Phase;
    let l = &s.look;
    let hub = &s.hub;
    let Some(flow) = hub.engine_layout.as_ref().filter(|f| f.project == project) else {
        return (
            panel(
                l,
                None,
                "Choose a compatible engine",
                Vec::new(),
                vec![dismiss(l, "Close")],
                true,
            ),
            true,
        );
    };
    let mut body = vec![copy(
        l,
        "This project uses the content/ layout and needs Ara preview 4 or newer.",
    )];
    if let Some(p) = hub.project(project) {
        body.push(shell::facts(
            l,
            &[
                ("Project", p.name.clone()),
                ("Pinned engine", p.engine_version.clone()),
            ],
        ));
    }
    match flow.phase {
        Phase::Checking => body.push(waiting(l, "Checking the current engine…")),
        Phase::Pinning => {
            body.push(waiting(l, "Saving your engine choice…"));
            body.push(copy(
                l,
                "The project opens after the engine pin and its undo history have finished saving.",
            ));
        }
        Phase::Decision | Phase::Installing => {
            body.push(copy(l, if flow.available { "Use the current engine for this project? This changes its engine pin in one recorded change, then continues opening it." } else { "Install the current engine first. After its signed download is verified, the project pin changes in one recorded change and the project opens." }));
            body.push(
                row()
                    .gap(6.0)
                    .align(Align::Center)
                    .child(
                        icon(icons::HARD_DRIVE)
                            .size(14.0)
                            .color(l.c.muted_foreground.0),
                    )
                    .child(
                        text(format!("Current engine: Ara {}", flow.version))
                            .text_size(12.0)
                            .color(l.c.muted_foreground.0),
                    ),
            );
        }
    }
    let closable = flow.phase != Phase::Pinning;
    let mut actions = vec![enabled(
        secondary(l, "Cancel")
            .key("engine-layout-cancel")
            .on_press(|s: &mut HubApp| s.intent(Intent::CancelEngineLayout)),
        closable,
    )];
    if flow.phase == Phase::Decision {
        actions.push(
            primary(
                l,
                if flow.available {
                    "Use current engine"
                } else {
                    "Install current engine"
                },
            )
            .key("engine-layout-use")
            .on_press(|s: &mut HubApp| s.intent(Intent::AcceptCurrentEngine)),
        );
    }
    (
        panel(
            l,
            Some((Status::Accent, icons::HARD_DRIVE)),
            "Choose a compatible engine",
            body,
            actions,
            closable,
        ),
        closable,
    )
}
