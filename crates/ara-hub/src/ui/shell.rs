//! Chrome around every page: the title bar, the sidebar, the scrolling page with its header
//! and banners, toasts, and the code blocks and copy buttons pages share.
use ara_diag::FixAction;
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{
    Align, Axis, El, Justify, col, el, icon, image, row, text, toasts as toast_stack,
};

use super::images::{Brand, Pic};
use super::{HubApp, agents, create, installs, learn, projects, settings};
use crate::model::{Intent, Link, State, View};

pub(super) fn title_bar(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let brand = row()
        .gap(8.0)
        .align(Align::Center)
        .child(brand_mark(s, Brand::Hub, 18.0))
        .child(
            text("Ara Hub")
                .text_size(12.5)
                .weight(600)
                .color(c.foreground.0),
        );
    // The launcher: a quiet field-shaped button that opens the palette.
    let ring = c.ring.0;
    let launcher = row()
        .key("launcher")
        .w(340.0)
        .h(26.0)
        .pad_x(8.0)
        .gap(8.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .fill(c.muted.0)
        .border(1.0, c.border.0)
        .color(c.muted_foreground.0)
        .cascade_ink()
        .label("Search or jump to")
        .hovered(move |e| e.border(1.0, ring.with_alpha(0.35)))
        .on_press_with(|s: &mut HubApp, tx| {
            s.palette.open();
            tx.focus("palette/query");
        })
        .child(icon(icons::SEARCH).size(13.0))
        .child(
            text("Search projects, templates and actions")
                .text_size(12.0)
                .grow(1.0),
        )
        .child(kbd(
            l,
            if cfg!(target_os = "macos") {
                "⌘K"
            } else {
                "Ctrl K"
            },
        ));
    let mut trail = Vec::new();
    if let Some(label) = update_label(s) {
        trail.push(
            button(l, Tone::Secondary, Size::Sm, Some(icons::DOWNLOAD), label)
                .key("update-available")
                .color(c.info_foreground.0)
                .on_press(|s: &mut HubApp| s.intent(Intent::Dialog(crate::model::Dialog::Update))),
        );
    }
    let (theme_icon, theme_label) = if s.hub.prefs.light {
        (icons::MOON, "Dark theme")
    } else {
        (icons::SUN, "Light theme")
    };
    trail.push(
        icon_button(l, Tone::Ghost, Size::Sm, theme_icon, theme_label)
            .key("theme")
            .on_press(|s: &mut HubApp| s.intent(Intent::ToggleTheme)),
    );
    title_bar_el(
        l,
        s.frame.inset,
        vec![brand],
        vec![launcher],
        trail,
        s.frame.controls,
    )
}

fn title_bar_el(
    l: &ara_kit::look::Look,
    inset: f32,
    lead: Vec<El<HubApp>>,
    middle: Vec<El<HubApp>>,
    trail: Vec<El<HubApp>>,
    controls: Option<bool>,
) -> El<HubApp> {
    ara_kit::ui::title_bar(l, inset, lead, middle, trail, controls)
}

/// A brand mark `size` design px square (rendered at 2x for any display).
pub(super) fn brand_mark(s: &HubApp, brand: Brand, size: f32) -> El<HubApp> {
    let px = (size * 3.0) as u32;
    match s.images.get(&Pic::Brand(brand, px)) {
        Some(img) => image(img).size(size),
        None => el().size(size),
    }
}

/// The update indicator's label, when a Hub update is offered and not skipped.
fn update_label(s: &HubApp) -> Option<&'static str> {
    let hub = &s.hub;
    let release = hub.releases.hub.as_ref()?;
    if hub.prefs.skipped_update.as_deref()
        == Some(crate::model::updates::identity(release).as_str())
    {
        return None;
    }
    Some(if hub.releases.prepared().is_some() {
        "Restart to update"
    } else if hub.releases.running("hub", &release.version).is_some() {
        "Downloading update"
    } else {
        "Update available"
    })
}

