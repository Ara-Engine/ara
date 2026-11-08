//! Settings: appearance, Hub updates, and technical release-source controls disclosed only
//! on request (Advanced).
use ara_kit::icons;
use ara_kit::ui::*;
use ara_update::Channel;
use stillkit::TextRole;
use stillkit::el::{Align, El, col, el, icon, menu, row, text};

use super::images::Brand;
use super::{HubApp, shell};
use crate::model::{Dialog, DownloadPolicy, Intent};

pub(super) fn body(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    col()
        .gap(16.0)
        .max_w(760.0)
        .child(appearance(s))
        .child(updates(s))
        .child(advanced(s, true))
        .child(el().h(8.0))
        .child(
            text(format!(
                "Ara Hub {} · {}",
                env!("CARGO_PKG_VERSION"),
                crate::build_identity().map_or_else(
                    || "development build".to_owned(),
                    |b| format!("build {}", b.commit.chars().take(8).collect::<String>())
                )
            ))
            .text_size(11.0)
            .family(mono())
            .color(l.c.muted_foreground.0),
        )
}

/// One setting: a title and explanation on the left, its control on the right.
fn setting(l: &ara_kit::look::Look, title: &str, detail: &str, control: El<HubApp>) -> El<HubApp> {
    row()
        .gap(16.0)
        .align(Align::Center)
        .child(
            col()
                .gap(3.0)
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(title.to_owned())
                        .text_size(12.5)
                        .weight(560)
                        .color(l.c.foreground.0),
                )
                .child(
                    text(detail.to_owned())
                        .text_size(11.5)
                        .wrap(3)
                        .color(l.c.muted_foreground.0),
                ),
        )
        .child(control)
}

fn appearance(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let theme = segmented(
        l,
        &[
            ("theme-dark", Some(icons::MOON), "Dark"),
            ("theme-light", Some(icons::SUN), "Light"),
        ],
        if s.hub.prefs.light { 1 } else { 0 },
        |i| {
            move |s: &mut HubApp| {
                if (i == 1) != s.hub.prefs.light {
                    s.intent(Intent::ToggleTheme);
                }
            }
        },
    );
    card(l)
        .pad(18.0)
        .gap(14.0)
        .child(
            text("Appearance")
                .font(TextRole::Title3)
                .color(l.c.foreground.0),
        )
        .child(setting(
            l,
            "Theme",
            "The Hub and editor share Ara's dark and light themes.",
            theme,
        ))
}

