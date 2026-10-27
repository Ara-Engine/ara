//! Engine install, repair and exact-installation removal decisions.
use super::*;

pub(super) fn install(p: &mut Page, hub: &Hub, version: &str, repair: bool) {
    heading(
        p,
        "package",
        &format!(
            "{} Ara {version}",
            if repair { "Repair" } else { "Install" }
        ),
    );
    let plan = hub.releases.plans.get(version);
    let destination = plan.map(|plan| plan.destination.clone()).or_else(|| {
        hub.releases
            .install_root
            .as_ref()
            .map(|root| root.join(version))
    });
    if let Some(destination) = destination {
        p.panel("col", |p| {
            p.text("label", "INSTALL LOCATION");
            p.text("dialog-copy muted", &crate::model::text::path(&destination));
        });
    }
    let kind = if repair { "repair" } else { "engine" };
    if hub.releases.starting(kind, version) {
        pending(p, hub, kind, version);
        return;
    }
    if let Some(job) = hub.releases.running(kind, version) {
        transfer(p, job);
        p.text(
            "dialog-copy muted",
            "Your other engine versions remain available while this installs.",
        );
        p.panel("dialog-actions", |p| {
            p.action(
                "cancel-engine-install",
                "",
                "",
                "Cancel installation",
                job.cancellable
                    .then(|| Intent::CancelDownload(job.id.clone())),
            );
            p.leaf("fill");
            p.action(
                "dialog-later",
                "primary",
                "",
                "Continue in background",
                Some(Intent::DismissDialog),
            );
        });
        return;
    }
    if (!repair && hub.engines.iter().any(|engine| engine.version == version))
        || (repair
            && hub.releases.jobs.iter().any(|job| {
                job.kind == "repair" && job.version == version && job.state == "complete"
            }))
    {
        p.panel("verified", |p| {
            p.icon("circle-check", "success icon-lg");
            p.text(
                "fg",
                if repair {
                    "Repaired and ready for your projects"
                } else {
                    "Installed and ready for your projects"
                },
            );
        });
        p.panel("dialog-actions", |p| {
            p.leaf("fill");
            p.action(
                "dialog-done",
                "primary",
                "",
                "Done",
                Some(Intent::DismissDialog),
            );
        });
        return;
    }
    p.text("dialog-copy", if repair { "Download a fresh copy of the editor and tools. Your projects and engine version pins stay unchanged. Close projects using this version before repairing." } else { "Install the editor and tools side by side. Existing projects keep their selected engine version." });
    if let Some(release) = hub
        .releases
        .available
        .iter()
        .find(|release| release.version == version)
    {
        release_identity(p, release);
        if !release.notes.is_empty() {
            p.markup
                .push_str("<scroll id='install-notes-scroll' class='install-notes-scroll'>");
            p.text("dialog-copy muted", &release.notes);
            p.markup.push_str("</scroll>");
        }
    } else if let Some(channel) = plan.and_then(|plan| plan.channel) {
        p.text(
            "dialog-eyebrow",
            if channel == ara_update::Channel::Preview {
                "Preview"
            } else {
                "Stable"
            },
        );
    }
    if let Some(plan) = plan {
        p.panel("install-summary", |p| {
            p.panel("row", |p| {
                p.text("small muted fill", "Download");
                p.text("fg", &bytes(plan.download_bytes));
            });
            p.panel("row", |p| {
                p.text("small muted fill", "Estimated required space");
                p.text("fg", &bytes(plan.required_bytes));
            });
            p.panel("row", |p| {
                p.text("small muted fill", "Available space");
                p.text("fg", &bytes(plan.available_bytes));
            });
        });
    } else {
        p.text(
            "small muted wrap",
            "Checking the installation location and available space…",
        );
    }
    let in_use = repair
        && hub.projects.iter().any(|project| {
            project.engine_version == version
                && hub.state(&project.id) != crate::model::State::Closed
        });
    if in_use {
        p.text(
            "problem-copy",
            "Close projects using this engine before repairing it.",
        );
    }
    if plan.is_some_and(|plan| plan.available_bytes < plan.required_bytes) {
        p.text(
            "problem-copy",
            "Free space on the installation drive, then try again.",
        );
    }
    crate::view::settings::advanced(p, hub, false);
    let failed = hub.releases.jobs.iter().any(|job| {
        job.kind == kind
            && job.version == version
            && matches!(job.state.as_str(), "failed" | "cancelled")
    });
    if hub
        .releases
        .jobs
        .iter()
        .any(|job| job.kind == kind && job.version == version && job.state == "cancelled")
    {
        p.text(
            "small muted wrap",
            "Installation cancelled. Retry when you're ready.",
        );
    }
    p.panel("dialog-actions", |p| {
        p.leaf("fill");
        p.action(
            "dialog-cancel",
            "",
            "",
            "Cancel",
            Some(Intent::DismissDialog),
        );
        let label = if repair {
            if failed {
                "Retry repair"
            } else {
                "Repair engine"
            }
        } else if failed {
            "Retry installation"
        } else {
            "Install engine"
        };
        let intent = if repair {
            Intent::RepairEngine(version.into())
        } else {
            Intent::InstallEngine(version.into())
        };
        p.action(
            "confirm-install-engine",
            "primary",
            "download",
            label,
            (plan.is_some_and(|plan| plan.available_bytes >= plan.required_bytes) && !in_use)
                .then_some(intent),
        );
    });
}

