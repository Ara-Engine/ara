//! Chrome shared by every page: sidebar, page header, offline banner, toasts.
use super::{Ctx, Page};
use crate::model::{Hub, Intent, Link, Pick, State, View};
use ara_diag::FixAction;

pub(super) fn sidebar(p: &mut Page, hub: &Hub) {
    p.panel("sidebar", |p| {
        p.panel("brand", |p| {
            let mark = if hub.prefs.light {
                "brand-wordmark-light"
            } else {
                "brand-wordmark"
            };
            p.image(mark, "wordmark");
            p.panel("brand-tag", |p| p.text("", "Hub"));
        });
        p.leaf("nav-gap");
        let agents: usize = hub
            .activity
            .iter()
            .map(|a| a.participants.iter().filter(|x| x.kind == "agent").count())
            .sum();
        for view in View::ALL {
            let count = match view {
                View::Projects => Some((hub.projects.len(), false)),
                View::Installs => Some((hub.engines.len(), false)),
                View::Agents if agents > 0 => Some((agents, true)),
                _ => None,
            };
            let class = if hub.view == view {
                "nav-item nav-on"
            } else {
                "nav-item"
            };
            p.button(
                &format!("nav-{view:?}").to_lowercase(),
                class,
                view.title(),
                Some(Intent::Navigate(view)),
                |p| {
                    p.icon(view.icon(), "inherit");
                    p.text("fill", view.title());
                    if let Some((count, live)) = count {
                        let class = if live {
                            "nav-count nav-live"
                        } else {
                            "nav-count"
                        };
                        p.panel(class, |p| p.text("", &count.to_string()));
                    }
                },
            );
        }
        p.leaf("fill");
        if !update_card(p, hub) && hub.view != View::Agents && agents == 0 {
            p.panel("side-card", |p| {
                p.panel("row", |p| {
                    p.icon("plug", "accent");
                    p.text("side-title", "Connect an AI agent");
                });
                p.text(
                    "side-copy",
                    "Install Ara's MCP server once. Agents then see every project you register here.",
                );
                p.action(
                    "side-agents",
                    "compact",
                    "",
                    "Set up an agent",
                    Some(Intent::Navigate(View::Agents)),
                );
            });
        }
        p.leaf("nav-gap");
        p.panel("status-row", |p| {
            let (dot, label) = match &hub.link {
                Link::Online => ("dot dot-on", "Hub service online"),
                Link::Connecting => ("dot dot-warn", "Connecting…"),
                Link::Offline { .. } => ("dot dot-off", "Hub service offline"),
            };
            p.leaf(dot);
            p.text("fill", label);
            let (icon, label) = if hub.prefs.light {
                ("moon", "Dark theme")
            } else {
                ("sun", "Light theme")
            };
            p.button(
                "theme",
                "ghost icon-compact",
                label,
                Some(Intent::ToggleTheme),
                |p| p.icon(icon, "inherit"),
            );
        });
        p.text(
            "tiny muted",
            &format!("Ara Hub · {}", ara_hub_service::ALPHA_VERSION),
        );
    });
}

pub(super) fn header(p: &mut Page, hub: &Hub, ctx: &Ctx) {
    p.panel("header", |p| {
        p.panel("col fill", |p| {
            p.text("title", hub.view.title());
            p.text("subtitle", &subtitle(hub));
        });
        match hub.view {
            View::Projects => {
                p.action(
                    "add-existing",
                    "",
                    "folder-open",
                    "Add existing",
                    Some(Intent::AddExisting),
                );
                p.action(
                    "new-project",
                    "primary",
                    "plus",
                    "New project",
                    Some(Intent::Navigate(View::Create)),
                );
            }
            View::Installs => p.action(
                "add-engine",
                "",
                "plus",
                "Add engine build",
                Some(Intent::AddEngine),
            ),
            _ => {}
        }
    });
    if hub.view == View::Projects {
        super::projects::toolbar(p, hub, ctx);
    }
}

