//! The studio window's frame: the title bar (menus from the command registry, Play
//! controls, the scene path, the command launcher), the dock in between, the status bar, and the
//! overlays they open (menus, the command palette).
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::Role;
use stillkit::el::{
    Align, Axis, El, Handled, KeyCode, col, dialog, el, icon, menu, row, text, text_input,
};

use super::{Studio, commands, dock};
use crate::catalog::Target;
use crate::catalog::menus::{Item, MENUS};

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
        .children(dock::floating(s))
        .children(drag_chip(s))
        .children(dock::drag_preview(s))
        // Menus at a point hang from an element of no size: an overlay never closes on a
        // press over the element it hangs from, and the root covers every press.
        .child(
            el().key("studio/menus")
                .absolute(0.0, 0.0)
                .size(0.0)
                .overlay(context_menu(s))
                .overlay(super::create::overlay(
                    s,
                    super::create::Anchor::Point(0.0, 0.0),
                )),
        )
        .overlay(super::preferences::overlay(s))
        .overlay(prompt(s))
        .overlay(why(s))
        .overlay(super::about::overlay(s))
        // Last: the import card sits over everything while the gate holds.
        .overlay(super::loading::overlay(s));
    palette(s, root)
}

/// The open one-field question (Save Layout As): Enter runs its command with the text.
fn prompt(s: &Studio) -> Option<stillkit::el::Overlay<Studio>> {
    let p = s.local.prompt.as_ref()?;
    let l = &s.look;
    let then = p.then;
    let submit = move |s: &mut Studio, text: &str| {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        s.local.prompt = None;
        commands::run(s, then, &Target::Text(text));
    };
    let close = |s: &mut Studio| s.local.prompt = None;
    let typed = p.text.clone();
    let panel = dialog_panel(
        l,
        None,
        p.title,
        field(
            l,
            Size::Md,
            "prompt/field",
            None,
            p.hint,
            &p.text,
            move |f| {
                f.on_input(|s: &mut Studio, t| {
                    if let Some(p) = s.local.prompt.as_mut() {
                        p.text = t.to_owned();
                    }
                })
                .on_submit(submit)
            },
        ),
        vec![
            button(l, Tone::Ghost, Size::Md, None, "Cancel")
                .key("prompt/cancel")
                .on_press(close),
            enabled(
                button(l, Tone::Primary, Size::Md, None, p.ok)
                    .key("prompt/ok")
                    .on_press(move |s: &mut Studio| submit(s, &typed)),
                !p.text.trim().is_empty(),
            ),
        ],
        close,
    )
    .w(380.0);
    Some(stillkit::el::dialog(panel).on_dismiss(close))
}

/// The diagnostic service's answer (Tools › Why?), line by line, with Ask Again.
fn why(s: &Studio) -> Option<stillkit::el::Overlay<Studio>> {
    let (question, lines) = s.local.why.as_ref()?;
    let l = &s.look;
    let close = |s: &mut Studio| s.local.why = None;
    let body = col().gap(4.0).children(lines.iter().map(|line| {
        text(line.clone())
            .text_size(12.0)
            .wrap(52)
            .color(l.c.foreground.0)
    }));
    let again = question.clone();
    let panel = dialog_panel(
        l,
        None,
        &format!("Why? · {question}"),
        body,
        vec![
            button(l, Tone::Ghost, Size::Md, None, "Ask again")
                .key("why/again")
                .on_press(move |s: &mut Studio| {
                    s.local.why = None;
                    commands::run(s, "tools.why", &Target::Text(String::new()));
                    if let Some(p) = s.local.prompt.as_mut() {
                        p.text = again.clone();
                    }
                }),
            button(l, Tone::Primary, Size::Md, None, "Close")
                .key("why/close")
                .on_press(close),
        ],
        close,
    )
    .w(520.0);
    Some(stillkit::el::dialog(panel).on_dismiss(close))
}

/// What an asset drag carries, beside the pointer.
fn drag_chip(s: &Studio) -> Option<El<Studio>> {
    let (_, name, (x, y)) = s.local.asset_drag.as_ref()?;
    let l = &s.look;
    let c = &l.c;
    Some(
        super::style::float(l, row())
            .absolute(x + 14.0, y + 10.0)
            .h(26.0)
            .pad_x(10.0)
            .gap(6.0)
            .align(Align::Center)
            .radius(l.radius.sm)
            .child(icon(icons::BOX).size(13.0).color(c.primary.0))
            .child(text(name.clone()).text_size(12.0).color(c.foreground.0)),
    )
}

