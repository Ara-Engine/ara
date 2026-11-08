//! Projects: a toolbar (search, sort, layout), then cover-art cards or a dense list. Each
//! project has one primary action; the rest live in its menu (the ⋯ button, a right click,
//! or the Menu key). Double-click or Enter opens; F2 renames in place.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{
    Align, El, Handled, KeyCode, PointerPhase, col, el, grid, icon, image, menu, row, text,
    text_input,
};
use stillkit::{Role, TextRole};

use super::images::Pic;
use super::{HubApp, create};
use crate::model::{Hub, Intent, Layout, Project, Sort, State, text as fmt};

/// Sidebar, page padding: what the grid can't use of the window's width.
const CHROME: f32 = 224.0 + 64.0;
const CARD_MIN: f32 = 248.0;
const GAP: f32 = 16.0;

/// The search, sort and layout row (once there is anything to search).
pub(super) fn toolbar(s: &HubApp) -> Option<El<HubApp>> {
    let l = &s.look;
    let hub = &s.hub;
    if hub.projects.is_empty() {
        return None;
    }
    let sort = match hub.prefs.sort {
        Sort::Recent => 0,
        Sort::Name => 1,
    };
    let layout = match hub.prefs.layout {
        Layout::Grid => 0,
        Layout::List => 1,
    };
    Some(
        row()
            .gap(10.0)
            .align(Align::Center)
            .child(
                search(
                    l,
                    "search",
                    "Search projects",
                    &hub.search,
                    Some("/"),
                    |f| f.on_input(|s: &mut HubApp, t| s.intent(Intent::Search(t.into()))),
                    |s: &mut HubApp| s.intent(Intent::Search(String::new())),
                )
                .w(300.0),
            )
            .child(el().grow(1.0))
            .child(text("Sort").text_size(12.0).color(l.c.muted_foreground.0))
            .child(segmented(
                l,
                &[
                    ("sort-recent", None, "Last opened"),
                    ("sort-name", None, "Name"),
                ],
                sort,
                |i| {
                    move |s: &mut HubApp| {
                        s.intent(Intent::Sort(if i == 0 { Sort::Recent } else { Sort::Name }))
                    }
                },
            ))
            .child(segmented(
                l,
                &[
                    ("layout-cards", Some(icons::LAYOUT_GRID), "Cards"),
                    ("layout-list", Some(icons::LIST), "List"),
                ],
                layout,
                |i| {
                    move |s: &mut HubApp| {
                        s.intent(Intent::Layout(if i == 0 {
                            Layout::Grid
                        } else {
                            Layout::List
                        }))
                    }
                },
            )),
    )
}

pub(super) fn body(s: &HubApp) -> El<HubApp> {
    let hub = &s.hub;
    if hub.projects.is_empty() {
        return if hub.ready() {
            first_run(s)
        } else {
            loading(s)
        };
    }
    let visible = hub.visible();
    if visible.is_empty() {
        return empty(
            &s.look,
            icons::SEARCH,
            &format!("No projects match “{}”", hub.search.trim()),
            "Search looks at project names and folders.",
            None,
        );
    }
    match hub.prefs.layout {
        Layout::Grid => cards(s, &visible),
        Layout::List => list(s, &visible),
    }
}

fn loading(s: &HubApp) -> El<HubApp> {
    row()
        .pad(40.0)
        .gap(10.0)
        .align(Align::Center)
        .child(spinner(&s.look, 16.0))
        .child(
            text("Loading projects…")
                .text_size(12.0)
                .color(s.look.c.muted_foreground.0),
        )
}

/// Columns of cards that fit the window.
pub(super) fn columns(s: &HubApp, min: f32) -> u16 {
    let width = (s.width.min(1280.0 + CHROME) - CHROME).max(min);
    (((width + GAP) / (min + GAP)).floor() as u16).clamp(1, 6)
}

