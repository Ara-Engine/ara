//! Installs: engine builds side by side (the version is each card's hero), published
//! releases to install with their progress inline, and each project's engine pin.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::TextRole;
use stillkit::el::{Align, El, Justify, col, el, menu, row, text};

use super::images::Brand;
use super::{HubApp, shell};
use crate::model::updates::Download;
use crate::model::{Dialog, Intent, State, text as fmt};

pub(super) fn body(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let hub = &s.hub;
    let mut page = col().gap(24.0);
    // Removal and repair in progress lead.
    for job in hub
        .releases
        .jobs
        .iter()
        .filter(|j| matches!(j.kind.as_str(), "uninstall" | "repair") && j.state == "running")
    {
        let verb = if job.kind == "repair" {
            "Repairing"
        } else {
            "Uninstalling"
        };
        let mut c = card(l)
            .pad(16.0)
            .gap(10.0)
            .child(text(format!("{verb} Ara {}", job.version)).font(TextRole::Headline))
            .child(transfer(s, job));
        if job.cancellable {
            let id = job.id.clone();
            c = c.child(
                row().justify(Justify::End).child(
                    button(l, Tone::Secondary, Size::Sm, Some(icons::X), "Cancel")
                        .key(format!("cancel-uninstall-{}", job.id))
                        .on_press(move |s: &mut HubApp| {
                            s.intent(Intent::CancelDownload(id.clone()))
                        }),
                ),
            );
        }
        page = page.child(c);
    }
    page = page.child(installed(s)).child(published(s));
    if !hub.projects.is_empty() {
        page = page.child(pins(s));
    }
    page
}

