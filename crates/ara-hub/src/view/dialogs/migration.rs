//! Read-only move preview and explicit consent before an old project can open.
use super::*;
use crate::model::migration::{Operation, Phase};

pub(super) fn body(p: &mut Page, hub: &Hub, project: &str) {
    let Some(flow) = hub
        .migration
        .as_ref()
        .filter(|flow| flow.project == project)
    else {
        return;
    };
    let name = hub
        .project(project)
        .map_or("project", |project| project.name.as_str());
    heading_with_close(
        p,
        "folder",
        &format!("Update {name}'s folder layout"),
        flow.cancellable(),
    );
    if flow.phase == Phase::Decision {
        p.markup.push_str("<scroll id='migration-details-scroll' class='migration-details-scroll'><panel id='migration-details' class='migration-details'>");
    }
    match flow.phase {
        Phase::Checking => {
            p.panel("row", |p| {
                p.icon("loader-circle", "accent");
                p.text("dialog-copy", "Checking what needs to move…");
            });
            p.text(
                "dialog-copy muted",
                "This check only reads the project. Nothing moves until you choose Migrate.",
            );
        }
        Phase::Decision => {
            p.text("dialog-copy", "This project uses an older folder layout. Migrate it before opening in this preview. Asset IDs and references are preserved, and the migration is recorded as one undoable change.");
            if let Some(plan) = &flow.plan {
                p.text("label", "WHAT MOVES");
                p.markup.push_str("<scroll id='migration-moves-scroll' class='migration-moves-scroll'><panel id='migration-moves' class='migration-moves'>");
                let mut moves = 0;
                for operation in &plan.operations {
                    if let Operation::Move { from, to } = operation {
                        moves += 1;
                        p.text("dialog-copy mono", &format!("{from} → {to}"));
                    }
                }
                if moves == 0 {
                    p.text(
                        "dialog-copy muted",
                        "Project settings and the authored-content root are updated.",
                    );
                }
                let rewrites = plan
                    .operations
                    .iter()
                    .filter(|operation| matches!(operation, Operation::Rewrite { .. }))
                    .count();
                if rewrites > 0 {
                    p.text(
                        "small muted wrap",
                        &format!("References and settings are updated in {rewrites} files."),
                    );
                }
                p.markup.push_str("</panel></scroll>");
                p.panel("migration-preview", |p| {
                    p.text("label", "AFTER MIGRATION");
                    p.markup.push_str("<scroll id='migration-preview-scroll' class='migration-preview-scroll'><panel id='migration-tree' class='migration-tree'>");
                    for line in plan.preview_tree.iter().take(160) { p.text("migration-tree-line", line); }
                    if plan.preview_tree.len() > 160 { p.text("small muted wrap", &format!("…and {} more paths", plan.preview_tree.len() - 160)); }
                    p.markup.push_str("</panel></scroll>");
                });
                p.text("dialog-copy muted", &format!("Authored files live in {}/. Cancel keeps the project closed and leaves its files unchanged.", plan.authored_root.trim_end_matches('/')));
            }
        }
        Phase::Migrating | Phase::Cancelling => {
            let phase = flow
                .job
                .as_ref()
                .map(|job| job.phase.as_str())
                .filter(|phase| !phase.is_empty())
                .unwrap_or("Preparing migration…");
            p.panel("row", |p| {
                p.icon("loader-circle", "accent");
                p.text("dialog-copy", phase);
            });
            let progress = flow
                .job
                .as_ref()
                .map_or(0., |job| job.progress)
                .clamp(0., 1.);
            p.panel("download-track", |p| {
                p.panel_styled(
                    "download-fill",
                    &format!("width:{}%", progress * 100.),
                    |_| {},
                )
            });
            p.text("dialog-copy muted", if flow.phase == Phase::Cancelling { "Requesting cancellation. If saving has already begun, the migration will finish safely." } else if flow.cancellable() { "The project opens once its files and history have finished saving. Cancel is available before the commit begins." } else { "Finishing the project files and history. The project will open when everything has been saved." });
        }
    }
    if flow.phase == Phase::Decision {
        p.markup.push_str("</panel></scroll>");
    }
    p.panel("dialog-actions", |p| {
        p.leaf("fill");
        p.action(
            "migration-cancel",
            "",
            "",
            if flow.phase == Phase::Cancelling {
                "Cancelling…"
            } else {
                "Cancel"
            },
            flow.cancellable().then_some(Intent::CancelMigration),
        );
        if flow.phase == Phase::Decision {
            p.action(
                "migration-submit",
                "primary",
                "",
                "Migrate",
                Some(Intent::MigrateProject),
            );
        }
    });
}