/// First run: one welcoming line, then the templates as the hero.
fn first_run(s: &HubApp) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let hub = &s.hub;
    let welcome = card(l)
        .pad(20.0)
        .child(
            row().gap(14.0).align(Align::Center)
                .child(el().size(40.0).radius(10.0).fill(c.primary.0.with_alpha(0.14)).align(Align::Center).justify(stillkit::el::Justify::Center).child(icon(icons::FOLDER_PLUS).size(20.0).color(c.info_foreground.0)))
                .child(col().gap(4.0).grow(1.0)
                    .child(text("No projects yet").font(TextRole::Headline).color(c.foreground.0))
                    .child(text("Start from a template below, add a project folder you already have, or drop a folder onto this window.").text_size(12.0).wrap(2).color(c.muted_foreground.0))),
        );
    let mut out = col().gap(20.0).child(welcome);
    if !hub.templates.is_empty() {
        out = out
            .child(section(
                l,
                "Start from a template",
                Some("Outpost and Showcase are real-art samples."),
                Vec::new(),
            ))
            .child(
                grid(columns(s, 230.0))
                    .gap(GAP)
                    .children(hub.templates.iter().map(|t| create::tile(s, t, false))),
            );
    }
    out
}

/// The state chip on a project (in-flight work wins).
fn state_chip(s: &HubApp, project: &Project) -> Option<El<HubApp>> {
    let l = &s.look;
    let hub = &s.hub;
    if let Some(busy) = hub.busy.get(&project.id) {
        return Some(
            row()
                .gap(6.0)
                .align(Align::Center)
                .h(22.0)
                .pad_x(8.0)
                .radius(11.0)
                .fill(stillkit::Rgba::new(0.0, 0.0, 0.0, 0.62))
                .child(spinner(l, 11.0))
                .child(
                    text(busy.label())
                        .text_size(11.0)
                        .weight(560)
                        .color(stillkit::Rgba::new(1.0, 1.0, 1.0, 0.95)),
                ),
        );
    }
    match hub.state(&project.id) {
        State::Editor => Some(photo_chip(l, Status::Success, "Open in editor")),
        State::Headless => Some(photo_chip(l, Status::Accent, "Running headless")),
        State::Closed => None,
    }
}

fn installed(hub: &Hub, project: &Project) -> bool {
    hub.engines
        .iter()
        .any(|e| e.version == project.engine_version)
}

/// The engine pin chip: neutral, or a warning when that build isn't installed.
fn engine_chip(s: &HubApp, project: &Project) -> El<HubApp> {
    let l = &s.look;
    if installed(&s.hub, project) || s.hub.engines.is_empty() {
        chip(l, Status::Neutral, &project.engine_version, false)
    } else {
        chip(
            l,
            Status::Warning,
            &format!("{} missing", project.engine_version),
            true,
        )
    }
}

/// The one primary action for a project's state.
fn primary_action(s: &HubApp, project: &Project, size: Size) -> El<HubApp> {
    let l = &s.look;
    let id = project.id.clone();
    let busy = s.hub.busy.contains_key(&id);
    let (tone, glyph, label, intent) = match s.hub.state(&id) {
        State::Closed if !installed(&s.hub, project) && !s.hub.engines.is_empty() => (
            Tone::Primary,
            icons::DOWNLOAD,
            format!("Install {} and open", project.engine_version),
            Intent::InstallAndOpen {
                project: id.clone(),
                headless: false,
            },
        ),
        State::Closed => (
            Tone::Primary,
            icons::PLAY,
            "Open".into(),
            Intent::Open(id.clone()),
        ),
        State::Headless => (
            Tone::Primary,
            icons::MONITOR,
            "Open editor".into(),
            Intent::Open(id.clone()),
        ),
        State::Editor => (
            Tone::Secondary,
            icons::SQUARE,
            "Close".into(),
            Intent::Close(id.clone()),
        ),
    };
    enabled(
        button(l, tone, size, Some(glyph), &label)
            .key(format!("open-{id}"))
            .on_press(move |s: &mut HubApp| s.intent(intent.clone())),
        !busy,
    )
}

/// Opens a project (its primary action when it isn't open).
fn open(s: &mut HubApp, id: &str) {
    let Some(project) = s.hub.project(id).cloned() else {
        return;
    };
    if s.hub.busy.contains_key(id) || s.hub.state(id) == State::Editor {
        return;
    }
    let intent = if !installed(&s.hub, &project) && !s.hub.engines.is_empty() {
        Intent::InstallAndOpen {
            project: project.id,
            headless: false,
        }
    } else {
        Intent::Open(project.id)
    };
    s.intent(intent);
}