pub(super) fn sidebar(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let agents: usize = hub
        .activity
        .iter()
        .map(|a| a.participants.iter().filter(|x| x.kind == "agent").count())
        .sum();
    let nav = |view: View, glyph, badge: Option<(usize, bool)>| {
        nav_item(
            l,
            &format!("nav-{view:?}").to_lowercase(),
            glyph,
            view.title(),
            hub.view == view,
            badge,
        )
        .on_press(move |s: &mut HubApp| s.intent(Intent::Navigate(view)))
    };
    let mut side = col()
        .key("sidebar")
        .w(224.0)
        .h_full()
        .pad(10.0)
        .gap(2.0)
        .fill(c.chrome.0)
        .focus_group(stillkit::el::Orientation::Vertical)
        .child(nav(
            View::Projects,
            icons::LAYOUT_GRID,
            Some((hub.projects.len(), false)),
        ))
        .child(nav(
            View::Installs,
            icons::HARD_DRIVE,
            Some((hub.engines.len(), false)),
        ))
        .child(nav(
            View::Agents,
            icons::BOT,
            (agents > 0).then_some((agents, true)),
        ))
        .child(nav(View::Learn, icons::GRADUATION_CAP, None))
        .child(el().grow(1.0));
    // Onboarding only while no agent is around and no client has Ara yet.
    if hub.view != View::Agents
        && agents == 0
        && !super::agents::connected(s)
        && update_label(s).is_none()
    {
        side = side.child(agent_card(s));
    }
    side.child(nav(View::Settings, icons::SETTINGS, None))
        .child(el().h(6.0))
        .child(divider(l))
        .child(status_row(s))
}

/// The one sidebar card: onboarding for agents while none is around.
fn agent_card(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    card(l)
        .pad(12.0)
        .gap(8.0)
        .child(
            row()
                .gap(8.0)
                .align(Align::Center)
                .child(icon(icons::PLUG).size(14.0).color(c.info_foreground.0))
                .child(
                    text("Connect an AI agent")
                        .text_size(12.0)
                        .weight(600)
                        .color(c.foreground.0),
                ),
        )
        .child(
            text("Install Ara's MCP server once. Agents then see every project you register here.")
                .text_size(11.5)
                .wrap(4)
                .color(c.muted_foreground.0),
        )
        .child(
            button(l, Tone::Secondary, Size::Sm, None, "Set up an agent")
                .key("side-agents")
                .on_press(|s: &mut HubApp| s.intent(Intent::Navigate(View::Agents))),
        )
        .child(el().h(4.0))
}

fn status_row(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let (status, label) = match &s.hub.link {
        Link::Online => (Status::Success, "Service online"),
        Link::Connecting => (Status::Warning, "Connecting…"),
        Link::Offline { .. } => (Status::Danger, "Service offline"),
    };
    row()
        .h(32.0)
        .pad_x(10.0)
        .gap(8.0)
        .align(Align::Center)
        .child(dot(l, status))
        .child(
            text(label)
                .text_size(11.5)
                .color(c.muted_foreground.0)
                .grow(1.0),
        )
        .child(
            text(ara_hub_service::ALPHA_VERSION)
                .text_size(11.0)
                .family(mono())
                .color(c.muted_foreground.0.with_alpha(0.7)),
        )
}

/// The current page in a scroll view: its header, the page's banners, then its body.
pub(super) fn page(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let hub = &s.hub;
    let header = match hub.view {
        View::Create => create::header(s),
        _ => page_header(l, hub.view.title(), Some(&subtitle(s)), header_actions(s)),
    };
    let body = match hub.view {
        View::Projects => projects::body(s),
        View::Create => create::body(s),
        View::Installs => installs::body(s),
        View::Agents => agents::body(s),
        View::Learn => learn::body(s),
        View::Settings => settings::body(s),
    };
    let mut content = col()
        .w_full()
        .max_w(1280.0)
        .pad_x(32.0)
        .pad_y(28.0)
        .gap(20.0)
        .child(header);
    if let Some(b) = offline(s) {
        content = content.child(b);
    }
    if hub.view == View::Projects
        && let Some(t) = projects::toolbar(s)
    {
        content = content.child(t);
    }
    col()
        .key(format!("page-{:?}", hub.view))
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .scroll(Axis::Vertical)
        .align(Align::Center)
        .transition(stillkit::el::Transition::slide(0.0, 6.0))
        .child(content.child(body))
}

