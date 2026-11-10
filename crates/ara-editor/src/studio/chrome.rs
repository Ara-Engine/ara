//! The studio window's frame: the title bar (menus from the command registry, Play
//! controls, the scene path, the ⌘K launcher), the dock in between, the status bar, and the
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
        .overlay(super::about::overlay(s));
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
            "Name",
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
                button(l, Tone::Primary, Size::Md, None, "Save")
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
    let panel = menu_panel(&s.look, entries(s, items, &target))
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
    // The engine's mark, as the legacy editor's chrome and the loading screen show it.
    let mark = match ara_kit::brand::mark(ara_kit::brand::Mark::Engine, 60) {
        Some(mark) => stillkit::el::image(mark).size(20.0),
        None => el().size(20.0),
    };
    let mut lead = vec![mark.label("Ara")];
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
            s.local.menu = if s.local.menu == Some(title) {
                None
            } else {
                Some(title)
            }
        });
    if open {
        b = b.fill(l.c.raised.0).color(l.c.foreground.0);
        let panel = menu_panel(l, entries(s, items, &Target::None))
            .max_h(560.0)
            .scroll(Axis::Vertical);
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
    /// A generated entry (a scene, a layout, a profile): its title, the command it runs
    /// with its target, and whether it is the current one.
    Entry(String, &'static str, Target, Option<bool>),
}

/// Flattens registry items: a submenu becomes a separator and a heading over its entries;
/// dynamic items become an entry each (`target` is what the menu acts on).
fn lines(s: &Studio, items: &'static [Item], depth: usize, target: &Target, out: &mut Vec<Line>) {
    for item in items {
        match item {
            Item::Separator => out.push(Line::Separator),
            // Only what the editor runs is listed.
            Item::Command(id) if crate::catalog::get(id).is_some() => out.push(Line::Command(id)),
            Item::Sub(title, sub) if depth < 2 => {
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
        Dynamic::GizmoTypes => {
            let settings = crate::shell::overlay::gizmos::Settings::read(&e.state);
            crate::shell::overlay::gizmos::TYPES
                .iter()
                .map(|g| {
                    entry(
                        g.label.to_owned(),
                        Target::Component(g.key.to_owned()),
                        Some(settings.shows(g.key)),
                    )
                })
                .collect()
        }
        // The studio has no floating windows yet.
        Dynamic::Windows => Vec::new(),
    }
}

/// Menu entries for registry items, with no empty headings and no leading, trailing or
/// doubled separators; commands act on `target`.
fn entries(s: &Studio, items: &'static [Item], target: &Target) -> Vec<El<Studio>> {
    let l = &s.look;
    let mut all = Vec::new();
    lines(s, items, 0, target, &mut all);
    let entry = |line: Option<&Line>| matches!(line, Some(Line::Command(_) | Line::Entry(..)));
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
        .into_iter()
        .filter_map(|line| match line {
            Line::Separator => Some(menu_separator(l)),
            Line::Heading(title) => Some(menu_heading(l, title)),
            Line::Command(id) => command_item(s, id, target),
            Line::Entry(title, id, target, checked) => {
                let key = format!("cmd/{id}/{title}");
                let item = match checked {
                    Some(on) => menu_check(l, &key, &title, on),
                    None => menu_item(l, &key, None, &title, None, false),
                };
                Some(item.on_press(move |s: &mut Studio| {
                    s.local.menu = None;
                    s.local.context = None;
                    commands::run(s, id, &target);
                }))
            }
        })
        .collect()
}

/// One command as a menu item: its icon, title, shortcut and summary.
fn command_item(s: &Studio, id: &'static str, target: &Target) -> Option<El<Studio>> {
    let target = target.clone();
    let l = &s.look;
    let spec = commands::spec(id)?;
    let glyph = icons::named(spec.icon);
    let keys = commands::shortcut(spec.keys);
    // The OS file manager by its own name.
    let title = match id {
        "assets.reveal" if cfg!(target_os = "macos") => "Show in Finder",
        _ => spec.title,
    };
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
    // What the last request did, for a few seconds; else the keys worth knowing.
    let hint = match &s.local.note {
        Some((note, at)) if at.elapsed() < std::time::Duration::from_secs(8) => text(note.clone())
            .key("status/note")
            .text_size(11.0)
            .color(c.foreground.0),
        _ => text("W E R T tools · F frame · Alt+drag orbit · RMB+WASD fly · ⌘K commands")
            .text_size(11.0)
            .color(c.muted_foreground.0.with_alpha(0.8)),
    };
    let bar = row()
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
    keys: &'static str,
    run: Run,
}

enum Run {
    Command(&'static str),
    Select(String),
    /// Open this scene document.
    Scene(String),
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
    for spec in crate::catalog::all() {
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
                    keys: "",
                    run: Run::Scene(doc.to_string()),
                },
            ));
        }
    }
    if scenes_only {
        all.sort_by_key(|e| std::cmp::Reverse(e.0));
        return all.into_iter().take(14).map(|(_, e)| e).collect();
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
            Run::Scene(doc) => {
                s.act("editor.scene.open", serde_json::json!({"scene": doc}));
                s.resolve();
                s.act("editor.focus", serde_json::json!({"panel": "scene"}));
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