fn installed(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let mut out = col()
        .gap(12.0)
        .child(section(l, "Installed builds", None, Vec::new()));
    if hub.engines.is_empty() && hub.ready() {
        return out.child(card(l).child(empty(
            l,
            icons::HARD_DRIVE,
            "No engine builds registered",
            "Add the folder of an Ara installation: the one with the ara executable and engine.json.",
            Some(button(l, Tone::Primary, Size::Md, Some(icons::PLUS), "Add engine build").key("empty-add-engine").on_press(|s: &mut HubApp| s.intent(Intent::AddEngine))),
        )));
    }
    for (index, engine) in hub.engines.iter().enumerate() {
        let pinned: Vec<_> = hub
            .projects
            .iter()
            .filter(|p| p.engine_version == engine.version)
            .collect();
        let open: Vec<_> = pinned
            .iter()
            .filter(|p| hub.state(&p.id) != State::Closed)
            .map(|p| p.name.as_str())
            .collect();
        let mut chips = row().gap(6.0).align(Align::Center).child(
            text(format!("Ara {}", engine.version))
                .font(TextRole::Title3)
                .color(c.foreground.0),
        );
        if hub.local_engine.as_ref() == Some(engine) {
            chips = chips.child(chip(l, Status::Accent, "This Hub", false));
        }
        chips = chips.child(chip(
            l,
            Status::Neutral,
            if engine.editor.is_some() {
                "Editor + CLI"
            } else {
                "CLI only"
            },
            false,
        ));
        let usage = match pinned.len() {
            0 => "No projects use this build".to_owned(),
            1 => "1 project uses this build".to_owned(),
            n => format!("{n} projects use this build"),
        };
        let mut actions = Vec::new();
        if let Some(folder) = engine.executable.parent().map(|p| p.to_owned()) {
            actions.push(
                button(l, Tone::Ghost, Size::Sm, Some(icons::FOLDER_OPEN), "Show")
                    .key(format!("engine-reveal-{index}"))
                    .on_press(move |s: &mut HubApp| s.intent(Intent::OpenPath(folder.clone()))),
            );
        }
        let v = engine.version.clone();
        let repairing = hub.releases.running("repair", &engine.version).is_some();
        actions.push(
            button(
                l,
                Tone::Secondary,
                Size::Sm,
                Some(icons::REFRESH_CW),
                if repairing { "Repairing…" } else { "Repair" },
            )
            .key(format!("engine-repair-{index}"))
            .on_press(move |s: &mut HubApp| {
                s.intent(Intent::Dialog(Dialog::RepairEngine(v.clone())))
            }),
        );
        if let Some(job) = hub.releases.running("uninstall", &engine.version) {
            actions.push(
                text("Uninstalling…")
                    .text_size(12.0)
                    .color(c.muted_foreground.0),
            );
            if job.cancellable {
                let id = job.id.clone();
                actions.push(
                    button(l, Tone::Secondary, Size::Sm, Some(icons::X), "Cancel")
                        .key(format!("engine-cancel-uninstall-{index}"))
                        .on_press(move |s: &mut HubApp| {
                            s.intent(Intent::CancelDownload(id.clone()))
                        }),
                );
            }
        } else {
            let e = engine.clone();
            let dialog = if engine.managed {
                Dialog::UninstallEngine(e)
            } else {
                Dialog::Confirm {
                    title: format!("Remove Ara {} from Hub?", engine.version),
                    detail: "This removes the build from Hub. The files stay on disk.".into(),
                    label: "Remove build".into(),
                    destructive: false,
                    action: Box::new(Intent::RemoveEngineBuild(engine.clone())),
                }
            };
            actions.push(
                button(
                    l,
                    Tone::Danger,
                    Size::Sm,
                    Some(icons::TRASH),
                    if engine.managed {
                        "Uninstall"
                    } else {
                        "Remove"
                    },
                )
                .key(format!("engine-remove-{index}"))
                .on_press(move |s: &mut HubApp| s.intent(Intent::Dialog(dialog.clone()))),
            );
        }
        let mut body = card(l)
            .pad(16.0)
            .gap(12.0)
            .child(
                row()
                    .gap(14.0)
                    .align(Align::Center)
                    .child(
                        el().size(44.0)
                            .radius(10.0)
                            .fill(c.muted.0)
                            .border(1.0, c.border.0)
                            .align(Align::Center)
                            .justify(Justify::Center)
                            .child(shell::brand_mark(s, Brand::Engine, 28.0)),
                    )
                    .child(
                        col().gap(4.0).grow(1.0).min_w(0.0).child(chips).child(
                            text(fmt::executable_directory(&engine.executable))
                                .text_size(11.0)
                                .family(mono())
                                .color(c.muted_foreground.0),
                        ),
                    )
                    .child(row().gap(6.0).align(Align::Center).children(actions)),
            )
            .child(
                row()
                    .gap(8.0)
                    .align(Align::Center)
                    .child(
                        stillkit::el::icon(icons::BOX)
                            .size(14.0)
                            .color(c.muted_foreground.0),
                    )
                    .child(text(usage).text_size(12.0).color(c.muted_foreground.0)),
            );
        if let Some(reason) = hub.engine_errors.get(&engine.version) {
            let key = format!("engine-error-{}", engine.version);
            if hub.banner_visible(&key) {
                body = body.child(banner(
                    l,
                    Status::Danger,
                    icons::CIRCLE_ALERT,
                    &format!("Can't remove: {reason}."),
                    vec![shell::dismiss(l, &format!("dismiss-{key}"), &key)],
                ));
            }
        } else if !open.is_empty() {
            let key = format!("engine-use-{}", engine.version);
            if hub.banner_visible(&key) {
                body = body.child(banner(
                    l,
                    Status::Info,
                    icons::INFO,
                    &format!(
                        "In use by {} (open). Close them before removing this build.",
                        open.join(", ")
                    ),
                    vec![shell::dismiss(l, &format!("dismiss-{key}"), &key)],
                ));
            }
        }
        out = out.child(body);
    }
    out
}

