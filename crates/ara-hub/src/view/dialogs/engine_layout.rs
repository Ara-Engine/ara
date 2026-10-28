//! A compatible engine is an explicit project-pin decision, never a silent repair.
use super::*;
use crate::model::engine_layout::Phase;

pub(super) fn body(p: &mut Page, hub: &Hub, project: &str) {
    let Some(flow) = hub
        .engine_layout
        .as_ref()
        .filter(|flow| flow.project == project)
    else {
        return;
    };
    heading_with_close(
        p,
        "brand-engine",
        "Choose a compatible engine",
        flow.phase != Phase::Pinning,
    );
    p.text(
        "dialog-copy",
        "This project uses the `content/` layout and needs Ara preview 4 or newer",
    );
    if let Some(project) = hub.project(project) {
        p.panel("col", |p| {
            p.text("label", &project.name);
            p.text(
                "dialog-copy muted",
                &format!("Pinned engine: {}", project.engine_version),
            );
        });
    }
    match flow.phase {
        Phase::Checking => {
            p.panel("row", |p| {
                p.icon("loader-circle", "accent");
                p.text("dialog-copy", "Checking the current engine…");
            });
        }
        Phase::Pinning => {
            p.panel("row", |p| {
                p.icon("loader-circle", "accent");
                p.text("dialog-copy", "Saving your engine choice…");
            });
            p.text(
                "dialog-copy muted",
                "The project opens after the engine pin and its undo history have finished saving.",
            );
        }
        Phase::Decision | Phase::Installing => {
            p.text("dialog-copy",if flow.available {
                "Use the current engine for this project? This changes its engine pin in one recorded change, then continues opening it."
            } else {
                "Install the current engine first. After its signed download is verified, the project pin changes in one recorded change and the project opens."
            });
            p.text(
                "dialog-copy muted",
                &format!("Current engine: Ara {}", flow.version),
            );
        }
    }
    p.panel("dialog-actions", |p| {
        p.leaf("fill");
        p.action(
            "engine-layout-cancel",
            "",
            "",
            "Cancel",
            (flow.phase != Phase::Pinning).then_some(Intent::CancelEngineLayout),
        );
        if flow.phase == Phase::Decision {
            p.action(
                "engine-layout-use",
                "primary",
                "",
                if flow.available {
                    "Use current engine"
                } else {
                    "Install current engine"
                },
                Some(Intent::AcceptCurrentEngine),
            );
        }
    });
}