fn header_actions(s: &HubApp) -> Vec<El<HubApp>> {
    let l = &s.look;
    match s.hub.view {
        View::Projects => vec![
            button(
                l,
                Tone::Secondary,
                Size::Md,
                Some(icons::FOLDER_OPEN),
                "Add existing",
            )
            .key("add-existing")
            .tooltip("Register a project folder (⌘O)")
            .on_press(|s: &mut HubApp| s.intent(Intent::AddExisting)),
            button(l, Tone::Primary, Size::Md, Some(icons::PLUS), "New project")
                .key("new-project")
                .tooltip("New project (⌘N)")
                .on_press(|s: &mut HubApp| s.intent(Intent::Navigate(View::Create))),
        ],
        View::Installs => vec![
            button(
                l,
                Tone::Secondary,
                Size::Md,
                Some(icons::PLUS),
                "Add engine build",
            )
            .key("add-engine")
            .on_press(|s: &mut HubApp| s.intent(Intent::AddEngine)),
        ],
        _ => Vec::new(),
    }
}

fn subtitle(s: &HubApp) -> String {
    let hub = &s.hub;
    match hub.view {
        View::Projects => {
            let open = hub
                .projects
                .iter()
                .filter(|p| hub.state(&p.id) != State::Closed)
                .count();
            let count = hub.projects.len();
            if count == 0 {
                return "Create your first project, or add one you already have.".into();
            }
            let noun = if count == 1 { "project" } else { "projects" };
            if open > 0 {
                format!("{count} {noun} · {open} open")
            } else {
                format!("{count} {noun}")
            }
        }
        View::Create => "Start from a template or one of the real-art samples.".into(),
        View::Installs => {
            "Engine builds install side by side. Each project pins one version.".into()
        }
        View::Agents => "Connect agents once; they work in any project registered here.".into(),
        View::Learn => "Samples to explore, Ara's docs and the command line.".into(),
        View::Settings => "Appearance, Hub updates and where releases come from.".into(),
    }
}

/// The service-offline banner, until dismissed for this incident.
fn offline(s: &HubApp) -> Option<El<HubApp>> {
    let l = &s.look;
    let Link::Offline { message, log } = &s.hub.link else {
        return None;
    };
    if !s.hub.banner_visible("offline") {
        return None;
    }
    let mut actions = Vec::new();
    if log.is_file() {
        let log = log.clone();
        actions.push(
            button(
                l,
                Tone::Ghost,
                Size::Sm,
                Some(icons::FILE_TEXT),
                "Open hub-start.log",
            )
            .key("open-start-log")
            .on_press(move |s: &mut HubApp| s.intent(Intent::OpenPath(log.clone()))),
        );
    }
    actions.push(dismiss(l, "dismiss-offline", "offline"));
    Some(banner(
        l,
        Status::Danger,
        icons::CIRCLE_ALERT,
        &format!("Can't reach the Ara Hub service: {message}. Retrying automatically."),
        actions,
    ))
}

/// A dismiss button for an inline banner.
pub(super) fn dismiss(l: &ara_kit::look::Look, key: &str, banner_key: &str) -> El<HubApp> {
    let banner_key = banner_key.to_owned();
    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Dismiss")
        .key(key)
        .on_press(move |s: &mut HubApp| s.intent(Intent::DismissBanner(banner_key.clone())))
}