fn subtitle(hub: &Hub) -> String {
    match hub.view {
        View::Projects => {
            let open = hub
                .projects
                .iter()
                .filter(|project| hub.state(&project.id) != State::Closed)
                .count();
            let count = hub.projects.len();
            if count == 0 {
                return "Create your first project, or add one you already have.".into();
            }
            let noun = if count == 1 { "project" } else { "projects" };
            if open > 0 {
                format!("{count} {noun} · {open} open")
            } else {
                format!("{count} {noun}")
            }
        }
        View::Create => "Start from a template or one of the real-art samples.".into(),
        View::Installs => {
            "Engine builds install side by side. Each project pins one version.".into()
        }
        View::Agents => "Connect agents once; they work in any project registered here.".into(),
        View::Learn => "Samples to explore and the docs that ship with Ara.".into(),
        View::Settings => "Hub updates, release channel and download source.".into(),
    }
}

/// The Hub update in the sidebar on every page but Settings (which shows it in full):
/// version, progress and the one action that moves it on. True when shown.
fn update_card(p: &mut Page, hub: &Hub) -> bool {
    let Some(release) = &hub.releases.hub else {
        return false;
    };
    let key = format!("update-{}", release.version);
    if hub.view == View::Settings || !hub.banner_visible(&key) {
        return false;
    }
    p.panel("side-card side-update", |p| {
        p.panel("row", |p| {
            p.icon("download", "accent");
            p.text("side-title fill", &format!("Ara Hub {}", release.version));
            dismiss_banner(p, "dismiss-update", key);
        });
        p.text("side-copy", &update_copy(hub, &release.version));
        update_action(p, hub, &release.version, "compact");
    });
    true
}

/// What an available Hub update is doing, in one line.
fn update_copy(hub: &Hub, version: &str) -> String {
    if let Some(job) = hub.releases.running("hub", version) {
        let progress = job
            .progress
            .as_ref()
            .map_or(0, |p| p.received.saturating_mul(100) / p.total.max(1));
        format!("Downloading verified update · {progress}%")
    } else if hub.releases.prepared().is_some() {
        "Ready. Open projects and the Hub service keep running.".into()
    } else {
        "Available to download.".into()
    }
}

/// The one action that moves a Hub update on: download, cancel or restart.
fn update_action(p: &mut Page, hub: &Hub, version: &str, size: &str) {
    if hub.releases.prepared().is_some() {
        p.action(
            "restart-update",
            &format!("primary {size}"),
            "refresh-cw",
            "Restart to update",
            Some(Intent::RestartUpdate),
        );
    } else if let Some(job) = hub.releases.running("hub", version) {
        p.action(
            "cancel-hub-update",
            size,
            "x",
            "Cancel",
            Some(Intent::CancelDownload(job.id.clone())),
        );
    } else {
        p.action(
            "download-update",
            size,
            "download",
            "Download update",
            Some(Intent::PrepareUpdate),
        );
    }
}

/// The Hub update in full, on Settings (elsewhere it is the sidebar card).
pub(super) fn update(p: &mut Page, hub: &Hub) {
    let Some(release) = &hub.releases.hub else {
        return;
    };
    let key = format!("update-{}", release.version);
    if hub.view != View::Settings || !hub.banner_visible(&key) {
        return;
    }
    p.panel("banner banner-info", |p| {
        p.icon("download", "accent");
        p.panel("col fill", |p| {
            p.text(
                "toast-title",
                &format!("Ara Hub {} is available", release.version),
            );
            p.text("small muted", &update_copy(hub, &release.version));
        });
        update_action(p, hub, &release.version, "compact");
        dismiss_banner(p, "dismiss-update", key);
    });
}

