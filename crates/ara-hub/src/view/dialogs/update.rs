//! Release notes, download choice and the explicit restart step.
use super::*;

pub(super) fn body(p: &mut Page, hub: &Hub) {
    let Some(release) = &hub.releases.hub else {
        heading(p, "brand-hub", "You're up to date");
        return;
    };
    heading(p, "brand-hub", &format!("Ara Hub {}", release.version));
    release_identity(p, release);
    p.panel("release-notes", |p| {
        p.text("label", "WHAT'S NEW");
        p.markup
            .push_str("<scroll id='release-notes-scroll' class='release-notes-scroll'>");
        p.text(
            "dialog-copy",
            if release.notes.trim().is_empty() {
                "This release includes improvements to Ara Hub."
            } else {
                &release.notes
            },
        );
        p.markup.push_str("</scroll>");
    });
    if hub.releases.starting("hub", &release.version) {
        pending(p, hub, "hub", &release.version);
    } else if let Some(job) = hub.releases.running("hub", &release.version) {
        transfer(p, job);
        p.text(
            "dialog-copy muted",
            "You can keep using Ara while this downloads. Restart is always your choice.",
        );
        p.panel("dialog-actions", |p| {
            p.action(
                "cancel-hub-update",
                "",
                "",
                "Cancel download",
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
    } else if hub.releases.prepared().is_some() {
        p.panel("verified", |p| {
            p.icon("circle-check", "success icon-lg");
            p.panel("col", |p| {
                p.text("fg", "Verified and ready to install");
                p.text("small muted", "The update has passed verification.");
            });
        });
        p.text(
            "dialog-copy muted",
            "Ara Hub will close and reopen. Your projects and the Hub service keep running.",
        );
        p.panel("dialog-actions", |p| {
            p.leaf("fill");
            p.action("dialog-later", "", "", "Later", Some(Intent::DismissDialog));
            p.action(
                "restart-update",
                "primary",
                "refresh-cw",
                "Restart and update",
                Some(Intent::RestartUpdate),
            );
        });
    } else {
        if hub.releases.jobs.iter().rev().any(|job| {
            job.kind == "hub" && job.version == release.version && job.state == "cancelled"
        }) {
            p.text(
                "small muted wrap",
                "Download cancelled. You can download this update again when ready.",
            );
        }
        p.text(
            "dialog-copy muted",
            "The update downloads first. Ara Hub asks you to restart when it is ready.",
        );
        crate::view::settings::advanced(p, hub, false);
        let skipped =
            hub.prefs.skipped_update.as_deref() == Some(updates::identity(release).as_str());
        if skipped {
            p.text(
                "small muted",
                "You skipped this release. You can still update here.",
            );
        }
        p.panel("dialog-actions", |p| {
            p.action(
                "skip-update",
                "ghost",
                "",
                "Skip this version",
                Some(Intent::SkipUpdate),
            );
            p.leaf("fill");
            p.action("dialog-later", "", "", "Later", Some(Intent::DismissDialog));
            p.action(
                "download-update",
                "primary",
                "download",
                "Update now",
                hub.releases
                    .can_self_update
                    .then_some(Intent::PrepareUpdate),
            );
        });
        if !hub.releases.can_self_update {
            p.text("small muted wrap", "This Hub is bundled with an engine. Use Ara Hub Setup to update the standalone Hub.");
        }
    }
}