fn updates(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let state = if hub.releases.checking {
        "Checking for updates…"
    } else if hub.releases.prepared().is_some() {
        "Ready to restart: choose when to update"
    } else if hub
        .releases
        .hub
        .as_ref()
        .is_some_and(|r| hub.releases.running("hub", &r.version).is_some())
    {
        "Downloading: you can keep using Ara"
    } else if hub.releases.hub.is_some() {
        "An update is available"
    } else {
        "Ara Hub is up to date"
    };
    let check = enabled(
        button(
            l,
            Tone::Secondary,
            Size::Sm,
            Some(icons::REFRESH_CW),
            if hub.releases.checking {
                "Checking…"
            } else {
                "Check for updates"
            },
        )
        .key("check-updates")
        .on_press(|s: &mut HubApp| s.intent(Intent::CheckUpdates)),
        !hub.releases.checking,
    );
    let mut status = row()
        .gap(10.0)
        .align(Align::Center)
        .child(shell::brand_mark(s, Brand::Hub, 28.0))
        .child(
            col()
                .gap(2.0)
                .grow(1.0)
                .child(
                    text(state)
                        .text_size(12.5)
                        .weight(560)
                        .color(c.foreground.0),
                )
                .child(
                    text(format!(
                        "Installed Hub {}. Checked on start and every four hours.",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .text_size(11.5)
                    .color(c.muted_foreground.0),
                ),
        );
    if let Some(release) = &hub.releases.hub {
        status = status.child(
            button(
                l,
                Tone::Primary,
                Size::Sm,
                Some(icons::SPARKLES),
                &format!("View {}", release.version),
            )
            .key("review-update")
            .on_press(|s: &mut HubApp| s.intent(Intent::Dialog(Dialog::Update))),
        );
    }
    status = status.child(check);
    let policy = download_select(s);
    let mut panel = card(l)
        .pad(18.0)
        .gap(14.0)
        .child(text("Updates").font(TextRole::Title3).color(c.foreground.0))
        .child(status);
    if !hub.releases.status.is_empty() {
        panel = panel.child(
            text(hub.releases.status.clone())
                .text_size(11.5)
                .wrap(3)
                .color(c.muted_foreground.0),
        );
    }
    panel
        .child(divider(l))
        .child(setting(l, "Release channel", "Preview includes prereleases as well as stable builds. Engine versions install separately.", channel_select(s, "release-channel")))
        .child(setting(l, "Hub update downloads", "The Hub always waits for you to choose Restart and update.", policy))
}

/// The release channel dropdown (Settings and Installs share it).
pub(super) fn channel_select(s: &HubApp, key: &str) -> El<HubApp> {
    let l = &s.look;
    let current = if s.hub.releases.channel == Channel::Preview {
        "Preview"
    } else {
        "Stable"
    };
    dropdown(
        s,
        key,
        current,
        &[
            ("Stable", Intent::ReleaseChannel(Channel::Stable)),
            ("Preview", Intent::ReleaseChannel(Channel::Preview)),
        ],
        120.0,
        l,
    )
}

fn download_select(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let ask = s.hub.prefs.hub_update_downloads == DownloadPolicy::Ask;
    dropdown(
        s,
        "hub-update-downloads",
        if ask {
            "Ask first"
        } else {
            "Download in background"
        },
        &[
            ("Ask first", Intent::HubUpdateDownloads(DownloadPolicy::Ask)),
            (
                "Download in background",
                Intent::HubUpdateDownloads(DownloadPolicy::Background),
            ),
        ],
        190.0,
        l,
    )
}

/// A dropdown of labelled intents; the current label is checked.
fn dropdown(
    s: &HubApp,
    key: &str,
    current: &str,
    options: &[(&str, Intent)],
    width: f32,
    l: &ara_kit::look::Look,
) -> El<HubApp> {
    let open = s.select.as_deref() == Some(key);
    let k = key.to_owned();
    let mut trigger = select(l, Size::Sm, key.to_owned(), current)
        .w(width)
        .expanded(open)
        .on_press(move |s: &mut HubApp| s.toggle_select(&k));
    if open {
        let items = options
            .iter()
            .map(|(label, intent)| {
                let intent = intent.clone();
                menu_check(l, &format!("{key}/{label}"), label, *label == current).on_press(
                    move |s: &mut HubApp| {
                        s.select = None;
                        s.intent(intent.clone());
                    },
                )
            })
            .collect();
        trigger = trigger.overlay(
            menu(menu_panel(l, items))
                .match_width()
                .on_dismiss(|s: &mut HubApp| s.select = None),
        );
    }
    trigger
}

/// Technical source controls, collapsed by default; shared with the install dialogs.
pub(super) fn advanced(s: &HubApp, source_controls: bool) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let toggle = button(
        l,
        Tone::Ghost,
        Size::Sm,
        Some(if hub.advanced {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        }),
        "Advanced",
    )
    .key(if source_controls {
        "release-advanced"
    } else {
        "install-advanced"
    })
    .expanded(hub.advanced)
    .on_press(|s: &mut HubApp| s.intent(Intent::ToggleAdvanced));
    let mut out = col().gap(10.0).child(row().child(toggle));
    if !hub.advanced {
        return out;
    }
    let mut body = card(l).pad(14.0).gap(10.0).fill(c.muted.0)
        .child(row().gap(8.0).align(Align::Center).child(icon(icons::SHIELD_CHECK).size(14.0).color(c.success_foreground.0)).child(label(l, "Release source and verification")))
        .child(text("Ara verifies the release signature against its trusted signing keys, then checks the download's SHA-256 checksum before installation.").text_size(11.5).wrap(4).color(c.muted_foreground.0));
    if source_controls {
        body = body
            .child(text("Release repository (owner/name). ARA_RELEASE_REPO overrides this setting; an optional GITHUB_TOKEN is read from the service environment.").text_size(11.5).wrap(4).color(c.muted_foreground.0))
            .child(row().gap(6.0)
                .child(field(l, Size::Sm, "release-repo", Some(icons::GIT_BRANCH), "owner/name", &hub.releases.repo, |f| f.on_input(|s: &mut HubApp, t| s.intent(Intent::ReleaseRepo(t.into()))).on_submit(|s: &mut HubApp, _| s.intent(Intent::SaveReleaseRepo))).grow(1.0))
                .child(button(l, Tone::Secondary, Size::Sm, Some(icons::CHECK), "Save").key("save-release-repo").on_press(|s: &mut HubApp| s.intent(Intent::SaveReleaseRepo))));
    } else {
        body = body.child(
            text(format!("Source: {}", hub.releases.repo))
                .text_size(11.5)
                .family(mono())
                .color(c.muted_foreground.0),
        );
    }
    out = out.child(body);
    out
}