fn published(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let channel = super::settings::channel_select(s, "installs-channel");
    let refresh = button(
        l,
        Tone::Secondary,
        Size::Sm,
        Some(icons::REFRESH_CW),
        "Check for releases",
    )
    .key("refresh-engines")
    .on_press(|s: &mut HubApp| s.intent(Intent::CheckUpdates));
    let mut out = col().gap(12.0).child(section(
        l,
        "Published versions",
        None,
        vec![channel, refresh],
    ));
    if !hub.releases.status.is_empty() {
        out = out.child(
            text(hub.releases.status.clone())
                .text_size(12.0)
                .wrap(3)
                .color(c.muted_foreground.0),
        );
    }
    for release in &hub.releases.available {
        let version = release.version.clone();
        let channel = if release.channel == ara_update::Channel::Preview {
            ("Preview", Status::Warning)
        } else {
            ("Stable", Status::Success)
        };
        let size = release
            .assets
            .iter()
            .find(|a| a.name.ends_with("-windows-x64.zip") || a.name.contains("macos"))
            .map(|a| format!("{:.0} MB", a.size as f64 / 1_000_000.0));
        let mut head = row()
            .gap(10.0)
            .align(Align::Center)
            .child(shell::brand_mark(s, Brand::Engine, 20.0))
            .child(
                text(format!("Ara {version}"))
                    .font(TextRole::Headline)
                    .color(c.foreground.0),
            )
            .child(chip(l, channel.1, channel.0, false));
        if let Some(size) = size {
            head = head.child(text(size).text_size(11.5).color(c.muted_foreground.0));
        }
        head = head.child(el().grow(1.0));
        let job = hub.releases.running("engine", &release.version);
        if hub.engines.iter().any(|e| e.version == release.version) {
            head = head.child(chip(l, Status::Success, "Installed", true));
        } else if let Some(job) = job {
            if job.cancellable {
                let id = job.id.clone();
                head = head.child(
                    button(l, Tone::Secondary, Size::Sm, Some(icons::X), "Cancel")
                        .key(format!("cancel-{version}"))
                        .on_press(move |s: &mut HubApp| {
                            s.intent(Intent::CancelDownload(id.clone()))
                        }),
                );
            }
        } else {
            let v = version.clone();
            head = head.child(
                button(l, Tone::Primary, Size::Sm, Some(icons::DOWNLOAD), "Install")
                    .key(format!("install-{version}"))
                    .on_press(move |s: &mut HubApp| {
                        s.intent(Intent::Dialog(Dialog::InstallEngine(v.clone())))
                    }),
            );
        }
        let notes: String = release.notes.chars().take(260).collect();
        let v = version.clone();
        let mut body = card(l).pad(16.0).gap(10.0).child(head);
        if let Some(job) = job {
            body = body.child(transfer(s, job));
        }
        if !notes.trim().is_empty() {
            body = body.child(
                text(notes)
                    .text_size(12.0)
                    .wrap(4)
                    .color(c.muted_foreground.0),
            );
        }
        body = body.child(
            row().child(
                button(
                    l,
                    Tone::Ghost,
                    Size::Sm,
                    Some(icons::BOOK_OPEN),
                    "Release notes",
                )
                .key(format!("notes-{version}"))
                .on_press(move |s: &mut HubApp| {
                    s.intent(Intent::Dialog(Dialog::InstallEngine(v.clone())))
                }),
            ),
        );
        out = out.child(body);
    }
    out
}

fn pins(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let mut rows = Vec::new();
    for (i, project) in hub.projects.iter().enumerate() {
        if i > 0 {
            rows.push(divider(l));
        }
        let state = hub.state(&project.id);
        let locked = state != State::Closed || hub.busy.contains_key(&project.id);
        let mut r = row()
            .h(52.0)
            .pad_x(14.0)
            .gap(12.0)
            .align(Align::Center)
            .child(avatar(
                l,
                &project.name,
                ara_kit::art::seed(&project.id),
                28.0,
            ))
            .child(
                col()
                    .gap(2.0)
                    .grow(1.0)
                    .min_w(0.0)
                    .child(
                        text(project.name.clone())
                            .text_size(12.5)
                            .weight(540)
                            .color(c.foreground.0),
                    )
                    .child(
                        text(fmt::path(&project.path))
                            .text_size(11.0)
                            .family(mono())
                            .color(c.muted_foreground.0),
                    ),
            );
        if locked {
            if state != State::Closed {
                r = r.child(
                    text("Close to change")
                        .text_size(11.5)
                        .color(c.muted_foreground.0),
                );
            }
            r = r.child(chip(
                l,
                Status::Neutral,
                hub.busy
                    .get(&project.id)
                    .map_or(project.engine_version.as_str(), |b| b.label()),
                false,
            ));
        } else {
            if !hub
                .engines
                .iter()
                .any(|e| e.version == project.engine_version)
            {
                r = r.child(chip(
                    l,
                    Status::Warning,
                    &format!("{} missing", project.engine_version),
                    true,
                ));
            }
            r = r.child(pin_select(s, &project.id, &project.engine_version));
        }
        rows.push(r);
    }
    col()
        .gap(12.0)
        .child(section(
            l,
            "Project engine pins",
            Some("Each project opens with the build it pins. Close a project before changing it."),
            Vec::new(),
        ))
        .child(card(l).children(rows))
}

