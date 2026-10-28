//! Product preferences lead; technical release-source controls are disclosed on demand.
use super::Page;
use crate::model::{DownloadPolicy, Hub, Intent};
use ara_update::Channel;

pub(super) fn channels(p: &mut Page, hub: &Hub) {
    let options = [(Channel::Stable, "Stable"), (Channel::Preview, "Preview")];
    p.dropdown(
        "release-channel",
        "Release channel",
        options
            .into_iter()
            .map(|(channel, label)| (label.into(), Intent::ReleaseChannel(channel)))
            .collect(),
        Some(Intent::ReleaseChannel(hub.releases.channel)),
    );
}
pub(super) fn body(p: &mut Page, hub: &Hub) {
    p.panel("panel-card", |p| {
        p.panel("row", |p| {
            p.image("brand-hub", "brand-icon");
            p.text("section-title", "Ara Hub updates");
        });
        p.text(
            "section-copy",
            &format!(
                "Installed Hub {}. Updates are checked on start and every four hours.",
                env!("CARGO_PKG_VERSION")
            ),
        );
        p.panel("row", |p| {
            channels(p, hub);
            p.action(
                "check-updates",
                "compact",
                "refresh-cw",
                if hub.releases.checking {
                    "Checking…"
                } else {
                    "Check for updates"
                },
                (!hub.releases.checking).then_some(Intent::CheckUpdates),
            );
        });
        let state = if hub.releases.checking {
            "Checking for updates"
        } else if hub.releases.prepared().is_some() {
            "Ready to restart — choose when to update"
        } else if hub.releases.hub.as_ref().is_some_and(|release| hub.releases.running("hub", &release.version).is_some()) {
            "Downloading — you can keep using Ara"
        } else if hub.releases.hub.is_some() {
            "An update is available"
        } else {
            "No newer Hub update is available"
        };
        p.text("fg", state);
        p.text("small muted wrap", &hub.releases.status);
        p.leaf("divider");
        p.panel("row", |p| {
            p.panel("col fill", |p| {
                p.text("fg", "Hub update downloads");
                p.text("small muted wrap", "Ara Hub always waits for you to choose Restart and update.");
            });
            p.dropdown("hub-update-downloads", "Hub update downloads", vec![
                ("Ask first".into(), Intent::HubUpdateDownloads(DownloadPolicy::Ask)),
                ("Download in background".into(), Intent::HubUpdateDownloads(DownloadPolicy::Background)),
            ], Some(Intent::HubUpdateDownloads(hub.prefs.hub_update_downloads)));
        });
        p.text("small muted wrap", "Preview includes prereleases as well as stable builds. Engine versions install separately and keep each project's selected version.");
    });
    advanced(p, hub, true);
}

/// Technical source controls never take space in the normal installation decision.
pub(super) fn advanced(p: &mut Page, hub: &Hub, source_controls: bool) {
    p.panel("col", |p| {
        p.panel("row", |p| p.action(if source_controls { "release-advanced" } else { "install-advanced" }, "ghost compact", if hub.advanced { "chevron-down" } else { "chevron-right" }, "Advanced", Some(Intent::ToggleAdvanced)));
        if !hub.advanced { return; }
        p.panel("install-summary", |p| {
            p.text("label", "RELEASE SOURCE AND VERIFICATION");
            p.text("small muted wrap", "Ara verifies the release signature against its trusted signing keys, then checks the downloaded file's SHA-256 checksum before installation.");
            if source_controls {
                p.text("small muted wrap", "Release repository (owner/name). ARA_RELEASE_REPO overrides this setting; an optional GITHUB_TOKEN is read from the service environment.");
                p.panel("row", |p| {
                    p.field("release-repo", "fill", &hub.releases.repo, Intent::ReleaseRepo);
                    p.action("save-release-repo", "compact", "check", "Save", Some(Intent::SaveReleaseRepo));
                });
            } else {
                p.text("small muted wrap", &format!("Source: {}", hub.releases.repo));
            }
        });
    });
}
