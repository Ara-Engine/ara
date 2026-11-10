//! Add Component, as in Unity: a compact picker under the Inspector's button. Components are
//! grouped by their schema module (Rendering, Physics, Audio...) into categories you open;
//! typing searches every one of them; the components you added last come first. Arrows move,
//! Enter (or Right) opens a category or adds a component, Left or Backspace goes back, and
//! the highlighted component's description shows at the bottom.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::json;
use stillkit::el::{Align, Axis, El, Handled, Side, col, el, icon, popover, row, text};
use stillkit::icons::Icon;

use super::Studio;

/// The open picker: what is typed, the highlighted row, and the category it is in.
#[derive(Clone, Debug, Default)]
pub struct Picker {
    pub query: String,
    pub active: usize,
    /// The schema module it shows, or None for the top level.
    pub category: Option<String>,
}

/// One row of the picker.
#[derive(Clone)]
enum Row {
    /// A category: its module, how many components it offers.
    Category(&'static str, usize),
    /// A component: its name, module and first doc line.
    Component(String, String, String),
}

/// A schema module's title and icon (`TypeDesc::module`); unknown ones read as themselves.
fn category(module: &str) -> (&str, Icon) {
    match module {
        "core" => ("Core", icons::MOVE_3D),
        "render" | "geo" | "material" => ("Rendering", icons::SUN),
        "physics" => ("Physics", icons::ATOM),
        "physics2d" => ("Physics 2D", icons::ATOM),
        "audio" => ("Audio", icons::VOLUME_2),
        "animation" => ("Animation", icons::PERSON_STANDING),
        "navigation" => ("Navigation", icons::ROUTE),
        "ai" => ("AI", icons::BRAIN),
        "vfx" => ("Effects", icons::SPARKLES),
        "twoD" => ("2D", icons::IMAGE),
        "terrain" => ("Terrain", icons::MOUNTAIN),
        "foliage" => ("Foliage", icons::TREES),
        "world" => ("World", icons::EARTH),
        "gameplay" => ("Gameplay", icons::GAMEPAD_2),
        "ui" => ("UI", icons::APP_WINDOW),
        "save" => ("Saving", icons::SAVE),
        "l10n" => ("Localization", icons::GLOBE),
        other => (other, icons::COMPONENT),
    }
}

/// Modules that share a category ("geo" and "material" read as Rendering) fold into one.
fn folded(module: &str) -> &str {
    match module {
        "geo" | "material" => "render",
        other => other,
    }
}

/// Components the whole selection lacks: name, folded module, first doc line.
fn addable(s: &Studio) -> Vec<(String, String, String)> {
    let selection = &s.editor.state.selection;
    let on_all = |name: &str| {
        selection.iter().all(|e| {
            s.resolved
                .get(e)
                .is_some_and(|r| r.record.contains_key(name))
        })
    };
    s.editor
        .host
        .store()
        .schema()
        .components()
        .filter(|t| !on_all(&t.name))
        .map(|t| {
            (
                t.name.clone(),
                folded(&t.module).to_owned(),
                t.doc.lines().next().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

/// The rows `p` shows: matches across everything while typing, a category's components
/// inside one, else the recent components and then every category.
fn rows(s: &Studio, p: &Picker) -> Vec<Row> {
    let all = addable(s);
    let query = p.query.trim();
    if !query.is_empty() {
        let mut found: Vec<(i32, Row)> = all
            .into_iter()
            .filter_map(|(name, module, doc)| {
                let score = super::chrome::score(query, &name)
                    .or_else(|| super::chrome::score(query, &super::inspector::spaced(&name)))?;
                Some((score, Row::Component(name, module, doc)))
            })
            .collect();
        found.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        return found.into_iter().map(|(_, row)| row).collect();
    }
    if let Some(module) = &p.category {
        let mut inside: Vec<Row> = all
            .into_iter()
            .filter(|(_, m, _)| m == module)
            .map(|(n, m, d)| Row::Component(n, m, d))
            .collect();
        inside.sort_by(|a, b| name(a).cmp(name(b)));
        return inside;
    }
    let mut out: Vec<Row> = s
        .local
        .recent_components
        .iter()
        .filter_map(|r| all.iter().find(|(n, _, _)| n == r))
        .map(|(n, m, d)| Row::Component(n.clone(), m.clone(), d.clone()))
        .collect();
    let mut modules: Vec<(&'static str, usize)> = Vec::new();
    for (_, module, _) in &all {
        match modules.iter_mut().find(|(m, _)| *m == module) {
            Some((_, n)) => *n += 1,
            None => modules.push((leak(module), 1)),
        }
    }
    modules.sort_by(|a, b| category(a.0).0.cmp(category(b.0).0));
    out.extend(modules.into_iter().map(|(m, n)| Row::Category(m, n)));
    out
}

/// A module name with a static lifetime: modules are a handful of schema constants, interned
/// once each.
fn leak(module: &str) -> &'static str {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(found) = names.get(module) {
        return found;
    }
    let made: &'static str = Box::leak(module.to_owned().into_boxed_str());
    names.insert(made);
    made
}

fn name(row: &Row) -> &str {
    match row {
        Row::Category(m, _) => m,
        Row::Component(n, _, _) => n,
    }
}

/// The Inspector's Add Component button and, while open, its picker.
pub(super) fn button_and_picker(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let open = s.local.add_component.is_some();
    let mut b = button(
        l,
        Tone::Secondary,
        Size::Sm,
        Some(icons::PLUS),
        "Add Component",
    )
    .key("inspector/add")
    .w(220.0)
    .expanded(open)
    .on_press(|s: &mut Studio| {
        s.local.add_component = match s.local.add_component {
            Some(_) => None,
            None => Some(Picker::default()),
        };
        s.local.focus = Some("inspector/add/search");
    });
    if let Some(p) = &s.local.add_component {
        b = b.overlay(
            popover(picker(s, p))
                .side(Side::Bottom)
                .align(Align::Center)
                .offset(4.0)
                .on_dismiss(|s: &mut Studio| s.local.add_component = None),
        );
    }
    // Centred under the components, as Unity places it.
    row()
        .pad_y(8.0)
        .justify(stillkit::el::Justify::Center)
        .child(b)
}

fn picker(s: &Studio, p: &Picker) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let rows = rows(s, p);
    let active = p.active.min(rows.len().saturating_sub(1));
    let searching = !p.query.trim().is_empty();
    let search = row().pad(6.0).child(
        field(
            l,
            Size::Sm,
            "inspector/add/search",
            Some(icons::SEARCH),
            "Search components",
            &p.query,
            |f| {
                f.on_input(|s: &mut Studio, t| {
                    if let Some(p) = s.local.add_component.as_mut() {
                        p.query = t.to_owned();
                        p.active = 0;
                    }
                })
                .on_submit(|s: &mut Studio, _| choose(s))
            },
        )
        .grow(1.0),
    );
    let mut panel = super::style::float(l, col())
        .key("inspector/add/panel")
        .w(300.0)
        .radius(l.radius.sm + 2.0)
        .on_key(|s: &mut Studio, key, _| keys(s, key.0))
        .child(search);
    // Inside a category: its name, and the way back.
    if let (Some(module), false) = (&p.category, searching) {
        let (title, glyph) = category(module);
        let hover = c.hover_surface.0;
        panel = panel.child(
            row().pad_x(6.0).child(
                row()
                    .key("inspector/add/back")
                    .h(26.0)
                    .grow(1.0)
                    .pad_x(6.0)
                    .gap(8.0)
                    .align(Align::Center)
                    .radius(super::style::RADIUS)
                    .color(c.foreground.0)
                    .cascade_ink()
                    .label("Back to categories")
                    .hovered(move |e| e.fill(hover))
                    .on_press(back)
                    .child(icon(icons::CHEVRON_LEFT).size(13.0))
                    .child(icon(glyph).size(13.0))
                    .child(text(title).text_size(12.0).weight(600)),
            ),
        );
    }
    let mut list = col()
        .key("inspector/add/list")
        .max_h(300.0)
        .pad_x(6.0)
        .pad_y(2.0)
        .gap(1.0)
        .scroll(Axis::Vertical);
    if rows.is_empty() {
        list = list.child(
            text(if searching {
                "No components match"
            } else {
                "Every component is already on the selection"
            })
            .text_size(12.0)
            .color(c.muted_foreground.0)
            .pad(8.0),
        );
    }
    let recent = !searching && p.category.is_none() && !s.local.recent_components.is_empty();
    for (i, r) in rows.iter().enumerate() {
        if recent && i == 0 {
            list = list.child(heading(l, "Recent"));
        }
        if recent
            && matches!(r, Row::Category(..))
            && matches!(rows.get(i.wrapping_sub(1)), Some(Row::Component(..)))
        {
            list = list.child(heading(l, "Categories"));
        }
        list = list.child(row_el(l, r, i, i == active, searching));
    }
    let about = match rows.get(active) {
        Some(Row::Component(_, _, doc)) => doc.clone(),
        Some(Row::Category(m, n)) => format!(
            "{n} component{} in {}",
            if *n == 1 { "" } else { "s" },
            category(m).0
        ),
        None => String::new(),
    };
    panel.child(list).child(
        row()
            .min_h(44.0)
            .pad_x(12.0)
            .pad_y(8.0)
            .fill(super::style::tones(l).strip)
            .child(
                text(about)
                    .text_size(11.5)
                    .color(c.muted_foreground.0)
                    .wrap(3)
                    .grow(1.0),
            ),
    )
}

/// A small label over a group of rows.
fn heading(l: &ara_kit::look::Look, title: &str) -> El<Studio> {
    row().h(22.0).pad_x(8.0).align(Align::End).child(
        text(title)
            .text_size(10.5)
            .weight(600)
            .color(l.c.muted_foreground.0),
    )
}

fn row_el(
    l: &ara_kit::look::Look,
    r: &Row,
    at: usize,
    active: bool,
    searching: bool,
) -> El<Studio> {
    let c = &l.c;
    let hover = c.hover_surface.0;
    let base = super::style::chosen(l, row(), active)
        .h(26.0)
        .pad_x(8.0)
        .gap(9.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .selected(active)
        .hovered(move |e| if active { e } else { e.fill(hover) })
        .on_pointer(move |s: &mut Studio, e, _| match e.phase {
            stillkit::el::PointerPhase::Hover => {
                if let Some(p) = s.local.add_component.as_mut()
                    && p.active != at
                {
                    p.active = at;
                }
                false
            }
            stillkit::el::PointerPhase::Up if e.button == stillkit::Button::Left => {
                if let Some(p) = s.local.add_component.as_mut() {
                    p.active = at;
                }
                choose(s);
                true
            }
            stillkit::el::PointerPhase::Down => true,
            _ => false,
        });
    match r {
        Row::Category(module, n) => {
            let (title, glyph) = category(module);
            base.key(format!("inspector/add/cat/{module}"))
                .label(title)
                .color(c.foreground.0)
                .cascade_ink()
                .child(icon(glyph).size(14.0).color(c.muted_foreground.0))
                .child(text(title).text_size(12.5).weight(500).grow(1.0))
                .child(
                    text(n.to_string())
                        .text_size(10.5)
                        .color(c.muted_foreground.0.with_alpha(0.7)),
                )
                .child(
                    icon(icons::CHEVRON_RIGHT)
                        .size(12.0)
                        .color(c.muted_foreground.0),
                )
        }
        Row::Component(name, module, _) => base
            .key(format!("inspector/add/{name}"))
            .label(name.clone())
            .child(
                icon(super::inspector::component_icon(name))
                    .size(14.0)
                    .color(c.info_foreground.0),
            )
            .child(
                text(super::inspector::spaced(name))
                    .text_size(12.5)
                    .weight(500)
                    .color(c.foreground.0)
                    .grow(1.0),
            )
            .children(searching.then(|| {
                text(category(module).0)
                    .text_size(11.0)
                    .color(c.muted_foreground.0)
            }))
            .child(el().w(0.0)),
    }
}

/// Enter or a click: open the highlighted category, or add the highlighted component.
fn choose(s: &mut Studio) {
    let Some(p) = s.local.add_component.clone() else {
        return;
    };
    let rows = rows(s, &p);
    match rows.get(p.active.min(rows.len().saturating_sub(1))) {
        Some(Row::Category(module, _)) => {
            s.local.add_component = Some(Picker {
                query: String::new(),
                active: 0,
                category: Some((*module).to_owned()),
            });
            s.local.focus = Some("inspector/add/search");
        }
        Some(Row::Component(name, _, _)) => {
            let name = name.clone();
            add(s, &name);
        }
        None => {}
    }
}

/// Back out of a category to the top level.
fn back(s: &mut Studio) {
    if let Some(p) = s.local.add_component.as_mut() {
        p.category = None;
        p.active = 0;
    }
    s.local.focus = Some("inspector/add/search");
}

fn keys(s: &mut Studio, key: u16) -> Handled {
    let Some(p) = s.local.add_component.clone() else {
        return Handled::No;
    };
    let n = rows(s, &p).len();
    let typing = !p.query.is_empty();
    match key {
        0x28 if n > 0 => set_active(s, (p.active + 1).min(n - 1)),
        0x26 => set_active(s, p.active.saturating_sub(1)),
        0x27 if !typing => choose(s),
        0x25 | 0x08 if !typing && p.category.is_some() => back(s),
        0x1B => s.local.add_component = None,
        _ => return Handled::No,
    }
    Handled::Yes
}

fn set_active(s: &mut Studio, at: usize) {
    if let Some(p) = s.local.add_component.as_mut() {
        p.active = at;
    }
}

/// Adds `name` to every selected entity (one undo step) and remembers it as recent.
fn add(s: &mut Studio, name: &str) {
    s.local.add_component = None;
    let recent = &mut s.local.recent_components;
    recent.retain(|r| r != name);
    recent.insert(0, name.to_owned());
    recent.truncate(5);
    let commands = s
        .editor
        .state
        .selection
        .iter()
        .map(|e| json!({"cmd": "component.add", "args": {"entity": e, "component": name}}))
        .collect();
    s.apply(commands, &format!("Add {name}"));
}
