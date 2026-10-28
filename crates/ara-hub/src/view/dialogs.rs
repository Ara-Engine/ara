//! Decisions and errors share one focused, accessible dialog surface.
mod engine;
mod error;
mod migration;
mod update;

use super::{Ctx, Page};
use crate::model::{Dialog, Hub, Intent, updates};
use std::fmt::Write as _;

pub(super) fn body(p: &mut Page, hub: &Hub, ctx: &Ctx) {
    let Some(dialog) = &hub.dialog else { return };
    let width = (ctx.size[0] - 48.).min(560.);
    let height = ctx.size[1] - 48.;
    let preview = matches!(dialog, Dialog::Migration(_))
        && hub
            .migration
            .as_ref()
            .is_some_and(|flow| flow.phase == crate::model::migration::Phase::Decision);
    let class = if preview {
        "dialog-card migration-card"
    } else {
        "dialog-card"
    };
    let card_height = if preview {
        format!(";height:{}px", height.min(640.))
    } else {
        String::new()
    };
    let _ = write!(
        p.markup,
        "<modal id='hub-dialog' class='dialog-layer' label='Ara Hub dialog'><scroll id='dialog-frame' class='dialog-frame' style='width:{width}px;max-height:{height}px'><panel id='dialog-card' class='{class}' style='width:{width}px{card_height}'>"
    );
    match dialog {
        Dialog::Migration(project) => migration::body(p, hub, project),
        Dialog::Update => update::body(p, hub),
        Dialog::InstallEngine(version) => engine::install(p, hub, version, false),
        Dialog::RepairEngine(version) => engine::install(p, hub, version, true),
        Dialog::UninstallEngine(engine) => engine::uninstall(p, hub, engine),
        Dialog::Confirm {
            title,
            detail,
            label,
            destructive,
            ..
        } => {
            heading(p, if *destructive { "trash" } else { "refresh-cw" }, title);
            p.text("dialog-copy", detail);
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
                    if *destructive {
                        "destructive-button"
                    } else {
                        "primary"
                    },
                    "",
                    label,
                    Some(Intent::ConfirmDialog),
                );
            });
        }
        Dialog::Error(id) => error::body(p, hub, *id),
    }
    p.markup.push_str("</panel></scroll></modal>");
    if ctx.dialog_close_hint {
        p.markup.push_str("<tooltip id='dialog-close-tooltip' class='dialog-close-tooltip' anchor='dialog-close' placement='bottom-end'><text id='dialog-close-tooltip-copy' text='Close dialog' /></tooltip>");
    }
}

fn heading(p: &mut Page, icon: &str, title: &str) {
    heading_with_close(p, icon, title, true);
}

fn heading_with_close(p: &mut Page, icon: &str, title: &str, close: bool) {
    p.panel("dialog-heading", |p| {
        if matches!(icon, "brand-hub" | "brand-engine") {
            p.image(icon, "brand-icon-lg");
        } else {
            p.panel("dialog-icon", |p| p.icon(icon, "accent icon-lg"));
        }
        p.text("dialog-title fill", title);
        p.button(
            "dialog-close",
            "ghost icon-compact",
            "Close dialog",
            close.then_some(Intent::DismissDialog),
            |p| p.icon("x", "inherit"),
        );
    });
}

fn pending(p: &mut Page, hub: &Hub, kind: &str, version: &str) {
    let cancelling = hub.releases.cancelling(kind, version);
    p.panel("row", |p| {
        p.icon("loader-circle", "accent");
        p.text(
            "dialog-copy",
            if cancelling {
                "Cancelling the request…"
            } else {
                "Preparing your download…"
            },
        );
    });
    p.panel("dialog-actions", |p| {
        p.action(
            "cancel-pending-transfer",
            "",
            "",
            "Cancel",
            (!cancelling).then(|| Intent::CancelPending {
                kind: kind.into(),
                version: version.into(),
            }),
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
}

fn release_identity(p: &mut Page, release: &ara_update::Manifest) {
    let channel = if release.channel == ara_update::Channel::Preview {
        "Preview"
    } else {
        "Stable"
    };
    let label = match &release.build {
        Some(build) => format!(
            "{channel} · build {}",
            build.commit.chars().take(8).collect::<String>()
        ),
        None => channel.into(),
    };
    p.text("dialog-eyebrow", &label);
}

pub(super) fn transfer(p: &mut Page, job: &crate::model::updates::Download) {
    if job.kind == "uninstall" {
        p.panel("row", |p| {
            p.icon("loader-circle", "accent");
            p.text(
                "small muted wrap",
                if job.cancellable {
                    "Preparing to remove this engine. You can still cancel."
                } else {
                    "Finishing removal of the engine files."
                },
            );
        });
        return;
    }
    let (received, total) = job
        .progress
        .as_ref()
        .map_or((0, 0), |value| (value.received, value.total));
    let percent = received
        .saturating_mul(100)
        .checked_div(total)
        .unwrap_or(0)
        .min(100);
    p.panel("transfer", |p| {
        p.panel("row", |p| {
            let phase = match job.phase.as_str() {
                "verifying" => "Verifying the download…",
                "extracting" => "Extracting engine files…",
                "installing" => "Finishing installation…",
                "preparing" => "Preparing to remove the engine…",
                "removing" => "Removing engine files…",
                _ if total > 0 && received >= total => "Verifying and installing…",
                _ => "Downloading…",
            };
            p.text("fg fill", phase);
            p.text("small muted", &format!("{percent}%"));
        });
        p.panel("download-track", |p| {
            p.panel_styled("download-fill", &format!("width:{percent}%"), |_| {})
        });
        p.text(
            "small muted",
            &format!("{} of {}", bytes(received), bytes(total)),
        );
        if let Some(disk) = &job.disk {
            p.text(
                "small muted",
                &format!(
                    "{} available · {} needed",
                    bytes(disk.available_bytes),
                    bytes(disk.required_bytes)
                ),
            );
        }
    });
}

fn bytes(bytes: u64) -> String {
    if bytes < 1_000 {
        return format!("{bytes} B");
    }
    if bytes < 1_000_000 {
        return format!("{:.1} KB", bytes as f64 / 1_000.);
    }
    let mb = bytes as f64 / 1_000_000.;
    if mb >= 1000. {
        format!("{:.1} GB", mb / 1000.)
    } else {
        format!("{mb:.0} MB")
    }
}