pub(super) fn offline(p: &mut Page, hub: &Hub) {
    let Link::Offline { message, log } = &hub.link else {
        return;
    };
    if !hub.banner_visible("offline") {
        return;
    }
    p.panel("banner banner-error", |p| {
        p.icon("circle-alert", "destructive icon-lg");
        p.panel("col fill", |p| {
            p.text("toast-title", "Can't reach the Ara Hub service");
            p.text("toast-copy", &format!("{message}. Retrying automatically."));
        });
        let log_exists = log.is_file();
        p.action(
            "open-start-log",
            "compact",
            "file",
            "Open hub-start.log",
            log_exists.then(|| Intent::OpenPath(log.clone())),
        );
        dismiss_banner(p, "dismiss-offline", "offline".into());
    });
}

pub(super) fn dismiss_banner(p: &mut Page, id: &str, key: String) {
    p.button(
        id,
        "ghost icon-compact",
        "Dismiss",
        Some(Intent::DismissBanner(key)),
        |p| p.icon("x", "inherit"),
    );
}

pub(super) fn toasts(p: &mut Page, hub: &Hub) {
    if hub.notices.is_empty() {
        return;
    }
    p.panel("toasts", |p| {
        for notice in &hub.notices {
            let class = if notice.is_error() {
                "toast toast-error"
            } else {
                "toast"
            };
            p.panel_key(&format!("notice-{}", notice.id), class, |p| {
                p.panel("row", |p| {
                    if notice.is_error() {
                        p.icon("circle-alert", "destructive");
                    } else {
                        p.icon("circle-check", "success");
                    }
                    p.text("toast-title fill", &notice.title);
                    p.button(
                        &format!("dismiss-{}", notice.id),
                        "ghost icon-compact",
                        "Dismiss",
                        Some(Intent::Dismiss(notice.id)),
                        |p| p.icon("x", "inherit"),
                    );
                });
                if let Some(detail) = &notice.detail {
                    p.text("toast-copy", detail);
                }
                let mut fix_index = 0;
                for diagnostic in &notice.diagnostics {
                    p.text(
                        "toast-copy",
                        &format!("{} {}", diagnostic.code, diagnostic.message),
                    );
                    for fix in &diagnostic.fixes {
                        let index = fix_index;
                        fix_index += 1;
                        p.panel("fix-row", |p| match &fix.action {
                            FixAction::Call { method, .. } if method.starts_with("hub.") => {
                                p.action(
                                    &format!("fix-{}-{index}", notice.id),
                                    "compact",
                                    "check",
                                    &fix.title,
                                    Some(Intent::Fix {
                                        notice: notice.id,
                                        fix: index,
                                    }),
                                );
                            }
                            FixAction::Cli { args } => {
                                p.text("toast-copy fill", &fix.title);
                                p.copy(
                                    hub.copied.as_deref(),
                                    &format!("fix-{}-{index}", notice.id),
                                    &format!("ara {}", args.join(" ")),
                                    false,
                                );
                            }
                            _ => p.text("toast-copy", &format!("Fix: {}", fix.title)),
                        });
                    }
                }
                if notice.undo.is_some() || notice.log.is_some() {
                    p.panel("fix-row", |p| {
                        if notice.undo.is_some() {
                            p.action(
                                &format!("undo-{}", notice.id),
                                "compact",
                                "",
                                "Undo",
                                Some(Intent::Fix {
                                    notice: notice.id,
                                    fix: usize::MAX,
                                }),
                            );
                        }
                        if let Some(log) = &notice.log {
                            p.action(
                                &format!("log-{}", notice.id),
                                "compact",
                                "file",
                                "Open log",
                                Some(Intent::OpenPath(log.clone())),
                            );
                        }
                    });
                }
            });
        }
    });
}

/// "Browse…" next to a path field.
pub(super) fn browse(p: &mut Page, id: &str, pick: Pick) {
    let intent = match pick {
        Pick::Destination => Intent::Browse,
        Pick::Register => Intent::AddExisting,
        Pick::Engine => Intent::AddEngine,
    };
    p.action(id, "", "folder-open", "Browse", Some(intent));
}