/// Informational toasts at the bottom right (errors open dialogs instead).
pub(super) fn toasts(s: &HubApp, root: El<HubApp>) -> El<HubApp> {
    let l = &s.look;
    let notices: Vec<_> = s.hub.notices.iter().filter(|n| !n.is_error()).collect();
    if notices.is_empty() {
        return root;
    }
    let stack = col()
        .gap(8.0)
        .pad(16.0)
        .align(Align::End)
        .children(notices.into_iter().map(|notice| {
            let id = notice.id;
            let mut actions = col().gap(6.0);
            let mut fix_index = 0;
            for diagnostic in &notice.diagnostics {
                for fix in &diagnostic.fixes {
                    let index = fix_index;
                    fix_index += 1;
                    if let FixAction::Call { method, .. } = &fix.action
                        && method.starts_with("hub.")
                    {
                        actions = actions.child(
                            button(l, Tone::Secondary, Size::Sm, Some(icons::CHECK), &fix.title)
                                .key(format!("fix-{id}-{index}"))
                                .on_press(move |s: &mut HubApp| {
                                    s.intent(Intent::Fix {
                                        notice: id,
                                        fix: index,
                                    })
                                }),
                        );
                    }
                }
            }
            let action = if notice.undo.is_some() {
                Some(
                    button(l, Tone::Ghost, Size::Sm, None, "Undo")
                        .key(format!("undo-{id}"))
                        .color(l.c.info_foreground.0)
                        .on_press(move |s: &mut HubApp| {
                            s.intent(Intent::Fix {
                                notice: id,
                                fix: usize::MAX,
                            })
                        }),
                )
            } else {
                notice.log.clone().map(|log| {
                    button(l, Tone::Ghost, Size::Sm, None, "Open log")
                        .key(format!("log-{id}"))
                        .on_press(move |s: &mut HubApp| s.intent(Intent::OpenPath(log.clone())))
                })
            };
            let message = match &notice.detail {
                Some(d) => format!("{} · {d}", notice.title),
                None => notice.title.clone(),
            };
            col()
                .gap(6.0)
                .child(toast(
                    l,
                    &format!("notice-{id}"),
                    Status::Success,
                    &message,
                    action,
                    move |s: &mut HubApp| s.intent(Intent::Dismiss(id)),
                ))
                .child(actions)
        }));
    root.overlay(toast_stack(stack))
}

/// A command or config in a code well with a copy button (it shows a check once copied).
pub(super) fn code(s: &HubApp, key: &str, content: &str) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let lines = col()
        .grow(1.0)
        .gap(2.0)
        .children(content.lines().map(|line| {
            text(line.to_owned())
                .text_size(11.5)
                .family(mono())
                .color(c.code_foreground.0)
        }));
    row()
        .w_full()
        .pad(10.0)
        .gap(8.0)
        .align(if content.lines().count() > 1 {
            Align::Start
        } else {
            Align::Center
        })
        .radius(l.radius.sm)
        .fill(c.code_background.0)
        .border(1.0, c.border.0)
        .child(lines)
        .child(copy(s, key, content))
}

/// A copy button: copies `content`, then shows a check while it is what was copied last.
pub(super) fn copy(s: &HubApp, key: &str, content: &str) -> El<HubApp> {
    let l = &s.look;
    let copied = s.hub.copied.as_deref() == Some(content);
    let content = content.to_owned();
    let b = icon_button(
        l,
        Tone::Ghost,
        Size::Sm,
        if copied { icons::CHECK } else { icons::COPY },
        if copied { "Copied" } else { "Copy" },
    )
    .key(key)
    .on_press(move |s: &mut HubApp| s.intent(Intent::Copy(content.clone())));
    if copied {
        b.color(l.c.success_foreground.0)
    } else {
        b
    }
}

/// Justified label/value rows (install summaries).
pub(super) fn facts(l: &ara_kit::look::Look, rows: &[(&str, String)]) -> El<HubApp> {
    let c = &l.c;
    card(l)
        .pad(12.0)
        .gap(6.0)
        .fill(c.muted.0)
        .children(rows.iter().map(|(k, v)| {
            row()
                .justify(Justify::Between)
                .child(text(*k).text_size(12.0).color(c.muted_foreground.0))
                .child(
                    text(v.clone())
                        .text_size(12.0)
                        .weight(560)
                        .color(c.foreground.0),
                )
        }))
}
