//! Installs: engine builds side by side, and each project's pin.
use super::Page;
use crate::model::{Hub, Intent, State, text};

pub(super) fn body(p: &mut Page, hub: &Hub) {
    p.panel("section-head", |p| {
        p.panel("row", |p| {
            p.text("section-title fill", "Published engine versions");
            super::settings::channels(p, hub);
            p.action(
                "refresh-engines",
                "compact",
                "refresh-cw",
                "Check for releases",
                Some(Intent::CheckUpdates),
            );
        });
        p.text("small muted wrap", &hub.releases.status);
    });
    for release in &hub.releases.available {
        p.panel("panel-card", |p| {
            p.panel("row", |p| {
                p.icon("package", "accent icon-lg");
                p.text("engine-version fill", &format!("Ara {}", release.version));
                p.panel("pill", |p| {
                    p.text(
                        "",
                        if release.channel == ara_update::Channel::Preview {
                            "Preview"
                        } else {
                            "Stable"
                        },
                    )
                });
                if let Some(asset) = release
                    .assets
                    .iter()
                    .find(|a| a.name.ends_with("-windows-x64.zip"))
                {
                    p.text(
                        "small muted",
                        &format!("{:.0} MB", asset.size as f64 / 1_000_000.),
                    );
                }
                if hub.engines.iter().any(|e| e.version == release.version) {
                    p.text("small muted", "Installed");
                } else if let Some(job) = hub.releases.running("engine", &release.version) {
                    let percent = job
                        .progress
                        .as_ref()
                        .map_or(0, |p| p.received.saturating_mul(100) / p.total.max(1));
                    p.text("small accent", &format!("Installing · {percent}%"));
                    p.action(
                        &format!("cancel-{}", release.version),
                        "compact",
                        "x",
                        "Cancel",
                        Some(Intent::CancelDownload(job.id.clone())),
                    );
                } else {
                    p.action(
                        &format!("install-{}", release.version),
                        "primary compact",
                        "download",
                        "Install",
                        Some(Intent::InstallEngine(release.version.clone())),
                    );
                }
            });
            let excerpt: String = release.notes.chars().take(260).collect();
            if let Some(job) = hub.releases.running("engine", &release.version) {
                let percent = job
                    .progress
                    .as_ref()
                    .map_or(0, |p| p.received.saturating_mul(100) / p.total.max(1));
                p.panel("download-track", |p| {
                    p.panel_styled("download-fill", &format!("width:{percent}%;"), |_| {})
                });
            }
            p.text("small muted wrap", &excerpt);
            p.action(
                &format!("notes-{}", release.version),
                "compact",
                "book-open",
                "Release notes",
                Some(Intent::OpenPath(
                    format!(
                        "https://github.com/{}/releases/tag/engine-v{}",
                        hub.releases.repo, release.version
                    )
                    .into(),
                )),
            );
        });
    }
    p.panel("section-head", |p| {
        p.text("section-title", "Installed builds")
    });
    if hub.engines.is_empty() && hub.ready() {
        p.panel("empty", |p| {
            p.panel("empty-icon", |p| p.icon("hard-drive", "icon-lg"));
            p.text("empty-title", "No engine builds registered");
            p.text(
                "empty-copy",
                "Add the folder of an Ara installation, the one with ara.exe and engine.json.",
            );
            p.action(
                "empty-add-engine",
                "primary",
                "plus",
                "Add engine build",
                Some(Intent::AddEngine),
            );
        });
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
            .collect();
        p.panel("panel-card", |p| {
            p.panel("row", |p| {
                p.panel("engine-icon", |p| p.icon("package", "icon-lg accent"));
                p.panel("col fill", |p| {
                    p.panel("row", |p| {
                        p.text("engine-version", &format!("Ara {}", engine.version));
                        if hub.local_engine.as_ref() == Some(engine) {
                            p.panel("pill pill-accent", |p| p.text("", "This Hub"));
                        }
                        let label = if engine.editor.is_some() {
                            "Editor + CLI"
                        } else {
                            "CLI only"
                        };
                        p.panel("pill", |p| p.text("", label));
                    });
                    p.text(
                        "small muted",
                        &text::middle(
                            &text::path(engine.executable.parent().unwrap_or(&engine.executable)),
                            90,
                        ),
                    );
                });
                if let Some(folder) = engine.executable.parent() {
                    p.action(
                        &format!("engine-reveal-{index}"),
                        "",
                        "folder-open",
                        "Show",
                        Some(Intent::OpenPath(folder.to_owned())),
                    );
                }
                p.action(
                    &format!("engine-remove-{index}"),
                    "danger",
                    "trash",
                    if engine.managed {
                        "Uninstall"
                    } else {
                        "Remove"
                    },
                    Some(Intent::RemoveEngine(index)),
                );
            });
            let usage = match pinned.len() {
                0 => "No projects use this build.".to_owned(),
                1 => "1 project uses this build.".to_owned(),
                n => format!("{n} projects use this build."),
            };
            p.text("small muted", &usage);
            if let Some(reason) = hub.engine_errors.get(&engine.version) {
                let key = format!("engine-error-{}", engine.version);
                if hub.banner_visible(&key) {
                    p.panel("banner banner-error", |p| {
                        p.icon("circle-alert", "inherit");
                        p.text("banner-copy fill", &format!("Can't remove: {reason}."));
                        super::shell::dismiss_banner(
                            p,
                            &format!("dismiss-engine-error-{}", engine.version),
                            key,
                        );
                    });
                }
            } else if !open.is_empty() {
                let key = format!("engine-use-{}", engine.version);
                let names: Vec<_> = open.iter().map(|p| p.name.as_str()).collect();
                if hub.banner_visible(&key) {
                    p.panel("banner banner-warn", |p| {
                        p.icon("info", "inherit");
                        p.text(
                            "banner-copy fill",
                            &format!(
                                "In use by {} (open). Close them before removing this build.",
                                names.join(", ")
                            ),
                        );
                        super::shell::dismiss_banner(
                            p,
                            &format!("dismiss-engine-use-{}", engine.version),
                            key,
                        );
                    });
                }
            }
        });
    }
    if hub.projects.is_empty() {
        return;
    }
    p.panel("section-head", |p| {
        p.text("section-title", "Project engine pins");
        p.text(
            "section-copy",
            "Each project opens with the build it pins. Close a project before changing its pin.",
        );
    });
    p.panel("list", |p| {
        for (row, project) in hub.projects.iter().enumerate() {
            if row > 0 {
                p.leaf("divider");
            }
            let state = hub.state(&project.id);
            let locked = state != State::Closed || hub.busy.contains_key(&project.id);
            p.panel("pin-row", |p| {
                p.panel("col fill", |p| {
                    p.text("fg", &project.name);
                    p.text("tiny muted", &text::middle(&text::path(&project.path), 70));
                });
                if locked {
                    if state != State::Closed {
                        p.text("small muted", "Close to change");
                    }
                    p.panel("pill", |p| p.text("", &project.engine_version));
                    return;
                }
                p.panel("segment", |p| {
                    for engine in &hub.engines {
                        let class = if engine.version == project.engine_version {
                            "seg seg-on"
                        } else {
                            "seg"
                        };
                        p.action(
                            &format!("pin-{}-{}", project.id, engine.version),
                            class,
                            "",
                            &engine.version,
                            Some(Intent::Pin {
                                project: project.id.clone(),
                                version: engine.version.clone(),
                            }),
                        );
                    }
                });
                if !hub
                    .engines
                    .iter()
                    .any(|e| e.version == project.engine_version)
                {
                    p.panel("pill pill-warn", |p| {
                        p.text("", &format!("{} missing", project.engine_version));
                    });
                }
            });
        }
    });
}
