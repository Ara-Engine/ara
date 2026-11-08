//! The command palette (⌘K, or the title bar's launcher): one field that finds projects,
//! templates, pages and actions by fuzzy match, ranked, with the keyboard doing everything:
//! type, Up/Down, Enter runs, Esc closes.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, Handled, KeyCode, col, dialog, el, icon, row, text, text_input};
use stillkit::icons::Icon;
use stillkit::{Role, ShadowLevel};

use super::HubApp;
use crate::model::{Intent, State, View};

/// The palette's own state (never the model's).
#[derive(Debug, Default)]
pub struct Palette {
    /// Whether it is showing.
    pub open: bool,
    /// What was typed.
    pub query: String,
    /// The highlighted result.
    pub active: usize,
}

impl Palette {
    /// Shows it empty.
    pub fn open(&mut self) {
        *self = Self {
            open: true,
            ..Self::default()
        };
    }
}

/// One thing the palette can do.
struct Entry {
    group: &'static str,
    icon: Icon,
    title: String,
    detail: String,
    intent: Intent,
}

/// Everything, before matching.
fn entries(s: &HubApp) -> Vec<Entry> {
    let hub = &s.hub;
    let mut out = Vec::new();
    for p in &hub.projects {
        let state = hub.state(&p.id);
        let (detail, intent) = match state {
            State::Editor => ("Open in editor".to_owned(), Intent::Open(p.id.clone())),
            _ => (
                crate::model::text::path(&p.path),
                Intent::Open(p.id.clone()),
            ),
        };
        out.push(Entry {
            group: "Projects",
            icon: icons::BOX,
            title: p.name.clone(),
            detail,
            intent,
        });
    }
    for t in &hub.templates {
        out.push(Entry {
            group: "New from template",
            icon: icons::FILE_PLUS,
            title: format!("New {} project", t.name),
            detail: super::create::description(t),
            intent: Intent::Template(t.id.clone()),
        });
    }
    for v in [
        View::Projects,
        View::Installs,
        View::Agents,
        View::Learn,
        View::Settings,
    ] {
        out.push(Entry {
            group: "Go to",
            icon: icons::ARROW_RIGHT,
            title: v.title().into(),
            detail: String::new(),
            intent: Intent::Navigate(v),
        });
    }
    let actions = [
        (icons::PLUS, "New project", Intent::Navigate(View::Create)),
        (
            icons::FOLDER_OPEN,
            "Add existing project…",
            Intent::AddExisting,
        ),
        (icons::HARD_DRIVE, "Add engine build…", Intent::AddEngine),
        (icons::REFRESH_CW, "Check for updates", Intent::CheckUpdates),
        (
            if hub.prefs.light {
                icons::MOON
            } else {
                icons::SUN
            },
            if hub.prefs.light {
                "Switch to dark theme"
            } else {
                "Switch to light theme"
            },
            Intent::ToggleTheme,
        ),
    ];
    for (icon, title, intent) in actions {
        out.push(Entry {
            group: "Actions",
            icon,
            title: title.into(),
            detail: String::new(),
            intent,
        });
    }
    out
}

/// A fuzzy score: every query character in order, rewarding word starts and runs; `None`
/// when they don't all appear.
fn score(query: &str, title: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let title: Vec<char> = title.to_lowercase().chars().collect();
    let mut at = 0;
    let mut total = 0;
    let mut run = 0;
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = title[at..].iter().position(|&c| c == q)? + at;
        let word_start = found == 0 || !title[found - 1].is_alphanumeric();
        run = if found == at && at > 0 { run + 1 } else { 0 };
        total += 10 + run * 6 + if word_start { 12 } else { 0 } - (found - at).min(10) as i32;
        at = found + 1;
    }
    Some(total - title.len() as i32 / 8)
}

/// The best matches, at most 12: groups stay together, the group with the best match first,
/// and within a group the best matches first (the original order when nothing is typed).
fn results(s: &HubApp) -> Vec<Entry> {
    let query = s.palette.query.trim();
    let mut scored: Vec<(i32, usize, Entry)> = entries(s)
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| score(query, &e.title).map(|sc| (sc, i, e)))
        .collect();
    if !query.is_empty() {
        let best = |group: &str, all: &[(i32, usize, Entry)]| {
            all.iter()
                .filter(|e| e.2.group == group)
                .map(|e| e.0)
                .max()
                .unwrap_or(i32::MIN)
        };
        let bests: Vec<i32> = scored.iter().map(|e| best(e.2.group, &scored)).collect();
        let mut keyed: Vec<_> = scored.into_iter().zip(bests).collect();
        keyed.sort_by(|(a, ba), (b, bb)| {
            bb.cmp(ba)
                .then(a.2.group.cmp(b.2.group))
                .then(b.0.cmp(&a.0))
                .then(a.1.cmp(&b.1))
        });
        scored = keyed.into_iter().map(|(e, _)| e).collect();
    }
    scored.into_iter().take(12).map(|(_, _, e)| e).collect()
}

