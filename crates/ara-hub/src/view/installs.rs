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
                    p.text(
                        "small accent",
                        &format!("Installing Ara {}", release.version),
                    );
                    p.action(
                        &format!("cancel-{}", release.version),
                        "compact",
                        "x",
                        "Cancel",
                        job.cancellable
                            .then(|| Intent::CancelDownload(job.id.clone())),
                    );
                } else {
                    p.action(
                        &format!("install-{}", release.version),
                        "primary compact",
                        "download",
                        "Install",
                        Some(Intent::Dialog(crate::model::Dialog::InstallEngine(
                            release.version.clone(),
                        ))),
                    );
                }
            });
            let excerpt: String = release.notes.chars().take(260).collect();
            if let Some(job) = hub.releases.running("engine", &release.version) {
                super::dialogs::transfer(p, job);
            }
            p.text("small muted wrap", &excerpt);
            // A button, not a full-width bar: rows keep it to its own width.
            p.panel("row", |p| {
                p.action(
                    &format!("notes-{}", release.version),
                    "compact",
                    "book-open",
                    "Release notes",
                    Some(Intent::Dialog(crate::model::Dialog::InstallEngine(
                        release.version.clone(),
                    ))),
                );
            });
        });
    }
    for job in
        hub.releases.jobs.iter().filter(|job| {
            matches!(job.kind.as_str(), "uninstall" | "repair") && job.state == "running"
        })
    {
        p.panel("panel-card", |p| {
            p.text(
                "engine-version",
                &format!(
                    "{} Ara {}",
                    if job.kind == "repair" {
                        "Repairing"
                    } else {
                        "Uninstalling"
                    },
                    job.version
                ),
            );
            super::dialogs::transfer(p, job);
            p.panel("row", |p| {
                p.leaf("fill");
                if job.cancellable {
                    p.action(
                        &format!("cancel-uninstall-{}", job.id),
                        "compact",
                        "x",
                        "Cancel",
                        Some(Intent::CancelDownload(job.id.clone())),
                    );
                }
            });
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
                        &text::middle(&text::executable_directory(&engine.executable), 90),
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
                    &format!("engine-repair-{index}"),
                    "",
                    "refresh-cw",
                    if hub.releases.running("repair", &engine.version).is_some() {
                        "Repairing…"
                    } else {
                        "Repair"
                    },
                    Some(Intent::Dialog(crate::model::Dialog::RepairEngine(
                        engine.version.clone(),
                    ))),
                );
                if let Some(job) = hub.releases.running("uninstall", &engine.version) {
                    p.text("small muted", "Uninstalling...");
                    if job.cancellable {
                        p.action(
                            &format!("engine-cancel-uninstall-{index}"),
                            "compact",
                            "x",
                            "Cancel",
                            Some(Intent::CancelDownload(job.id.clone())),
                        );
                    }
                } else {
                    p.action(
                        &format!("engine-remove-{index}"),
                        "danger",
                        "trash",
                        if engine.managed {
                            "Uninstall"
                        } else {
                            "Remove"
                        },
                        Some(Intent::Dialog(if engine.managed {
                            crate::model::Dialog::UninstallEngine(engine.clone())
                        } else {
                            crate::model::Dialog::Confirm {
                                title: format!("Remove Ara {} from Hub?", engine.version),
                                detail: "This removes the build from Hub. The files stay on disk."
                                    .into(),
                                label: "Remove build".into(),
                                destructive: false,
                                action: Box::new(Intent::RemoveEngineBuild(engine.clone())),
                            }
                        })),
                    );
                }
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
                    p.panel("pill", |p| {
                        p.text(
                            "",
                            hub.busy
                                .get(&project.id)
                                .map_or(project.engine_version.as_str(), |busy| busy.label()),
                        )
                    });
                    return;
                }
                let choose = |version: &str| Intent::ChooseEngine {
                    project: project.id.clone(),
                    version: version.into(),
                };
                p.dropdown(
                    &format!("project-engine-{}", project.id),
                    "Engine version",
                    hub.engines
                        .iter()
                        .map(|engine| (engine.version.clone(), choose(&engine.version)))
                        .collect(),
                    Some(choose(&project.engine_version)),
                );
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
