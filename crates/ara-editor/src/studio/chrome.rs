//! The studio window's frame: the title bar (menus from the command registry, Play
//! controls, the scene path, the ⌘K launcher), the dock in between, the status bar, and the
//! overlays they open (menus, the command palette).
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{
    Align, Axis, El, Handled, Justify, KeyCode, col, dialog, el, icon, menu, row, text, text_input,
};
use stillkit::{Role, ShadowLevel};

use super::{Studio, commands, dock};
use crate::shell::commands::Target;
use crate::shell::commands::menus::{Item, MENUS};

/// The whole window.
pub fn view(s: &Studio) -> El<Studio> {
    let root = col()
        .key("studio")
        .w_full()
        .h_full()
        .fill(super::style::tones(&s.look).gap)
        .on_key(keys)
        .child(title_bar(s))
        .child(dock::view(s).grow(1.0).min_h(0.0))
        .child(status_bar(s))
        .children(drag_chip(s))
        .overlay(context_menu(s))
        .overlay(super::create::overlay(s))
        .overlay(super::preferences::overlay(s));
    palette(s, root)
}

/// What an asset drag carries, beside the pointer.
fn drag_chip(s: &Studio) -> Option<El<Studio>> {
    let (_, name, (x, y)) = s.local.asset_drag.as_ref()?;
    let l = &s.look;
    let c = &l.c;
    Some(
        row()
            .absolute(x + 14.0, y + 10.0)
            .h(26.0)
            .pad_x(10.0)
            .gap(6.0)
            .align(Align::Center)
            .radius(l.radius.sm)
            .fill(c.popover.0)
            .shadow_level(stillkit::ShadowLevel::Menu)
            .child(icon(icons::BOX).size(13.0).color(c.primary.0))
            .child(text(name.clone()).text_size(12.0).color(c.foreground.0)),
    )
}

/// The open context menu at the pointer: the registry's layout for what was clicked, its
/// commands run on that target.
fn context_menu(s: &Studio) -> Option<stillkit::el::Overlay<Studio>> {
    let (kind, target, (x, y)) = s.local.context.clone()?;
    let items = crate::shell::commands::menus::context(kind);
    let panel = menu_panel(&s.look, entries(s, items, true, &target))
        .max_h(560.0)
        .scroll(Axis::Vertical);
    Some(
        menu(panel)
            .at(x, y)
            .on_dismiss(|s: &mut Studio| s.local.context = None),
    )
}

/// Window-wide shortcuts: the registry's chords for the commands the studio runs. Keys a
/// focused field takes (typing, its own Enter) never reach here.
fn keys(s: &mut Studio, key: KeyCode, tx: &mut stillkit::el::Tx) -> Handled {
    match commands::for_key(key.0, tx.modifiers()) {
        Some(id) => {
            commands::run(s, id, &Target::None);
            if id == "edit.palette" {
                tx.focus("palette/query");
            }
            Handled::Yes
        }
        None => Handled::No,
    }
}