/// A project's name, or the inline rename field while renaming it.
fn name(s: &HubApp, project: &Project, role: TextRole) -> El<HubApp> {
    let l = &s.look;
    match &s.hub.rename {
        Some((id, draft)) if *id == project.id => row()
            .h(26.0)
            .pad_x(6.0)
            .radius(l.radius.xs)
            .fill(l.c.muted.0)
            .border(1.0, l.c.ring.0)
            .align(Align::Center)
            .grow(1.0)
            .child(
                text_input("rename-field")
                    .grow(1.0)
                    .value(draft.as_str())
                    .select_all_on_focus()
                    .font(role)
                    .color(l.c.foreground.0)
                    .on_input(|s: &mut HubApp, t| s.intent(Intent::RenameDraft(t.into())))
                    .on_submit(|s: &mut HubApp, _| s.intent(Intent::RenameCommit))
                    .on_key(|s: &mut HubApp, key, _| {
                        if key == KeyCode::ESCAPE {
                            s.intent(Intent::RenameCancel);
                            Handled::Yes
                        } else {
                            Handled::No
                        }
                    }),
            ),
        _ => text(project.name.clone())
            .font(role)
            .color(l.c.foreground.0)
            .grow(1.0),
    }
}

/// The ⋯ button that opens a project's menu.
fn more(s: &HubApp, project: &Project) -> El<HubApp> {
    let id = project.id.clone();
    icon_button(
        &s.look,
        Tone::Ghost,
        Size::Sm,
        icons::ELLIPSIS,
        "More actions",
    )
    .key(format!("more-{}", project.id))
    .on_press(move |s: &mut HubApp| s.intent(Intent::Menu(Some(id.clone()))))
}

/// The project's menu, anchored under its ⋯ button while it is the open one.
fn with_menu(s: &HubApp, project: &Project, el: El<HubApp>) -> El<HubApp> {
    if s.hub.menu.as_deref() != Some(project.id.as_str()) {
        return el;
    }
    el.overlay(
        menu(project_menu(s, project))
            .anchor(format!("more-{}", project.id))
            .align(stillkit::el::Align::End)
            .on_dismiss(|s: &mut HubApp| s.intent(Intent::Menu(None))),
    )
}

fn project_menu(s: &HubApp, project: &Project) -> El<HubApp> {
    let l = &s.look;
    let hub = &s.hub;
    let id = project.id.clone();
    let state = hub.state(&id);
    let busy = hub.busy.contains_key(&id);
    let missing = !installed(hub, project) && !hub.engines.is_empty();
    let item = |key: &str,
                glyph,
                label: &str,
                shortcut: Option<&str>,
                danger: bool,
                ok: bool,
                intent: Intent| {
        let key = format!("menu-{key}");
        enabled(
            menu_item(l, &key, Some(glyph), label, shortcut, danger).on_press(
                move |s: &mut HubApp| {
                    s.intent(Intent::Menu(None));
                    s.intent(intent.clone());
                },
            ),
            ok && !busy,
        )
    };
    let open = if missing {
        Intent::InstallAndOpen {
            project: id.clone(),
            headless: false,
        }
    } else {
        Intent::Open(id.clone())
    };
    let headless = if missing {
        Intent::InstallAndOpen {
            project: id.clone(),
            headless: true,
        }
    } else {
        Intent::OpenHeadless(id.clone())
    };
    let rename_id = id.clone();
    menu_panel(
        l,
        vec![
            menu_heading(l, &project.name),
            item(
                "open",
                icons::PLAY,
                if missing {
                    "Install and open in editor"
                } else {
                    "Open in editor"
                },
                Some("↩"),
                false,
                state != State::Editor,
                open,
            ),
            item(
                "headless",
                icons::SERVER,
                if missing {
                    "Install and open headless"
                } else {
                    "Open headless"
                },
                None,
                false,
                state == State::Closed,
                headless,
            ),
            item(
                "close",
                icons::SQUARE,
                "Close",
                None,
                false,
                state != State::Closed,
                Intent::Close(id.clone()),
            ),
            menu_separator(l),
            menu_item(
                l,
                "menu-rename",
                Some(icons::PENCIL),
                "Rename",
                Some("F2"),
                false,
            )
            .on_press_with(move |s: &mut HubApp, tx| {
                s.intent(Intent::Menu(None));
                s.intent(Intent::Rename(rename_id.clone()));
                tx.focus("rename-field");
            }),
            item(
                "reveal",
                icons::FOLDER_OPEN,
                if cfg!(target_os = "macos") {
                    "Show in Finder"
                } else {
                    "Show in Explorer"
                },
                None,
                false,
                true,
                Intent::Reveal(id.clone()),
            ),
            item(
                "copy-path",
                icons::COPY,
                "Copy path",
                None,
                false,
                true,
                Intent::Copy(fmt::path(&project.path)),
            ),
            menu_separator(l),
            item(
                "remove",
                icons::TRASH,
                "Remove from list",
                None,
                true,
                true,
                Intent::Remove(id.clone()),
            ),
        ],
    )
}