/// The open context menu at the pointer: the registry's layout for what was clicked, its
/// commands run on that target.
fn context_menu(s: &Studio) -> Option<stillkit::el::Overlay<Studio>> {
    let (kind, target, (x, y)) = s.local.context.clone()?;
    let items = crate::catalog::menus::context(kind);
    Some(
        menu(cascade(s, items, &target, y))
            .at(x, y)
            .on_dismiss(|s: &mut Studio| s.local.context = None),
    )
}

/// Window-wide shortcuts: the registry's chords for the commands the studio runs. Keys a
/// focused field takes (typing, its own Enter) never reach here.
fn keys(s: &mut Studio, key: KeyCode, tx: &mut stillkit::el::Tx) -> Handled {
    match commands::for_key(s, key.0, tx.modifiers()) {
        Some(id) => {
            commands::run(s, &id, &Target::None);
            if id == "edit.palette" {
                tx.focus("palette/query");
            }
            Handled::Yes
        }
        None => Handled::No,
    }
}

/// The command launcher's chord as this platform writes it: ⌘K on macOS, Ctrl K elsewhere.
pub(super) fn launcher_chord() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘K"
    } else {
        "Ctrl K"
    }
}

/// The status bar's keys worth knowing, with the launcher chord in the platform's own words.
pub(super) fn idle_hint() -> String {
    format!(
        "W E R T tools · F frame · Alt+drag orbit · RMB+WASD fly · {} commands",
        launcher_chord()
    )
}

