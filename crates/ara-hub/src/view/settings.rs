use super::Page;
use crate::model::{Hub, Intent};
use ara_update::Channel;

pub(super) fn channels(p: &mut Page, hub: &Hub) {
    p.panel("segment", |p| {
        for (channel, label) in [(Channel::Stable, "Stable"), (Channel::Preview, "Preview")] {
            p.action(
                &format!("channel-{label}"),
                if hub.releases.channel == channel {
                    "seg seg-on"
                } else {
                    "seg"
                },
                "",
                label,
                Some(Intent::ReleaseChannel(channel)),
            );
        }
    });
}
pub(super) fn body(p: &mut Page, hub: &Hub) {
    p.panel("panel-card", |p| {
        p.text("section-title", "Ara Hub updates");
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
        p.text(
            "small muted",
            "Preview includes prereleases as well as stable builds.",
        );
        p.text("small muted", &hub.releases.status);
    });
    p.panel("panel-card", |p| {
        p.text("section-title", "Release source");
        p.text("section-copy", "GitHub repository (owner/name). ARA_RELEASE_REPO overrides this setting.");
        p.panel("row", |p| {
            p.field("release-repo", "fill", &hub.releases.repo, Intent::ReleaseRepo);
            p.action("save-release-repo", "compact", "check", "Save", Some(Intent::SaveReleaseRepo));
        });
        p.text("small muted", "Optional GITHUB_TOKEN is read from the service environment. All releases require an embedded trusted signature.");
    });
}