/// What every project element does on its own: double-click opens, F2 renames, Menu and
/// right click open its menu.
fn interactive(el: El<HubApp>, project: &Project) -> El<HubApp> {
    let (a, b, c) = (project.id.clone(), project.id.clone(), project.id.clone());
    el.focusable()
        .role(Role::ListItem)
        .label(project.name.clone())
        .on_pointer(move |s: &mut HubApp, e, _| {
            if e.phase == PointerPhase::Down && e.clicks == 2 && e.button == stillkit::Button::Left
            {
                open(s, &a);
                return true;
            }
            false
        })
        .on_context_menu(move |s: &mut HubApp, _, _| s.intent(Intent::Menu(Some(b.clone()))))
        .on_key(move |s: &mut HubApp, key, tx| match key.0 {
            0x0D => {
                open(s, &c);
                Handled::Yes
            }
            0x71 => {
                s.intent(Intent::Rename(c.clone()));
                tx.focus("rename-field");
                Handled::Yes
            }
            0x2E | 0x08 if tx.modifiers().ctrl || key.0 == 0x2E => {
                s.intent(Intent::Remove(c.clone()));
                Handled::Yes
            }
            _ => Handled::No,
        })
}

/// The project's picture: its thumbnail, else its generated cover with its initials.
fn cover(s: &HubApp, project: &Project, radius: f32) -> El<HubApp> {
    let l = &s.look;
    let file = project
        .thumbnail
        .clone()
        .filter(|p| !s.images.failed(&Pic::File(p.clone())));
    let pic = file
        .map(Pic::File)
        .unwrap_or(Pic::Cover(ara_kit::art::seed(&project.id), l.dark));
    let generated = matches!(pic, Pic::Cover(..));
    let mut frame = el().w_full().radius(radius).fill(l.c.muted.0);
    if let Some(img) = s.images.get(&pic) {
        frame = frame.child(image(img).w_full().h_full().radius(radius));
    }
    if generated {
        frame = frame.child(
            el().absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .align(Align::Center)
                .justify(stillkit::el::Justify::Center)
                .child(
                    text(initials(&project.name))
                        .text_size(40.0)
                        .weight(700)
                        .color(stillkit::Rgba::new(
                            1.0,
                            1.0,
                            1.0,
                            if l.dark { 0.75 } else { 0.85 },
                        )),
                ),
        );
    }
    frame
}