fn title_bar(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    // The engine's mark, as the legacy editor's chrome and the loading screen show it.
    let mark = match ara_kit::brand::mark(ara_kit::brand::Mark::Engine, 60) {
        Some(mark) => stillkit::el::image(mark).size(20.0),
        None => el().size(20.0),
    };
    // The menu bar is one Tab stop; Left and Right move between its menus.
    let menus = row()
        .gap(4.0)
        .align(Align::Center)
        .role(stillkit::Role::MenuBar)
        .focus_group(stillkit::el::Orientation::Horizontal)
        .children(MENUS.iter().map(|m| {
            if m.title == "Create" {
                create_button(s)
            } else {
                menu_button(s, m.title, m.items)
            }
        }));
    let lead = vec![mark.label("Ara"), menus];
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
    let project = super::loading::project_name(&s.editor);
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
        .child(kbd(l, launcher_chord()));
    // Always a little air from the run controls, however narrow the window.
    let crumb = row()
        .gap(6.0)
        .pad_x(8.0)
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
    // Menus, Play, project, search and the window buttons; the theme is in Settings.
    ara_kit::ui::title_bar_flush(
        l,
        s.frame.inset,
        lead,
        vec![play],
        vec![crumb, launcher],
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
    b.overlay(super::create::overlay(
        s,
        super::create::Anchor::Key("menu/Create"),
    ))
}

/// A title-bar menu: its name, and its registry items while open.
fn menu_button(s: &Studio, title: &'static str, items: &'static [Item]) -> El<Studio> {
    let l = &s.look;
    let open = s.local.menu == Some(title);
    let mut b = button(l, Tone::Ghost, Size::Sm, None, title)
        .key(format!("menu/{title}"))
        .expanded(open)
        .on_press(move |s: &mut Studio| {
            s.local.submenu = None;
            s.local.menu = if s.local.menu == Some(title) {
                None
            } else {
                Some(title)
            }
        });
    if open {
        b = b.fill(l.c.raised.0).color(l.c.foreground.0);
        // Title-bar menus hang under the 40 px title bar.
        let panel = cascade(s, items, &Target::None, 40.0);
        b = b.overlay(menu(panel).on_dismiss(|s: &mut Studio| s.local.menu = None));
    }
    b
}

/// One line of a flattened menu.
#[derive(Clone)]
enum Line {
    Separator,
    Heading(&'static str),
    Command(&'static str),
    /// A submenu that opens beside its row: its title and items.
    Sub(&'static str, &'static [Item]),
    /// A generated entry (a scene, a layout, a profile): its title, the command it runs
    /// with its target, and whether it is the current one.
    Entry(String, &'static str, Target, Option<bool>),
}

/// Flattens registry items: a menu's own submenus cascade beside it (`Line::Sub`); one
/// level further in they become a separator and a heading over their entries; dynamic
/// items become an entry each (`target` is what the menu acts on).
fn lines(s: &Studio, items: &'static [Item], depth: usize, target: &Target, out: &mut Vec<Line>) {
    for item in items {
        match item {
            Item::Separator => out.push(Line::Separator),
            // Only what the editor runs; a context menu, only what applies to what was
            // clicked (the menu bar keeps every entry).
            // Window › a core panel: checked while shown, and a press hides or shows it (its
            // tab has no ×).
            Item::Command(
                id @ ("window.hierarchy" | "window.inspector" | "window.game" | "window.assets"),
            ) if *target == Target::None => {
                let panel = id.trim_start_matches("window.");
                let shown = crate::state::panel_visible(&s.editor.state, panel);
                let title = super::dock::panel_title(panel).0.to_owned();
                out.push(if shown {
                    Line::Entry(
                        title,
                        "window.close_panel",
                        Target::Panel(panel.to_owned()),
                        Some(true),
                    )
                } else {
                    Line::Entry(title, id, Target::None, Some(false))
                });
            }
            Item::Command(id)
                if crate::catalog::get(id).is_some()
                    && (*target == Target::None || commands::applies(s, id, target)) =>
            {
                out.push(Line::Command(id))
            }
            // A cascading submenu, when it has something to show for this target.
            Item::Sub(title, sub) if depth == 0 => {
                let mut inner = Vec::new();
                lines(s, sub, 1, target, &mut inner);
                if inner
                    .iter()
                    .any(|l| matches!(l, Line::Command(_) | Line::Entry(..)))
                {
                    out.push(Line::Sub(title, sub));
                }
            }
            Item::Sub(title, sub) if depth < 3 => {
                out.push(Line::Separator);
                out.push(Line::Heading(title));
                lines(s, sub, depth + 1, target, out);
            }
            Item::Dynamic(kind) => out.extend(dynamic(s, *kind, target)),
            Item::Command(_) | Item::Sub(..) => {}
        }
    }
}

/// The entries a dynamic item stands for, now.
fn dynamic(s: &Studio, kind: crate::catalog::menus::Dynamic, target: &Target) -> Vec<Line> {
    use crate::catalog::menus::Dynamic;
    let e = &s.editor;
    let command = kind.command();
    let entry = |title: String, target: Target, checked: Option<bool>| {
        Line::Entry(title, command, target, checked)
    };
    match kind {
        Dynamic::RecentScenes => {
            let workspace = e.host.store().workspace();
            let mut scenes: Vec<String> = workspace
                .docs()
                .filter(|(_, d)| d.kind == "scene")
                .map(|(_, d)| d.id.to_string())
                .collect();
            // Recently opened first, then the rest by path.
            scenes.sort_by_key(|id| {
                (
                    e.state
                        .recent_scenes
                        .iter()
                        .position(|r| r == id)
                        .unwrap_or(usize::MAX),
                    workspace.path_of(id).unwrap_or("").to_owned(),
                )
            });
            scenes
                .into_iter()
                .take(12)
                .map(|id| {
                    let path = workspace.path_of(&id).unwrap_or(&id);
                    let path = workspace.reference_path(path).to_owned();
                    let open = id == e.state.doc;
                    entry(path, Target::Doc(id), Some(open))
                })
                .collect()
        }
        Dynamic::SavedLayouts => crate::state::saved_layouts(e.host.root())
            .into_iter()
            .map(|name| entry(name.clone(), Target::Text(name), None))
            .collect(),
        Dynamic::BuildProfiles => {
            let names = s
                .assets
                .as_ref()
                .map(|b| crate::build::profiles(&b.files))
                .unwrap_or_default();
            let setting = e
                .state
                .settings
                .get(crate::build::PROFILE_SETTING)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let chosen = crate::build::selected_profile(&names, setting)
                .ok()
                .map(str::to_owned);
            names
                .into_iter()
                .map(|name| {
                    let on = chosen.as_deref() == Some(name.as_str());
                    entry(name.clone(), Target::Text(name), Some(on))
                })
                .collect()
        }
        Dynamic::PrefabSources => {
            let Some(entity) = target.entity(&e.state) else {
                return Vec::new();
            };
            let path = match target {
                Target::Field(path) => Some(path.as_str()),
                _ => None,
            };
            crate::prefab::source_choices(e.host.store(), &e.state.doc, entity)
                .into_iter()
                .map(|source| {
                    let logical = e.host.store().workspace().reference_path(&source.path).to_owned();
                    let choice = serde_json::json!({"entity": entity, "path": path, "destination": format!("@{logical}#{}", source.doc)});
                    entry(logical, Target::Text(choice.to_string()), None)
                })
                .collect()
        }
        // Floating windows, by the panels they hold; the front one is checked.
        Dynamic::Windows => {
            let front = e.state.floating.last().map(|f| f.id.clone());
            e.state
                .floating
                .iter()
                .map(|f| {
                    let title = super::dock::window_title(&f.root);
                    entry(
                        title,
                        Target::Window(f.id.clone()),
                        Some(front.as_ref() == Some(&f.id)),
                    )
                })
                .collect()
        }
        // Panels not open yet, to open as a tab beside the clicked one.
        Dynamic::AddTab => {
            let Target::Panel(beside) = target else {
                return Vec::new();
            };
            super::dock::PANELS
                .iter()
                .filter(|p| !crate::state::panel_visible(&e.state, p))
                .map(|p| {
                    let (title, _) = super::dock::panel_title(p);
                    entry(
                        title.to_owned(),
                        Target::Text(format!("{p}@{beside}")),
                        None,
                    )
                })
                .collect()
        }
    }
}

/// Menu entries for registry items, with no empty headings and no leading, trailing or
/// doubled separators; commands act on `target`.
/// A menu's rows from `items` at submenu `depth` (0: the menu itself), its empty
/// headings and doubled separators dropped.
fn clean_lines(s: &Studio, items: &'static [Item], depth: usize, target: &Target) -> Vec<Line> {
    let mut all = Vec::new();
    lines(s, items, depth, target, &mut all);
    let entry = |line: Option<&Line>| {
        matches!(
            line,
            Some(Line::Command(_) | Line::Entry(..) | Line::Sub(..))
        )
    };
    let mut clean: Vec<Line> = Vec::new();
    for (i, line) in all.iter().enumerate() {
        match line {
            Line::Heading(_) if !entry(all.get(i + 1)) => {}
            Line::Separator if !entry(clean.last()) => {}
            _ => clean.push(line.clone()),
        }
    }
    while matches!(clean.last(), Some(Line::Separator | Line::Heading(_))) {
        clean.pop();
    }
    clean
}

/// A menu and, beside it at its row, the submenu open from it: both in one menu overlay,
/// so the pointer moves between them, and hovering another row of the menu closes it.
/// `from` is where the menu's top sits in the window: a submenu that would run off the
/// window's bottom rises instead, so the menu never moves under the pointer.
fn cascade(s: &Studio, items: &'static [Item], target: &Target, from: f32) -> El<Studio> {
    let l = &s.look;
    let lines = clean_lines(s, items, 0, target);
    // Where the open submenu's row sits: menu rows are 26 px, separators 7, headings 24,
    // a 1 px gap between, inside the panel's 4 px padding.
    let mut top: f32 = 0.0;
    let mut open = None;
    for line in &lines {
        if let Line::Sub(title, sub) = line
            && s.local.submenu == Some(*title)
        {
            open = Some(*sub);
            break;
        }
        top += 1.0
            + match line {
                Line::Separator => 7.0,
                Line::Heading(_) => 24.0,
                _ => 26.0,
            };
    }
    let height = |lines: &[Line]| {
        8.0 + lines
            .iter()
            .map(|line| match line {
                Line::Separator => 8.0,
                Line::Heading(_) => 25.0,
                _ => 27.0,
            })
            .sum::<f32>()
    };
    let menu_height = height(&lines).min(560.0);
    let menu = menu_panel(l, render(s, lines, 0, target))
        .max_h(560.0)
        .scroll(Axis::Vertical);
    let sub = open.map(|sub| {
        let sub_lines = clean_lines(s, sub, 1, target);
        // Room under the menu's top, within the window: the submenu fits in it (scrolling
        // past it), beside its row where it can, risen where it would run off.
        // (stillkit flips a menu that would come within a few px of the bottom.)
        let room = (s.size.1 - from - 24.0)
            .clamp(120.0, 560.0)
            .max(menu_height);
        let sub_height = height(&sub_lines).min(room);
        let top = top.min(room - sub_height).max(0.0);
        col().child(el().h(top)).child(
            menu_panel(l, render(s, sub_lines, 1, target))
                .max_h(room)
                .scroll(Axis::Vertical),
        )
    });
    row().gap(2.0).align(Align::Start).child(menu).children(sub)
}

/// The rows of a menu's `lines` at submenu `depth`.
fn render(s: &Studio, clean: Vec<Line>, depth: usize, target: &Target) -> Vec<El<Studio>> {
    let l = &s.look;
    // In a menu's own rows, hovering an entry closes a submenu open beside it, so its
    // press runs it rather than only closing the submenu.
    let leave = |item: El<Studio>| match depth {
        0 => item.on_pointer(|s: &mut Studio, e, _| {
            let closes = e.phase == stillkit::el::PointerPhase::Hover && s.local.submenu.is_some();
            if closes {
                s.local.submenu = None;
            }
            closes
        }),
        _ => item,
    };
    clean
        .into_iter()
        .filter_map(|line| match line {
            Line::Separator => Some(menu_separator(l)),
            Line::Heading(title) => Some(menu_heading(l, title)),
            Line::Command(id) => command_item(s, id, target).map(leave),
            Line::Sub(title, _) => Some(sub_item(s, title)),
            Line::Entry(title, id, target, checked) => {
                let key = format!("cmd/{id}/{title}");
                let item = match checked {
                    Some(on) => menu_check(l, &key, &title, on),
                    None => menu_item(l, &key, None, &title, None, false),
                };
                Some(leave(item.on_press(move |s: &mut Studio| {
                    s.local.menu = None;
                    s.local.context = None;
                    commands::run(s, id, &target);
                })))
            }
        })
        .collect()
}

/// A submenu's row: its title and a chevron; hovering or pressing it opens its entries
/// beside it (a press never closes it: the pointer that pressed has already opened it),
/// as Unity's and Unreal's menus cascade.
fn sub_item(s: &Studio, title: &'static str) -> El<Studio> {
    let l = &s.look;
    let open = s.local.submenu == Some(title);
    let lit = super::style::tones(l).hover;
    menu_item(l, &format!("sub/{title}"), None, title, None, false)
        .expanded(open)
        // Lit while its submenu shows.
        .fill(if open {
            lit
        } else {
            stillkit::Rgba::TRANSPARENT
        })
        // A handled hover redraws the menu (an unhandled one leaves the view as it was).
        .on_pointer(move |s: &mut Studio, e, _| {
            let opens =
                e.phase == stillkit::el::PointerPhase::Hover && s.local.submenu != Some(title);
            if opens {
                s.local.submenu = Some(title);
            }
            opens
        })
        .on_press(move |s: &mut Studio| s.local.submenu = Some(title))
        .child(
            icon(icons::CHEVRON_RIGHT)
                .size(12.0)
                .color(l.c.muted_foreground.0),
        )
}

/// One command as a menu item: its icon, title, shortcut and summary.
fn command_item(s: &Studio, id: &'static str, target: &Target) -> Option<El<Studio>> {
    let target = target.clone();
    let l = &s.look;
    let spec = commands::spec(id)?;
    let glyph = icons::named(spec.icon);
    let keys = commands::keys_of(s, id);
    // The OS file manager by its own name.
    let title = match id {
        "assets.reveal" if cfg!(target_os = "macos") => "Show in Finder",
        _ => spec.title,
    };
    // Not built yet: dimmed, its reason on hover, and a press says it in the status bar.
    if let Some(reason) = crate::catalog::later(id) {
        let item = menu_item(l, &format!("cmd/{id}"), glyph, title, None, false)
            .color(l.c.muted_foreground.0)
            .tooltip(reason)
            .on_press(move |s: &mut Studio| {
                s.local.menu = None;
                s.local.context = None;
                commands::run(s, id, &Target::None);
            });
        return Some(item.description(reason));
    }
    let item = menu_item(
        l,
        &format!("cmd/{id}"),
        glyph,
        title,
        keys.as_deref(),
        id == "edit.delete",
    )
    .on_press(move |s: &mut Studio| {
        s.local.menu = None;
        s.local.context = None;
        commands::run(s, id, &target);
    });
    Some(item.description(spec.summary))
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
    group = group.child(play_menu(s));
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

/// Play's options beside Stop (Unreal's Play dropdown): the mode, clients and their network
/// conditions, Start Paused, and Simulate; while Play runs, Eject or Possess, reading the
/// selection's running values, and keeping runtime changes.
fn play_menu(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let open = s.local.play_menu;
    let mut trigger = icon_button(
        l,
        Tone::Ghost,
        Size::Sm,
        icons::CHEVRON_DOWN,
        "Play options",
    )
    .key("play/options")
    .expanded(open)
    .on_press(|s: &mut Studio| s.local.play_menu = !s.local.play_menu);
    if !open {
        return trigger;
    }
    let run = |id: &'static str| {
        move |s: &mut Studio| {
            s.local.play_menu = false;
            commands::run(s, id, &Target::None);
        }
    };
    // Settings stay open on change, so several can be set in one visit.
    let set = |id: &'static str| move |s: &mut Studio| commands::run(s, id, &Target::None);
    let settings = &s.editor.state.settings;
    let mode = settings
        .get("play.mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("local");
    let clients = settings
        .get("play.clients")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0) as usize;
    let paused = settings
        .get("play.startPaused")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let playing = s.editor.state.playing.is_some();
    let mut items = vec![menu_heading(l, "Play as")];
    for (value, title, id) in [
        ("local", "Local", "play.mode.local"),
        ("host", "Host and clients", "play.mode.host"),
        (
            "dedicated",
            "Dedicated server and clients",
            "play.mode.dedicated",
        ),
    ] {
        items.push(enabled(
            menu_check(l, &format!("play/mode/{value}"), title, mode == value).on_press(set(id)),
            !playing,
        ));
    }
    if mode != "local" {
        items.push(
            row()
                .key("play/clients")
                .h(28.0)
                .pad_x(8.0)
                .gap(6.0)
                .align(Align::Center)
                .child(
                    text("Clients")
                        .text_size(12.5)
                        .color(c.foreground.0)
                        .grow(1.0),
                )
                .child(enabled(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::MINUS, "Fewer clients")
                        .key("play/clients/fewer")
                        .on_press(set("play.clients.fewer")),
                    !playing && clients > 1,
                ))
                .child(
                    text(clients.to_string())
                        .text_size(12.5)
                        .family(mono())
                        .color(c.foreground.0),
                )
                .child(enabled(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, "More clients")
                        .key("play/clients/more")
                        .on_press(set("play.clients.more")),
                    !playing && clients < 16,
                )),
        );
        // Each client's network conditions: a press moves to the next profile.
        for player in 1..=clients.min(16) {
            let current = crate::play_controls::network_profile(&s.editor, player);
            let at = crate::play_controls::PROFILES
                .iter()
                .position(|(name, _)| *name == current)
                .unwrap_or(0);
            let (_, label) = crate::play_controls::PROFILES[at];
            let (next, _) =
                crate::play_controls::PROFILES[(at + 1) % crate::play_controls::PROFILES.len()];
            items.push(enabled(
                row()
                    .key(format!("play/network/{player}"))
                    .h(26.0)
                    .pad_x(8.0)
                    .gap(8.0)
                    .align(Align::Center)
                    .radius(l.radius.sm)
                    .hovered({
                        let hover = c.hover_surface.0;
                        move |e| e.fill(hover)
                    })
                    .label(format!("Client {player} network: {label}"))
                    .tooltip("Network conditions for the next Play: press for the next")
                    .on_press(move |s: &mut Studio| {
                        if let Err(error) =
                            crate::play_controls::set_network_profile(&mut s.editor, player, next)
                        {
                            s.editor.report_failure(&*anyhow::anyhow!("{error}"));
                        }
                    })
                    .child(icon(icons::WIFI).size(13.0).color(c.muted_foreground.0))
                    .child(
                        text(format!("Client {player}"))
                            .text_size(12.0)
                            .color(c.foreground.0)
                            .grow(1.0),
                    )
                    .child(text(label).text_size(11.5).color(c.muted_foreground.0)),
                !playing,
            ));
        }
    }
    items.push(enabled(
        menu_check(l, "play/paused", "Start paused", paused).on_press(set("play.start_paused")),
        !playing,
    ));
    items.push(menu_separator(l));
    let busy = s.play.busy();
    if playing {
        let ejected = s.play.ejected();
        items.push(enabled(
            menu_item(
                l,
                "play/eject",
                Some(icons::VIDEO),
                if ejected {
                    "Ejected: possess the selection to return"
                } else {
                    "Eject"
                },
                Some("F8"),
                false,
            )
            .on_press(run("play.eject")),
            !busy && !ejected,
        ));
        items.push(enabled(
            menu_item(
                l,
                "play/possess",
                Some(icons::USER),
                "Possess Selected",
                None,
                false,
            )
            .on_press(run("play.possess")),
            !busy && !s.editor.state.selection.is_empty(),
        ));
        items.push(menu_separator(l));
        for (key, glyph, title, id) in [
            (
                "play/inspect",
                icons::SCAN_EYE,
                "Read Running Values",
                "play.inspect",
            ),
            (
                "play/keep",
                icons::SAVE,
                "Keep Runtime Changes…",
                "play.keep",
            ),
            (
                "play/keep-driven",
                icons::SAVE,
                "Keep Including Driven Values…",
                "play.keep_driven",
            ),
        ] {
            items.push(enabled(
                menu_item(l, key, Some(glyph), title, None, false).on_press(run(id)),
                !busy && !s.editor.state.selection.is_empty(),
            ));
        }
    } else {
        items.push(
            menu_item(
                l,
                "play/simulate",
                Some(icons::PLAY),
                "Simulate",
                Some("Alt+F5"),
                false,
            )
            .tooltip("Run the scene through a spectator camera, with no player actor")
            .on_press(run("play.simulate")),
        );
    }
    items.push(
        menu_item(
            l,
            "play/settings",
            Some(icons::SETTINGS),
            "Play Settings…",
            None,
            false,
        )
        .on_press(run("play.settings")),
    );
    if let Some(message) = &s.play.message {
        items.push(
            row().pad_x(8.0).pad_y(6.0).max_w(300.0).child(
                text(message.clone())
                    .text_size(11.5)
                    .wrap(4)
                    .color(c.muted_foreground.0),
            ),
        );
    }
    trigger = trigger.overlay(
        stillkit::el::menu(menu_panel(l, items).min_w(280.0))
            .on_dismiss(|s: &mut Studio| s.local.play_menu = false),
    );
    trigger
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
    // What the last request did, for a few seconds; else the newest warning or error
    // (Unity's status bar: a press opens it in the Console); else the keys worth knowing.
    let latest = s.logs.iter().rev().find(|r| {
        matches!(
            r.severity,
            ara_diag::Severity::Error | ara_diag::Severity::Warning
        )
    });
    let hint = match (&s.local.note, latest) {
        (Some((note, at)), _) if at.elapsed() < std::time::Duration::from_secs(8) => {
            text(note.clone())
                .key("status/note")
                .text_size(11.0)
                .color(c.foreground.0)
        }
        (_, Some(r)) => {
            let error = r.severity == ara_diag::Severity::Error;
            let seq = r.seq;
            let hover = super::style::tones(l).hover;
            row()
                .key("status/log")
                .min_w(0.0)
                .max_w(520.0)
                .gap(6.0)
                .pad_x(6.0)
                .h(18.0)
                .radius(super::style::RADIUS)
                .align(Align::Center)
                .label(format!(
                    "{}: {}",
                    if error { "Error" } else { "Warning" },
                    r.message
                ))
                .tooltip("Show it in the Console")
                .hovered(move |e| e.fill(hover))
                .on_press(move |s: &mut Studio| {
                    s.local.console_open = Some(seq);
                    s.act(
                        "editor.layout.show",
                        serde_json::json!({"panel": "console"}),
                    );
                })
                .child(
                    icon(if error {
                        icons::CIRCLE_X
                    } else {
                        icons::TRIANGLE_ALERT
                    })
                    .size(12.0)
                    .color(if error {
                        c.destructive_foreground.0
                    } else {
                        c.warning_foreground.0
                    }),
                )
                .child(
                    text(r.message.clone())
                        .text_size(11.0)
                        .wrap(1)
                        .color(c.foreground.0)
                        .min_w(0.0),
                )
        }
        _ => text(idle_hint())
            .text_size(11.0)
            .color(c.muted_foreground.0.with_alpha(0.8)),
    };
    let bar = row()
        .key("status")
        // Its menu: the windows the status bar reports on.
        .on_context_menu(|s: &mut Studio, x, y| {
            s.local.context = Some((
                crate::catalog::menus::Context::Status,
                crate::catalog::Target::None,
                (x, y),
            ));
        })
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
        .child(right);
    match super::scripts::chooser(s) {
        Some(chooser) => bar.overlay(chooser),
        None => bar,
    }
}

/// One palette result.
struct Entry {
    group: &'static str,
    icon: Option<stillkit::icons::Icon>,
    title: String,
    detail: String,
    /// The binding in force, as this platform writes it.
    keys: Option<String>,
    run: Run,
}

enum Run {
    Command(&'static str),
    Select(String),
    /// Open this scene document.
    Scene(String),
    /// Open Settings at this setting (its scope, searched by its label).
    Setting(&'static crate::settings::Def),
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
            .filter(|_| commands::ported(id) && crate::catalog::later(id).is_none())
            .map(|spec| Entry {
                group,
                icon: icons::named(spec.icon),
                title: spec.title.to_owned(),
                detail: spec.summary.to_owned(),
                keys: commands::keys_of(s, spec.id),
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
    for spec in crate::catalog::all() {
        if crate::catalog::later(spec.id).is_some() {
            continue;
        }
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
                    keys: commands::keys_of(s, spec.id),
                    run: Run::Command(spec.id),
                },
            ));
        }
    }
    // Scenes in the project; `#` lists only them (File › Open Scene).
    let (scene_query, scenes_only) = match query.strip_prefix('#') {
        Some(rest) => (rest.trim(), true),
        None => (query, false),
    };
    let workspace = s.editor.host.store().workspace();
    for (doc, document) in workspace.docs() {
        if document.kind != "scene" {
            continue;
        }
        let path = workspace.path_of(doc).unwrap_or_default();
        let name = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".scene");
        if let Some(sc) = score(scene_query, name) {
            all.push((
                sc + if scenes_only { 20 } else { 1 },
                Entry {
                    group: "Scenes",
                    icon: Some(icons::CLAPPERBOARD),
                    title: name.to_owned(),
                    detail: path.to_owned(),
                    keys: None,
                    run: Run::Scene(doc.to_string()),
                },
            ));
        }
    }
    if scenes_only {
        all.sort_by_key(|e| std::cmp::Reverse(e.0));
        return all.into_iter().take(14).map(|(_, e)| e).collect();
    }
    // Settings, by label (VS Code's and Unity's search reach them too).
    for def in crate::settings::DEFS {
        if let Some(sc) = score(query, def.label) {
            all.push((
                sc,
                Entry {
                    group: "Settings",
                    icon: Some(icons::SETTINGS_2),
                    title: def.label.to_owned(),
                    detail: format!("{} · {}", def.category, def.description),
                    keys: None,
                    run: Run::Setting(def),
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
                    keys: None,
                    run: Run::Select(id.clone()),
                },
            ));
        }
    }
    all.sort_by_key(|e| std::cmp::Reverse(e.0));
    // Best first, at most 8 of a kind (so settings and entities still show beside many
    // commands), each group together where its best result ranks.
    let mut taken: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut best: Vec<Entry> = all
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| {
            let n = taken.entry(e.group).or_default();
            *n += 1;
            *n <= 8
        })
        .take(14)
        .collect();
    let mut order: Vec<&'static str> = Vec::new();
    for entry in &best {
        if !order.contains(&entry.group) {
            order.push(entry.group);
        }
    }
    best.sort_by_key(|e| order.iter().position(|g| *g == e.group));
    best
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
            Run::Scene(doc) => {
                s.act("editor.scene.open", serde_json::json!({"scene": doc}));
                s.resolve();
                s.act("editor.focus", serde_json::json!({"panel": "scene"}));
            }
            Run::Setting(def) => {
                super::preferences::open(s, def.scope);
                if let Some(view) = s.local.settings.as_mut() {
                    view.query = def.label.to_owned();
                }
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
                super::style::active_fill(l)
            } else {
                stillkit::Rgba::TRANSPARENT
            })
            .color(c.foreground.0)
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
                    .color(c.muted_foreground.0)
                    .grow(1.0),
            );
        if let Some(k) = &e.keys {
            item = item.child(kbd(l, k));
        }
        list = list.child(item);
    }
    let panel = super::style::float(l, col())
        .key("palette")
        .w(640.0)
        .radius(l.radius.base + 2.0)
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