/// Runs the active result and closes.
fn run(s: &mut HubApp) {
    if let Some(entry) = results(s).into_iter().nth(s.palette.active) {
        s.palette.open = false;
        s.intent(entry.intent);
    }
}

pub(super) fn layer(s: &HubApp, root: El<HubApp>) -> El<HubApp> {
    if !s.palette.open {
        return root;
    }
    let l = &s.look;
    let c = &l.c;
    let found = results(s);
    let active = s.palette.active.min(found.len().saturating_sub(1));
    let field = row()
        .h(48.0)
        .pad_x(14.0)
        .gap(10.0)
        .align(Align::Center)
        .child(icon(icons::SEARCH).size(16.0).color(c.muted_foreground.0))
        .child(
            text_input("palette/query")
                .focus_ring(false)
                .grow(1.0)
                .placeholder("Search projects, templates and actions")
                .value(s.palette.query.as_str())
                .text_size(14.0)
                .color(c.foreground.0)
                .on_input(|s: &mut HubApp, t| {
                    s.palette.query = t.into();
                    s.palette.active = 0;
                })
                .on_submit(|s: &mut HubApp, _| run(s))
                .on_key(move |s: &mut HubApp, key, _| {
                    let n = results(s).len().max(1);
                    match key {
                        KeyCode::DOWN => s.palette.active = (s.palette.active + 1) % n,
                        KeyCode::UP => s.palette.active = (s.palette.active + n - 1) % n,
                        KeyCode::ESCAPE => s.palette.open = false,
                        _ => return Handled::No,
                    }
                    Handled::Yes
                }),
        )
        .child(kbd(l, "esc"));
    let mut list = col()
        .pad(6.0)
        .gap(1.0)
        .max_h(420.0)
        .scroll(stillkit::el::Axis::Vertical);
    let mut group = "";
    for (i, e) in found.iter().enumerate() {
        if e.group != group {
            group = e.group;
            list = list.child(
                row().h(26.0).pad_x(10.0).align(Align::End).child(
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
            .h(36.0)
            .pad_x(10.0)
            .gap(10.0)
            .align(Align::Center)
            .radius(l.radius.sm)
            .role(Role::ListBoxOption)
            .selected(on)
            .label(e.title.clone())
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
            .on_press(move |s: &mut HubApp| {
                s.palette.active = i;
                run(s);
            })
            .child(icon(e.icon).size(15.0))
            .child(text(e.title.clone()).text_size(12.5).weight(520));
        if !e.detail.is_empty() {
            let ink = if on {
                c.selection_muted_foreground.0
            } else {
                c.muted_foreground.0
            };
            item = item.child(
                text(e.detail.clone())
                    .text_size(11.5)
                    .color(ink)
                    .grow(1.0)
                    .text_align(stillkit::el::TextAlign::End),
            );
        }
        list = list.child(item);
    }
    if found.is_empty() {
        list = list.child(
            row()
                .h(64.0)
                .justify(stillkit::el::Justify::Center)
                .align(Align::Center)
                .child(
                    text(format!("Nothing matches “{}”", s.palette.query.trim()))
                        .text_size(12.0)
                        .color(c.muted_foreground.0),
                ),
        );
    }
    let footer = row()
        .h(34.0)
        .pad_x(14.0)
        .gap(14.0)
        .align(Align::Center)
        .child(
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(kbd(l, "↑↓"))
                .child(text("move").text_size(11.0).color(c.muted_foreground.0)),
        )
        .child(
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(kbd(l, "↩"))
                .child(text("run").text_size(11.0).color(c.muted_foreground.0)),
        )
        .child(el().grow(1.0));
    let panel = col()
        .key("palette")
        .w(620.0)
        .radius(l.radius.base + 2.0)
        .fill(c.popover.0)
        .border(1.0, c.separator_strong.0)
        .shadow_level(ShadowLevel::Dialog)
        .role(Role::Dialog)
        .label("Command palette")
        .child(field)
        .child(divider(l))
        .child(list)
        .child(divider(l))
        .child(footer);
    // Placed high, not centred: the eye stays near the title bar's launcher.
    let positioned = col()
        .w_full()
        .h_full()
        .align(Align::Center)
        .child(el().h(72.0))
        .child(panel);
    root.overlay(dialog(positioned).on_dismiss(|s: &mut HubApp| s.palette.open = false))
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn fuzzy_scores_prefer_word_starts_and_runs() {
        assert!(score("nm", "Night Market").is_some());
        assert!(score("xyz", "Night Market").is_none());
        assert!(score("night", "Night Market") > score("nght", "Night Market"));
        assert!(
            score("nm", "Night Market") > score("nm", "Unnamed"),
            "word starts win"
        );
    }
}