fn cards(s: &HubApp, visible: &[&Project]) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let cols = columns(s, CARD_MIN);
    let card_w =
        ((s.width.min(1280.0 + CHROME) - CHROME) - GAP * f32::from(cols - 1)) / f32::from(cols);
    let thumb_h = ((card_w - 2.0) * 9.0 / 16.0).round();
    grid(cols).gap(GAP).children(visible.iter().map(|project| {
        let hot = s.hub.highlight.as_deref() == Some(project.id.as_str());
        let mut chip_row = row().absolute(10.0, 10.0).gap(6.0);
        if let Some(chip) = state_chip(s, project) {
            chip_row = chip_row.child(chip);
        }
        let picture = el()
            .w_full()
            .h(thumb_h)
            .child(cover(s, project, l.radius.base - 1.0).h_full())
            .child(chip_row);
        let body = col()
            .pad_x(14.0)
            .pad_y(12.0)
            .gap(6.0)
            .child(
                row()
                    .gap(6.0)
                    .align(Align::Center)
                    .h(24.0)
                    .child(name(s, project, TextRole::Headline))
                    .child(more(s, project)),
            )
            .child(
                text(fmt::middle(
                    &fmt::path(&project.path),
                    ((card_w - 28.0) / 6.6) as usize,
                ))
                .text_size(11.0)
                .family(mono())
                .color(c.muted_foreground.0),
            )
            .child(
                row()
                    .gap(6.0)
                    .align(Align::Center)
                    .pad_y(2.0)
                    .child(icon(icons::CLOCK).size(12.0).color(c.muted_foreground.0))
                    .child(
                        text(fmt::relative(project.last_opened, s.now_ms))
                            .text_size(11.5)
                            .color(c.muted_foreground.0),
                    )
                    .child(el().grow(1.0))
                    .child(engine_chip(s, project)),
            );
        let footer = col()
            .pad_x(14.0)
            .child(primary_action(s, project, Size::Md))
            .child(el().h(14.0));
        let mut card = hover_card(l)
            .key(project.id.clone())
            .child(picture)
            .child(body)
            .child(footer);
        if hot {
            card = card
                .border(1.0, c.ring.0)
                .shadow_level(stillkit::ShadowLevel::Raised);
        }
        with_menu(s, project, interactive(card, project))
    }))
}

fn list(s: &HubApp, visible: &[&Project]) -> El<HubApp> {
    let l = &s.look;
    let c = &l.c;
    let head = |t: &str| {
        text(t.to_owned())
            .text_size(11.0)
            .weight(600)
            .color(c.muted_foreground.0)
    };
    let header = row()
        .h(32.0)
        .pad_x(14.0)
        .gap(14.0)
        .align(Align::Center)
        .child(el().w(64.0))
        .child(head("Name").grow(1.0))
        .child(head("Status").w(140.0))
        .child(head("Last opened").w(110.0))
        .child(head("Engine").w(150.0))
        .child(el().w(150.0));
    let mut items = Vec::new();
    for (i, project) in visible.iter().enumerate() {
        let hot = s.hub.highlight.as_deref() == Some(project.id.as_str());
        let r =
            row()
                .key(project.id.clone())
                .h(56.0)
                .pad_x(14.0)
                .gap(14.0)
                .align(Align::Center)
                .fill(if hot {
                    c.primary.0.with_alpha(0.08)
                } else {
                    stillkit::Rgba::TRANSPARENT
                })
                .child(cover(s, project, l.radius.xs).w(64.0).h(36.0))
                .child(
                    col()
                        .gap(3.0)
                        .grow(1.0)
                        .min_w(0.0)
                        .child(row().child(name(s, project, TextRole::Body)))
                        .child(
                            text(fmt::path(&project.path))
                                .text_size(11.0)
                                .family(mono())
                                .color(c.muted_foreground.0),
                        ),
                )
                .child(el().w(140.0).child(
                    state_chip(s, project).unwrap_or_else(|| {
                        text("Closed").text_size(11.5).color(c.muted_foreground.0)
                    }),
                ))
                .child(
                    text(fmt::relative(project.last_opened, s.now_ms))
                        .text_size(11.5)
                        .color(c.muted_foreground.0)
                        .w(110.0),
                )
                .child(el().w(150.0).child(engine_chip(s, project)))
                .child(
                    row()
                        .w(150.0)
                        .gap(4.0)
                        .justify(stillkit::el::Justify::End)
                        .child(primary_action(s, project, Size::Sm))
                        .child(more(s, project)),
                );
        if i > 0 {
            items.push(divider(l));
        }
        items.push(with_menu(s, project, interactive(r, project)));
    }
    card(l).child(header).child(divider(l)).children(items)
}