/// A project's engine dropdown; choosing confirms the change in a dialog first.
fn pin_select(s: &HubApp, project: &str, version: &str) -> El<HubApp> {
    let l = &s.look;
    let key = format!("project-engine-{project}");
    let open = s.select.as_deref() == Some(key.as_str());
    let k = key.clone();
    let mut trigger = select(l, Size::Sm, key.clone(), version)
        .w(160.0)
        .expanded(open)
        .on_press(move |s: &mut HubApp| s.toggle_select(&k));
    if open {
        let items = s
            .hub
            .engines
            .iter()
            .map(|e| {
                let (p, v) = (project.to_owned(), e.version.clone());
                menu_check(
                    l,
                    &format!("{key}/{}", e.version),
                    &e.version,
                    e.version == version,
                )
                .on_press(move |s: &mut HubApp| {
                    s.select = None;
                    s.intent(Intent::ChooseEngine {
                        project: p.clone(),
                        version: v.clone(),
                    });
                })
            })
            .collect();
        trigger = trigger.overlay(
            menu(menu_panel(l, items))
                .match_width()
                .on_dismiss(|s: &mut HubApp| s.select = None),
        );
    }
    trigger
}

/// A release job's progress: phase, percent, a bar, bytes and disk space.
pub(super) fn transfer(s: &HubApp, job: &Download) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    if job.kind == "uninstall" {
        return row()
            .gap(8.0)
            .align(Align::Center)
            .child(spinner(l, 14.0))
            .child(
                text(if job.cancellable {
                    "Preparing to remove this engine. You can still cancel."
                } else {
                    "Finishing removal of the engine files."
                })
                .text_size(12.0)
                .color(c.muted_foreground.0),
            );
    }
    let (received, total) = job
        .progress
        .as_ref()
        .map_or((0, 0), |p| (p.received, p.total));
    let fraction = if total > 0 {
        received as f32 / total as f32
    } else {
        0.0
    };
    let phase = match job.phase.as_str() {
        "verifying" => "Verifying the download…",
        "extracting" => "Extracting engine files…",
        "installing" => "Finishing installation…",
        "preparing" => "Preparing to remove the engine…",
        "removing" => "Removing engine files…",
        _ if total > 0 && received >= total => "Verifying and installing…",
        _ => "Downloading…",
    };
    let mut out = col()
        .gap(8.0)
        .child(
            row()
                .child(
                    text(phase)
                        .text_size(12.0)
                        .weight(540)
                        .color(c.foreground.0)
                        .grow(1.0),
                )
                .child(
                    text(format!("{}%", (fraction * 100.0).round()))
                        .text_size(12.0)
                        .family(mono())
                        .color(c.muted_foreground.0),
                ),
        )
        .child(progress(l, fraction, Status::Accent))
        .child(
            text(format!("{} of {}", bytes(received), bytes(total)))
                .text_size(11.5)
                .color(c.muted_foreground.0),
        );
    if let Some(disk) = &job.disk {
        out = out.child(
            text(format!(
                "{} available · {} needed",
                bytes(disk.available_bytes),
                bytes(disk.required_bytes)
            ))
            .text_size(11.5)
            .color(c.muted_foreground.0),
        );
    }
    out
}

/// Bytes for people: B, KB, MB, GB.
pub(super) fn bytes(bytes: u64) -> String {
    if bytes < 1_000 {
        return format!("{bytes} B");
    }
    if bytes < 1_000_000 {
        return format!("{:.1} KB", bytes as f64 / 1_000.0);
    }
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}