fn title_bar(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let mut lead = vec![
        el().size(20.0)
            .radius(6.0)
            .fill(c.primary.0)
            .align(Align::Center)
            .justify(Justify::Center)
            .child(
                text("A")
                    .text_size(12.0)
                    .weight(800)
                    .color(c.primary_foreground.0),
            ),
    ];
    lead.extend(MENUS.iter().map(|m| {
        if m.title == "Create" {
            create_button(s)
        } else {
            menu_button(s, m.title, m.items)
        }
    }));
    let play = play_controls(s);
    let workspace = s.editor.host.store().workspace();
    let scene = workspace
        .path_of(&s.editor.state.doc)
        .map(|path| workspace.reference_path(path))
        .unwrap_or("Scene")
        .rsplit('/')
        .next()
        .unwrap_or("Scene")
        .trim_end_matches(".scene")
        .to_owned();
    let project = s
        .editor
        .host
        .root()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hover = super::style::tones(l).hover;
    let launcher = row()
        .key("launcher")
        .w(220.0)
        .h(26.0)
        .pad_x(8.0)
        .gap(8.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(super::style::tones(l).well)
        .color(c.muted_foreground.0)
        .cascade_ink()
        .label("Search or run a command")
        .hovered(move |e| e.fill(hover))
        .on_press_with(|s: &mut Studio, tx| {
            s.local.palette = Some((String::new(), 0));
            tx.focus("palette/query");
        })
        .child(icon(icons::SEARCH).size(13.0))
        .child(text("Search or run…").text_size(12.0).grow(1.0))
        .child(kbd(
            l,
            if cfg!(target_os = "macos") {
                "⌘K"
            } else {
                "Ctrl K"
            },
        ));
    let crumb = row()
        .gap(6.0)
        .align(Align::Center)
        .child(
            text(project)
                .text_size(12.0)
                .weight(560)
                .color(c.foreground.0),
        )
        .child(
            icon(icons::CHEVRON_RIGHT)
                .size(12.0)
                .color(c.muted_foreground.0),
        )
        .child(text(scene).text_size(12.0).color(c.muted_foreground.0));
    let theme = icon_button(
        l,
        Tone::Ghost,
        Size::Sm,
        if s.editor.state.light {
            icons::MOON
        } else {
            icons::SUN
        },
        "Toggle theme",
    )
    .key("theme")
    .on_press(|s: &mut Studio| {
        let id = if s.editor.state.light {
            "view.theme.dark"
        } else {
            "view.theme.light"
        };
        commands::run(s, id, &Target::None);
    });
    ara_kit::ui::title_bar(
        l,
        s.frame.inset,
        lead,
        vec![play],
        vec![crumb, launcher, theme],
        s.frame.controls,
    )
}

/// The title bar's Create: opens the Create browser under it.
fn create_button(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let open = s
        .local
        .create
        .as_ref()
        .is_some_and(|b| b.anchor == super::create::Anchor::Key("menu/Create"));
    let mut b = button(l, Tone::Ghost, Size::Sm, None, "Create")
        .key("menu/Create")
        .expanded(open)
        .on_press(|s: &mut Studio| {
            super::create::toggle(s, super::create::Anchor::Key("menu/Create"), None)
        });
    if open {
        b = b.fill(l.c.raised.0).color(l.c.foreground.0);
    }
    b
}

/// A title-bar menu: its name, and its registry items while open.
fn menu_button(s: &Studio, title: &'static str, items: &'static [Item]) -> El<Studio> {
    let l = &s.look;
    let open = s.local.menu == Some(title);
    let mut b = button(l, Tone::Ghost, Size::Sm, None, title)
        .key(format!("menu/{title}"))
        .expanded(open)
        .on_press(move |s: &mut Studio| {
            s.local.menu = if s.local.menu == Some(title) {
                None
            } else {
                Some(title)
            }
        });
    if open {
        b = b.fill(l.c.raised.0).color(l.c.foreground.0);
        let panel = menu_panel(l, entries(s, items, false, &Target::None))
            .max_h(560.0)
            .scroll(Axis::Vertical);
        b = b.overlay(menu(panel).on_dismiss(|s: &mut Studio| s.local.menu = None));
    }
    b
}

/// One line of a flattened menu.
#[derive(Clone, Copy)]
enum Line {
    Separator,
    Heading(&'static str),
    Command(&'static str),
}

/// Flattens registry items: a submenu becomes a separator and a heading over its entries.
/// `hide` drops commands the studio doesn't run yet (context menus stay short).
fn lines(items: &'static [Item], depth: usize, hide: bool, out: &mut Vec<Line>) {
    for item in items {
        match item {
            Item::Separator => out.push(Line::Separator),
            Item::Command(id) if !(hide && commands::missing(id).is_some()) => {
                out.push(Line::Command(id))
            }
            Item::Sub(title, sub) if depth < 2 => {
                out.push(Line::Separator);
                out.push(Line::Heading(title));
                lines(sub, depth + 1, hide, out);
            }
            Item::Command(_) | Item::Sub(..) | Item::Dynamic(_) => {}
        }
    }
}

/// Menu entries for registry items, with no empty headings and no leading, trailing or
/// doubled separators; commands act on `target`.
fn entries(s: &Studio, items: &'static [Item], hide: bool, target: &Target) -> Vec<El<Studio>> {
    let l = &s.look;
    let mut all = Vec::new();
    lines(items, 0, hide, &mut all);
    let mut clean: Vec<Line> = Vec::new();
    for (i, line) in all.iter().enumerate() {
        match line {
            Line::Heading(_) if !matches!(all.get(i + 1), Some(Line::Command(_))) => {}
            Line::Separator if !matches!(clean.last(), Some(Line::Command(_))) => {}
            _ => clean.push(*line),
        }
    }
    while matches!(clean.last(), Some(Line::Separator | Line::Heading(_))) {
        clean.pop();
    }
    clean
        .into_iter()
        .filter_map(|line| match line {
            Line::Separator => Some(menu_separator(l)),
            Line::Heading(title) => Some(menu_heading(l, title)),
            Line::Command(id) => command_item(s, id, target),
        })
        .collect()
}

/// One registry command as a menu item: its icon, title and shortcut; unavailable with
/// its reason when the studio doesn't run it yet.
fn command_item(s: &Studio, id: &'static str, target: &Target) -> Option<El<Studio>> {
    let target = target.clone();
    let l = &s.look;
    let spec = commands::spec(id)?;
    let glyph = icons::named(spec.icon);
    let keys = commands::shortcut(spec.keys);
    let item = menu_item(
        l,
        &format!("cmd/{id}"),
        glyph,
        spec.title,
        keys.as_deref(),
        id == "edit.delete",
    )
    .on_press(move |s: &mut Studio| {
        s.local.menu = None;
        s.local.context = None;
        commands::run(s, id, &target);
    });
    Some(match commands::missing(id) {
        Some(reason) => enabled(item.description(reason), false),
        None => item.description(spec.summary),
    })
}

/// Play, Pause/Resume, Step and Stop, centred in the title bar.
fn play_controls(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let playing = s.editor.state.playing;
    let run = |id: &'static str| move |s: &mut Studio| commands::run(s, id, &Target::None);
    let main = if playing == Some(true) {
        icon_button(l, Tone::Ghost, Size::Sm, icons::PAUSE, "Pause (F6)")
            .key("play/pause")
            .on_press(run("play.pause"))
    } else {
        icon_button(
            l,
            Tone::Ghost,
            Size::Sm,
            icons::PLAY,
            if playing == Some(false) {
                "Resume (F5)"
            } else {
                "Play (F5)"
            },
        )
        .key("play/start")
        .color(c.success_foreground.0)
        .on_press(run("play.start"))
    };
    // While Play runs the group takes the state's tint (green, amber paused), no outline.
    let fill = match playing {
        Some(true) => c.success.0.with_alpha(0.22),
        Some(false) => c.warning.0.with_alpha(0.22),
        None => super::style::tones(l).well,
    };
    let mut group = row()
        .h(26.0)
        .pad_x(2.0)
        .gap(1.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(fill)
        .child(main)
        .child(enabled(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::STEP_FORWARD,
                "Step one tick (F7)",
            )
            .key("play/step")
            .on_press(run("play.step")),
            playing.is_some(),
        ))
        .child(enabled(
            icon_button(l, Tone::Ghost, Size::Sm, icons::SQUARE, "Stop (Shift+F5)")
                .key("play/stop")
                .on_press(run("play.stop")),
            playing.is_some(),
        ));
    if playing.is_some() {
        group = group.child(chip(
            l,
            Status::Success,
            if playing == Some(true) {
                "Playing"
            } else {
                "Paused"
            },
            true,
        ));
    }
    group
}

fn status_bar(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let count = s.resolved.len();
    let selected = s.editor.state.selection.len();
    let entities = match selected {
        0 => format!("{count} entities"),
        n => format!("{count} entities · {n} selected"),
    };
    let mut right = row().gap(14.0).align(Align::Center);
    // Play is never only a colour (I40): the chip names the state.
    if let Some(playing) = s.editor.state.playing {
        let (status, label) = if playing {
            (Status::Success, "Playing")
        } else {
            (Status::Warning, "Paused")
        };
        right = right.child(chip(l, status, label, true));
    }
    let agents = s.editor.state.presence.len();
    let authority = match agents {
        0 => "Authority ready".to_owned(),
        1 => "Authority ready · 1 agent connected".to_owned(),
        n => format!("Authority ready · {n} agents connected"),
    };
    let hint = text("W E R T tools · F frame · Alt+drag orbit · RMB+WASD fly · ⌘K commands")
        .text_size(11.0)
        .color(c.muted_foreground.0.with_alpha(0.8));
    row()
        .key("status")
        .h(22.0)
        .pad_x(10.0)
        .gap(14.0)
        .align(Align::Center)
        .fill(super::style::tones(l).gap)
        .child(
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(dot(l, Status::Success))
                .child(text(authority).text_size(11.0).color(c.muted_foreground.0)),
        )
        .child(
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(icon(icons::BOX).size(12.0).color(c.muted_foreground.0))
                .child(text(entities).text_size(11.0).color(c.muted_foreground.0)),
        )
        .child(el().grow(1.0))
        .child(hint)
        .child(right)
}

/// One palette result.
struct Entry {
    group: &'static str,
    icon: Option<stillkit::icons::Icon>,
    title: String,
    detail: String,
    keys: &'static str,
    run: Run,
}

enum Run {
    Command(&'static str),
    Select(String),
}

/// A fuzzy match of `query` in `title`: letters in order, rewarding runs and word starts.
pub(super) fn score(query: &str, title: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let title: Vec<char> = title.to_lowercase().chars().collect();
    let (mut at, mut total) = (0, 0);
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = title[at..].iter().position(|&c| c == q)? + at;
        let start = found == 0 || !title[found - 1].is_alphanumeric();
        total += 10 + if found == at { 6 } else { 0 } + if start { 12 } else { 0 }
            - (found - at).min(10) as i32;
        at = found + 1;
    }
    Some(total)
}

/// Commands the studio runs and the scene's entities, best first, at most 14.
fn results(s: &Studio, query: &str) -> Vec<Entry> {
    let entry = |group: &'static str, id: &'static str| {
        commands::spec(id)
            .filter(|_| commands::ported(id))
            .map(|spec| Entry {
                group,
                icon: icons::named(spec.icon),
                title: spec.title.to_owned(),
                detail: spec.summary.to_owned(),
                keys: spec.keys,
                run: Run::Command(spec.id),
            })
    };
    // With no query: what was run lately, then what fits the moment (I23).
    if query.is_empty() {
        let suggested: &[&'static str] = if s.editor.state.selection.is_empty() {
            &[
                "create.at_cursor",
                "view.frame_all",
                "edit.preferences",
                "layout.default",
                "window.history",
            ]
        } else {
            &[
                "view.frame",
                "edit.rename",
                "edit.duplicate",
                "create.empty_child",
                "create.at_cursor",
            ]
        };
        let recent = s
            .local
            .recent_commands
            .iter()
            .filter_map(|id| entry("Recent", id));
        let rest = suggested
            .iter()
            .filter(|id| !s.local.recent_commands.contains(id))
            .filter_map(|id| entry("Suggested", id));
        return recent.chain(rest).take(14).collect();
    }
    let mut all: Vec<(i32, Entry)> = Vec::new();
    for spec in crate::shell::commands::all()
        .iter()
        .filter(|spec| commands::ported(spec.id))
    {
        if let Some(sc) = score(query, spec.title) {
            // Recency only breaks ties between similar matches.
            let recent = if s.local.recent_commands.contains(&spec.id) {
                4
            } else {
                0
            };
            all.push((
                sc + 2 + recent,
                Entry {
                    group: "Commands",
                    icon: icons::named(spec.icon),
                    title: spec.title.to_owned(),
                    detail: spec.summary.to_owned(),
                    keys: spec.keys,
                    run: Run::Command(spec.id),
                },
            ));
        }
    }
    for (id, entity) in s.resolved.iter() {
        let name = entity
            .record
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(id);
        if let Some(sc) = score(query, name) {
            all.push((
                sc,
                Entry {
                    group: "Entities",
                    icon: Some(icons::BOX),
                    title: name.to_owned(),
                    detail: id.clone(),
                    keys: "",
                    run: Run::Select(id.clone()),
                },
            ));
        }
    }
    all.sort_by_key(|e| std::cmp::Reverse(e.0));
    all.into_iter().take(14).map(|(_, e)| e).collect()
}

fn run(s: &mut Studio) {
    let Some((query, active)) = s.local.palette.clone() else {
        return;
    };
    if let Some(entry) = results(s, &query).into_iter().nth(active) {
        s.local.palette = None;
        match entry.run {
            Run::Command(id) => {
                if id != "edit.delete" && id != "edit.palette" {
                    s.local.recent_commands.retain(|r| *r != id);
                    s.local.recent_commands.insert(0, id);
                    s.local.recent_commands.truncate(5);
                }
                commands::run(s, id, &Target::None)
            }
            Run::Select(id) => {
                s.select(vec![id], false);
                s.act("editor.frame", serde_json::json!({}));
            }
        }
    }
}

fn palette(s: &Studio, root: El<Studio>) -> El<Studio> {
    let Some((query, active)) = &s.local.palette else {
        return root;
    };
    let l = &s.look;
    let c = &l.c;
    let found = results(s, query);
    let active = (*active).min(found.len().saturating_sub(1));
    let field = row()
        .h(46.0)
        .pad_x(14.0)
        .gap(10.0)
        .align(Align::Center)
        .child(icon(icons::SEARCH).size(16.0).color(c.muted_foreground.0))
        .child(
            text_input("palette/query")
                .focus_ring(false)
                .grow(1.0)
                .placeholder("Run a command or jump to an entity")
                .value(query.as_str())
                .text_size(14.0)
                .color(c.foreground.0)
                .on_input(|s: &mut Studio, t| s.local.palette = Some((t.to_owned(), 0)))
                .on_submit(|s: &mut Studio, _| run(s))
                .on_key(|s: &mut Studio, key, _| {
                    let Some((query, active)) = s.local.palette.clone() else {
                        return Handled::No;
                    };
                    let n = results(s, &query).len().max(1);
                    let next = match key {
                        KeyCode::DOWN => (active + 1) % n,
                        KeyCode::UP => (active + n - 1) % n,
                        KeyCode::ESCAPE => {
                            s.local.palette = None;
                            return Handled::Yes;
                        }
                        _ => return Handled::No,
                    };
                    s.local.palette = Some((query, next));
                    Handled::Yes
                }),
        )
        .child(kbd(l, "esc"));
    let mut list = col().pad(6.0).gap(1.0).max_h(440.0).scroll(Axis::Vertical);
    let mut group = "";
    for (i, e) in found.iter().enumerate() {
        if e.group != group {
            group = e.group;
            list = list.child(
                row().h(24.0).pad_x(10.0).align(Align::End).child(
                    text(group)
                        .text_size(11.0)
                        .weight(600)
                        .color(c.muted_foreground.0),
                ),
            );
        }
        let on = i == active;
        let mut item = row()
            .key(format!("palette/{i}"))
            .h(34.0)
            .pad_x(10.0)
            .gap(10.0)
            .align(Align::Center)
            .radius(l.radius.sm)
            .role(Role::ListBoxOption)
            .selected(on)
            .fill(if on {
                c.selection_surface.0
            } else {
                stillkit::Rgba::TRANSPARENT
            })
            .color(if on {
                c.selection_foreground.0
            } else {
                c.foreground.0
            })
            .cascade_ink()
            .on_press(move |s: &mut Studio| {
                if let Some((query, _)) = s.local.palette.clone() {
                    s.local.palette = Some((query, i));
                }
                run(s);
            })
            .child(el().size(16.0).children(e.icon.map(|g| icon(g).size(15.0))))
            .child(text(e.title.clone()).text_size(12.5).weight(520))
            .child(
                text(e.detail.clone())
                    .text_size(11.5)
                    .color(if on {
                        c.selection_muted_foreground.0
                    } else {
                        c.muted_foreground.0
                    })
                    .grow(1.0),
            );
        if let Some(k) = commands::shortcut(e.keys) {
            item = item.child(kbd(l, &k));
        }
        list = list.child(item);
    }
    let panel = col()
        .key("palette")
        .w(640.0)
        .radius(l.radius.base + 2.0)
        .fill(c.popover.0)
        .shadow_level(ShadowLevel::Dialog)
        .role(Role::Dialog)
        .label("Command palette")
        .child(field)
        .child(list);
    let placed = col()
        .w_full()
        .h_full()
        .align(Align::Center)
        .child(el().h(64.0))
        .child(panel);
    root.overlay(dialog(placed).on_dismiss(|s: &mut Studio| s.local.palette = None))
}