pub(super) fn uninstall(p: &mut Page, hub: &Hub, engine: &crate::model::Engine) {
    heading(p, "trash", &format!("Uninstall Ara {}?", engine.version));
    p.text(
        "dialog-copy",
        "This removes the installed editor and tools. Your project files stay on disk.",
    );
    let summary = hub.releases.removal(&engine.version);
    p.panel("install-summary", |p| {
        p.text("label", "INSTALLATION TO REMOVE");
        let destination = summary.map_or_else(
            || crate::model::text::executable_directory(&engine.executable),
            |summary| crate::model::text::path(&summary.destination),
        );
        p.text("dialog-copy", &destination);
        if let Some(summary) = summary {
            p.panel("row", |p| {
                p.text("small muted fill", "Approximate space freed");
                p.text("fg", &bytes(summary.bytes));
            });
        } else {
            p.panel("row", |p| {
                p.icon("loader-circle", "muted");
                p.text("small muted", "Calculating space…");
            });
        }
    });
    let projects: Vec<_> = hub
        .projects
        .iter()
        .filter(|project| project.engine_version == engine.version)
        .collect();
    let in_use = projects
        .iter()
        .any(|project| hub.state(&project.id) != crate::model::State::Closed);
    if projects.is_empty() {
        p.text(
            "dialog-copy muted",
            "No registered projects use this engine version.",
        );
    } else {
        p.text("label", "PROJECTS USING THIS VERSION");
        p.markup
            .push_str("<scroll id='uninstall-projects-scroll' class='install-notes-scroll'>");
        p.text(
            "dialog-copy",
            &projects
                .iter()
                .map(|project| project.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        p.markup.push_str("</scroll>");
        p.text(
            "dialog-copy muted",
            "These projects keep their engine pin and will need this version installed again.",
        );
    }
    if in_use {
        p.text(
            "problem-copy",
            "Close projects using this engine before uninstalling it.",
        );
    }
    p.panel("dialog-actions", |p| {
        p.leaf("fill");
        p.action(
            "dialog-cancel",
            "",
            "",
            "Cancel",
            Some(Intent::DismissDialog),
        );
        p.action(
            "dialog-confirm",
            "destructive-button",
            "trash",
            "Uninstall engine",
            (!in_use && summary.is_some()).then_some(Intent::ConfirmDialog),
        );
    });
}
