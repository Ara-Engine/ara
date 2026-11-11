//! Hierarchy: the scene's entities as a virtual tree (any number of rows, only those near
//! sight built), 22 px rows with a disclosure, a type icon, the name, and visibility and
//! lock toggles on hover. Click selects (Shift or ⌘ toggles), double-click frames, F2
//! renames in place. The search keeps matches and their ancestors.
use std::collections::{BTreeMap, BTreeSet};

use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{
    Align, El, Handled, Justify, KeyCode, PointerPhase, col, el, icon, row, text, text_input,
    virtual_list,
};

use super::Studio;

const ROW: f32 = super::style::ROW;
const INDENT: f32 = 14.0;

/// One visible row.
struct Row {
    id: String,
    parent: Option<String>,
    name: String,
    depth: usize,
    children: bool,
    open: bool,
    glyph: stillkit::icons::Icon,
    /// Part of a prefab instance (its root or inside it): drawn in the prefab colour.
    prefab: bool,
}

/// Whether entity `id` (`record`) matches every term of a Hierarchy search: `t:Light`
/// matches a component type, `is:prefab` prefab instances, other words the name (Unity's
/// search syntax).
fn matches(terms: &[String], id: &str, name: &str, record: &ara_doc::Record) -> bool {
    let name = name.to_lowercase();
    terms.iter().all(|term| match term.strip_prefix("t:") {
        // `is:prefab`: instance roots and their members.
        None if term == "is:prefab" => id.contains('/') || record.get("prefab").is_some(),
        Some(kind) => record
            .keys()
            .filter(|k| ara_doc::is_component_key(k))
            .any(|k| k.to_lowercase().contains(kind)),
        None => name.contains(term.as_str()) || id.to_lowercase() == *term,
    })
}

/// The icon for what an entity is, by its components.
pub(super) fn glyph(record: &ara_doc::Record) -> stillkit::icons::Icon {
    let has = |c: &str| record.get(c).is_some();
    if has("Camera") {
        icons::VIDEO
    } else if record
        .get("Light")
        .and_then(|l| l.get("kind"))
        .and_then(Value::as_str)
        == Some("directional")
    {
        icons::SUN
    } else if has("Light") {
        icons::LIGHTBULB
    } else if has("Terrain") {
        icons::MOUNTAIN
    } else if has("VfxSystem") || has("Vfx") {
        icons::SPARKLES
    } else if has("AudioSource") {
        icons::VOLUME_2
    } else if has("MeshRenderer") {
        icons::BOX
    } else {
        icons::SQUARE_DASHED
    }
}

/// The visible rows: depth-first in sibling order, collapsed branches skipped, filtered by
/// `query` ([`matches`] and their ancestors, all open).
fn rows(s: &Studio, query: &str) -> Vec<Row> {
    let resolved = &s.resolved;
    let mut children: BTreeMap<Option<&str>, Vec<(&str, &str, &str)>> = BTreeMap::new();
    for (id, e) in resolved.iter() {
        let name = e.record.get("name").and_then(Value::as_str).unwrap_or(id);
        children.entry(e.parent.as_deref()).or_default().push((
            e.order.as_deref().unwrap_or(""),
            name,
            id,
        ));
    }
    // Authored order, or by name (Unity's alphanumeric sorting).
    let alphabetical = s
        .editor
        .state
        .settings
        .get("hierarchy.alphabetical")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    for list in children.values_mut() {
        match alphabetical {
            true => list.sort_by(|a, b| {
                a.1.to_lowercase()
                    .cmp(&b.1.to_lowercase())
                    .then(a.2.cmp(b.2))
            }),
            false => list.sort(),
        }
    }
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let keep: Option<BTreeSet<&str>> = (!terms.is_empty()).then(|| {
        let mut keep = BTreeSet::new();
        for (id, e) in resolved.iter() {
            let name = e.record.get("name").and_then(Value::as_str).unwrap_or(id);
            if matches(&terms, id, name, &e.record) {
                let mut at = Some(id.as_str());
                while let Some(a) = at.filter(|a| keep.insert(*a)) {
                    at = resolved.get(a).and_then(|e| e.parent.as_deref());
                }
            }
        }
        keep
    });
    let mut out = Vec::new();
    fn walk<'a>(
        parent: Option<&'a str>,
        depth: usize,
        s: &Studio,
        children: &BTreeMap<Option<&'a str>, Vec<(&'a str, &'a str, &'a str)>>,
        keep: &Option<BTreeSet<&str>>,
        out: &mut Vec<Row>,
    ) {
        let Some(list) = children.get(&parent) else {
            return;
        };
        for &(_, name, id) in list {
            if keep.as_ref().is_some_and(|k| !k.contains(id)) {
                continue;
            }
            let has_children = children.get(&Some(id)).is_some_and(|c| !c.is_empty());
            let open = keep.is_some() || !s.editor.state.collapsed.contains(id);
            let record = &s.resolved[id].record;
            out.push(Row {
                id: id.to_owned(),
                parent: parent.map(str::to_owned),
                name: name.to_owned(),
                depth,
                children: has_children,
                open,
                glyph: glyph(record),
                prefab: id.contains('/') || record.get("prefab").is_some(),
            });
            if has_children && open {
                walk(Some(id), depth + 1, s, children, keep, out);
            }
        }
    }
    walk(None, 0, s, &children, &keep, &mut out);
    out
}

pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let query = s.local.hierarchy_query.clone();
    let search = search(
        l,
        "hierarchy/search",
        // Short enough for a narrow panel; the syntax is in its tooltip and label.
        "Filter",
        &query,
        None,
        |f| {
            f.label("Filter by name, or by component: t:Light")
                .tooltip("Filter by name, or by component: t:Light")
                .on_input(|s: &mut Studio, t| s.local.hierarchy_query = t.into())
        },
        |s: &mut Studio| s.local.hierarchy_query.clear(),
    );
    let all = rows(s, &query);
    let count = all.len();
    let body = if s.resolved.is_empty() {
        empty(
            l,
            icons::LAYERS,
            "This scene is empty",
            "Create an entity from the Create menu, or drag a model in from Assets.",
            None,
        )
    } else if all.is_empty() {
        empty(
            l,
            icons::SEARCH,
            "No entities match",
            "The filter looks at names, or at components with t:Light.",
            None,
        )
    } else {
        let rows: std::sync::Arc<Vec<Row>> = std::sync::Arc::new(all);
        let selection: BTreeSet<String> = s.editor.state.selection.iter().cloned().collect();
        let built = rows.clone();
        let (hidden, locked) = (s.editor.state.hidden.clone(), s.editor.state.locked.clone());
        let rename = s.local.rename.clone();
        let drop = s.local.row_drag.as_ref().and_then(|d| d.target.clone());
        let hovered = s.local.hierarchy_hover.clone();
        // The search's first name word (filters such as `t:light` are not names), to mark
        // in each row's name as Unity does.
        let needle = s
            .local
            .hierarchy_query
            .split_whitespace()
            .find(|t| !t.contains(':'))
            .unwrap_or("")
            .to_lowercase();
        virtual_list("hierarchy/rows", rows.len(), ROW, move |range| {
            range
                .map(|i| {
                    row_el(
                        l,
                        (&built, i),
                        (
                            selection.contains(&built[i].id),
                            hovered.as_ref() == Some(&built[i].id),
                        ),
                        hidden.contains(&built[i].id),
                        locked.contains(&built[i].id),
                        (rename.as_ref(), &needle),
                        drop.as_ref()
                            .filter(|(t, _)| *t == built[i].id)
                            .map(|(_, p)| *p),
                    )
                })
                .collect()
        })
        .grow(1.0)
        .pad_x(4.0)
        // One Tab stop for the whole tree, on its selected row: the rows' own arrows move
        // through it (the WAI tree pattern), as Unity's and VS Code's trees do.
        .focus_group(stillkit::el::Orientation::Vertical)
        // Empty space: create at the root (rows have their own menu).
        .on_context_menu(|s: &mut Studio, x, y| {
            s.local.context = Some((
                crate::catalog::menus::Context::Hierarchy,
                crate::catalog::Target::None,
                (x, y),
            ));
        })
    };
    // The scene's root: its name and what it holds (Unity's scene header row).
    let workspace = s.editor.host.store().workspace();
    let scene = workspace
        .path_of(&s.editor.state.doc)
        .map(|p| workspace.reference_path(p))
        .unwrap_or("Scene")
        .rsplit('/')
        .next()
        .unwrap_or("Scene")
        .trim_end_matches(".scene")
        .to_owned();
    let shown = if count != s.resolved.len() {
        format!("{count} of {}", s.resolved.len())
    } else {
        s.resolved.len().to_string()
    };
    let storage = workspace.path_of(&s.editor.state.doc).map(str::to_owned);
    let root = row()
        .key("hierarchy/scene")
        .on_context_menu(move |s: &mut Studio, x, y| {
            let target = storage
                .clone()
                .map_or(crate::catalog::Target::None, crate::catalog::Target::Asset);
            s.local.context = Some((crate::catalog::menus::Context::Scene, target, (x, y)));
        })
        .h(22.0)
        .grow(1.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(super::style::tones(l).band)
        .child(
            icon(icons::CLAPPERBOARD)
                .size(13.0)
                .color(c.muted_foreground.0),
        )
        .child(
            text(scene)
                .text_size(12.0)
                .weight(600)
                .color(c.foreground.0)
                .grow(1.0),
        )
        .child(text(shown).text_size(11.0).color(c.muted_foreground.0));
    col()
        .grow(1.0)
        .min_h(0.0)
        .pad_y(4.0)
        .gap(3.0)
        .child(
            row()
                .pad_x(4.0)
                .gap(2.0)
                .child(search.h(22.0).grow(1.0))
                .child(filter_menu(s)),
        )
        .child(row().pad_x(4.0).child(root))
        .child(body)
}

/// The saved Hierarchy filters (`hierarchy.filters`: name to query), by name.
pub(super) fn saved_filters(s: &Studio) -> Vec<(String, String)> {
    s.editor
        .state
        .settings
        .get("hierarchy.filters")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, query)| Some((name.clone(), query.as_str()?.to_owned())))
        .collect()
}

/// The Hierarchy's filter menu (Unity's search filters): the type filters, the saved
/// ones, and Save This Filter for what is typed.
fn filter_menu(s: &Studio) -> El<Studio> {
    use crate::catalog::Target;
    let l = &s.look;
    const KEY: &str = "hierarchy/filters";
    let open = s.local.select.as_deref() == Some(KEY);
    let query = s.local.hierarchy_query.trim().to_owned();
    let mut b = super::scene_overlays::toggle(l, icons::FUNNEL, "Filters", !query.is_empty())
        .key(KEY)
        .expanded(open)
        .on_press(|s: &mut Studio| {
            s.local.select = match s.local.select.as_deref() == Some(KEY) {
                true => None,
                false => Some(KEY.into()),
            }
        });
    if !open {
        return b;
    }
    let run = |id: &'static str, target: Target| {
        move |s: &mut Studio| {
            s.local.select = None;
            super::commands::run(s, id, &target);
        }
    };
    let mut items = vec![menu_heading(l, "Show")];
    for (id, title, filter) in [
        ("hierarchy.filter.lights", "Lights", "t:light"),
        ("hierarchy.filter.cameras", "Cameras", "t:camera"),
        ("hierarchy.filter.meshes", "Meshes", "t:meshrenderer"),
        ("hierarchy.filter.audio", "Audio", "t:audio"),
        ("hierarchy.filter.effects", "Effects", "t:vfx"),
        ("hierarchy.filter.prefabs", "Prefab instances", "is:prefab"),
    ] {
        items.push(
            menu_check(l, &format!("{KEY}/{id}"), title, query == filter)
                .on_press(run(id, Target::None)),
        );
    }
    let alphabetical = s
        .editor
        .state
        .settings
        .get("hierarchy.alphabetical")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    items.push(menu_separator(l));
    items.push(menu_heading(l, "Sort"));
    items.push(
        menu_check(l, &format!("{KEY}/alphabetical"), "By name", alphabetical).on_press(
            move |s: &mut Studio| {
                s.local.select = None;
                s.act(
                    "editor.settings.set",
                    json!({"key": "hierarchy.alphabetical", "value": !alphabetical}),
                );
            },
        ),
    );
    let saved = saved_filters(s);
    if !saved.is_empty() {
        items.push(menu_separator(l));
        items.push(menu_heading(l, "Saved"));
    }
    for (name, filter) in saved {
        items.push(
            menu_check(l, &format!("{KEY}/saved/{name}"), &name, query == filter)
                .on_press(run("hierarchy.filter.apply", Target::Text(name.clone())))
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Remove this filter")
                        .size(18.0)
                        .key(format!("{KEY}/saved/{name}/remove"))
                        .on_press(run("hierarchy.filter.remove", Target::Text(name))),
                ),
        );
    }
    items.push(menu_separator(l));
    items.push(enabled(
        menu_item(
            l,
            &format!("{KEY}/save"),
            Some(icons::BOOKMARK),
            "Save This Filter",
            None,
            false,
        )
        .on_press(run("hierarchy.filter.save", Target::None)),
        !query.is_empty(),
    ));
    items.push(enabled(
        menu_item(
            l,
            &format!("{KEY}/clear"),
            Some(icons::X),
            "Clear Filter",
            None,
            false,
        )
        .on_press(run("hierarchy.filter.all", Target::None)),
        !query.is_empty(),
    ));
    b = b.overlay(
        stillkit::el::menu(menu_panel(l, items).min_w(220.0))
            .side(stillkit::el::Side::Bottom)
            .align(Align::End)
            .on_dismiss(|s: &mut Studio| s.local.select = None),
    );
    b
}

fn row_el(
    l: &ara_kit::look::Look,
    (rows, at): (&std::sync::Arc<Vec<Row>>, usize),
    (selected, hovered): (bool, bool),
    hidden: bool,
    locked: bool,
    (rename, needle): (Option<&(String, String)>, &str),
    drop: Option<ara_ui::drag_drop::Placement>,
) -> El<Studio> {
    let r = &rows[at];
    let c = &l.c;
    let (fill, ink, muted) = if selected {
        (
            c.selection_surface.0,
            c.selection_foreground.0,
            c.selection_muted_foreground.0,
        )
    } else {
        (
            stillkit::Rgba::TRANSPARENT,
            c.foreground.0,
            c.muted_foreground.0,
        )
    };
    let id = r.id.clone();
    let disclosure = if r.children {
        let id = id.clone();
        let open = r.open;
        el().size(16.0)
            .align(Align::Center)
            .justify(Justify::Center)
            .color(muted)
            .cascade_ink()
            .label(if open { "Collapse" } else { "Expand" })
            .expanded(open)
            .on_press(move |s: &mut Studio| {
                let set = &mut s.editor.state.collapsed;
                if !set.remove(&id) {
                    set.insert(id.clone());
                }
            })
            .child(
                icon(if r.open {
                    icons::CHEVRON_DOWN
                } else {
                    icons::CHEVRON_RIGHT
                })
                .size(12.0),
            )
    } else {
        el().size(16.0)
    };
    let name: El<Studio> = match rename.filter(|(rid, _)| *rid == r.id) {
        Some((_, draft)) => {
            let rid = r.id.clone();
            text_input("hierarchy/rename")
                .grow(1.0)
                .value(draft.as_str())
                .select_all_on_focus()
                .text_size(12.0)
                .color(c.foreground.0)
                .fill(c.input.0)
                .radius(3.0)
                .on_input(|s: &mut Studio, t| {
                    if let Some((_, d)) = s.local.rename.as_mut() {
                        *d = t.to_owned();
                    }
                })
                .on_submit(move |s: &mut Studio, t| {
                    let name = t.trim().to_owned();
                    s.local.rename = None;
                    if !name.is_empty() {
                        s.apply(vec![json!({"cmd":"property.set","args":{"entity":rid,"path":"name","value":name}})], "Rename entity");
                    }
                })
                .on_key(|s: &mut Studio, key, _| {
                    if key == KeyCode::ESCAPE {
                        s.local.rename = None;
                        Handled::Yes
                    } else {
                        Handled::No
                    }
                })
        }
        None => {
            let tint = if hidden {
                muted
            } else if r.prefab {
                // Prefab instances read in the prefab colour, as Unity's do.
                c.info_foreground.0
            } else {
                ink
            };
            // The search's match marked in the name (lowercase byte offsets match the
            // name's only when lowering keeps lengths; otherwise it stays plain).
            let at = (!needle.is_empty())
                .then(|| r.name.to_lowercase())
                .filter(|lower| lower.len() == r.name.len())
                .and_then(|lower| lower.find(needle));
            match at {
                Some(at) => {
                    // Three pieces side by side, each with its own ink (a row's ink
                    // cascades over rich runs).
                    let end = at + needle.len();
                    let piece = |s: &str| text(s.to_owned()).text_size(12.0);
                    row()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(piece(&r.name[..at]).color(tint))
                        .child(
                            piece(&r.name[at..end])
                                .weight(700)
                                .color(c.warning_foreground.0),
                        )
                        .child(piece(&r.name[end..]).color(tint))
                }
                None => text(r.name.clone()).text_size(12.0).grow(1.0).color(tint),
            }
        }
    };
    let toggle = |glyph, label: &str, on: bool, method: &'static str, key: String| {
        let id = id.clone();
        // Unset off the hovered row it isn't shown: a plain spacer, not a Tab stop on
        // every row of the tree.
        if !on && !hovered {
            return el().size(18.0);
        }
        el().key(key)
            .size(18.0)
            .align(Align::Center)
            .justify(Justify::Center)
            .radius(4.0)
            .color(muted)
            .cascade_ink()
            .label(label)
            .tooltip(label)
            // Shown while set, and on the row under the pointer; faint there when unset.
            .opacity(match (on, hovered) {
                (true, _) => 1.0,
                (false, true) => 0.55,
                (false, false) => 0.0,
            })
            .hovered(move |e| e.opacity(1.0).color(ink))
            .on_press(move |s: &mut Studio| s.act(method, json!({"entity": id, "enabled": !on})))
            .child(icon(glyph).size(13.0))
    };
    let pid = r.id.clone();
    // Where a drag would land: a line between rows, or this row as the new parent.
    use ara_ui::drag_drop::Placement;
    let indent = 10.0 + r.depth as f32 * INDENT;
    let (fill, line) = match drop {
        Some(Placement::Inside) => (c.primary.0.with_alpha(0.22), None),
        Some(Placement::Before) => (fill, Some(0.0)),
        Some(Placement::After) => (fill, Some(ROW - 2.0)),
        None => (fill, None),
    };
    let marker = line.map(|y| {
        el().absolute(indent, y)
            .h(2.0)
            .w(220.0)
            .radius(1.0)
            .fill(c.primary.0)
    });
    super::style::chosen(l, row(), selected)
        .key(format!("row/{}", r.id))
        .h(ROW)
        .gap(4.0)
        .align(Align::Center)
        .radius(4.0)
        .border(
            1.0,
            if drop == Some(Placement::Inside) {
                c.primary.0
            } else {
                stillkit::Rgba::TRANSPARENT
            },
        )
        .fill(fill)
        .children(marker)
        .color(ink)
        .cascade_ink()
        .role(stillkit::Role::TreeItem)
        .label(r.name.clone())
        .selected(selected)
        .expanded(r.open)
        .on_pointer({
            let rows = rows.clone();
            move |s: &mut Studio, e, tx| {
                if e.phase == PointerPhase::Hover {
                    if s.local.hierarchy_hover.as_ref() != Some(&pid) {
                        s.local.hierarchy_hover = Some(pid.clone());
                    }
                    return true;
                }
                if e.button != stillkit::Button::Left {
                    return false;
                }
                match e.phase {
                    PointerPhase::Down => {
                        if e.clicks == 2 {
                            s.select(vec![pid.clone()], false);
                            s.act("editor.frame", json!({}));
                        } else if e.mods.shift || e.mods.ctrl {
                            s.select(vec![pid.clone()], true);
                        } else if !s.editor.state.selection.contains(&pid) {
                            // A selected row keeps the selection, so it can drag them all.
                            s.select(vec![pid.clone()], false);
                        }
                        s.local.row_drag = Some(super::state::RowDrag {
                            source: pid.clone(),
                            at,
                            target: None,
                        });
                        tx.focus(format!("row/{pid}").as_str());
                    }
                    PointerPhase::Move => {
                        if e.travel().x.abs() + e.travel().y.abs() < 4.0 {
                            return true;
                        }
                        // Rows are uniform, so the pointer's offset from the pressed row
                        // names the row under it; its top quarter is Before, its bottom
                        // quarter After, the middle Inside.
                        let k = (e.local.y / ROW).floor();
                        let to = (at as f32 + k).clamp(0.0, rows.len().saturating_sub(1) as f32)
                            as usize;
                        let within = (e.local.y - k * ROW) / ROW;
                        use ara_ui::drag_drop::Placement;
                        // Sorted by name, the rows show no authored order to place
                        // between: a drop only reparents.
                        let sorted = s
                            .editor
                            .state
                            .settings
                            .get("hierarchy.alphabetical")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let place = if sorted {
                            Placement::Inside
                        } else if within < 0.25 {
                            Placement::Before
                        } else if within > 0.75 {
                            Placement::After
                        } else {
                            Placement::Inside
                        };
                        if let Some(drag) = s.local.row_drag.as_mut() {
                            drag.target = Some((rows[to].id.clone(), place));
                        }
                    }
                    PointerPhase::Up => {
                        let drag = s.local.row_drag.take();
                        match drag.and_then(|d| d.target.map(|t| (d.source, t))) {
                            Some((source, (target, place))) => {
                                drop_rows(s, &source, &target, place)
                            }
                            // A plain click on a row of a multi-selection selects just it.
                            None if !(e.mods.shift || e.mods.ctrl)
                                && s.editor.state.selection.len() > 1 =>
                            {
                                s.select(vec![pid.clone()], false)
                            }
                            None => {}
                        }
                    }
                    PointerPhase::Cancel => s.local.row_drag = None,
                    PointerPhase::Hover => return false,
                }
                true
            }
        })
        .on_context_menu({
            let id = r.id.clone();
            move |s: &mut Studio, x, y| {
                if !s.editor.state.selection.contains(&id) {
                    s.select(vec![id.clone()], false);
                }
                s.local.context = Some((
                    crate::catalog::menus::Context::Entity,
                    crate::catalog::Target::Entity(id.clone()),
                    (x, y),
                ));
            }
        })
        .focusable()
        .on_key({
            let rows = rows.clone();
            move |s: &mut Studio, key, tx| navigate(s, &rows, at, key.0, tx)
        })
        .child(el().w(10.0 + r.depth as f32 * INDENT))
        .child(disclosure)
        .child(
            icon(r.glyph)
                .size(14.0)
                .color(if selected { ink } else { muted }),
        )
        .child(name)
        .child(toggle(
            if hidden { icons::EYE_OFF } else { icons::EYE },
            if hidden { "Show" } else { "Hide" },
            hidden,
            "editor.visibility",
            format!("row/{}/visibility", r.id),
        ))
        .child(toggle(
            if locked {
                icons::LOCK
            } else {
                icons::LOCK_OPEN
            },
            if locked { "Unlock" } else { "Lock" },
            locked,
            "editor.lock",
            format!("row/{}/lock", r.id),
        ))
        .child(el().w(2.0))
}

/// The tree's keyboard (WAI tree pattern): Up and Down move through visible rows, Right
/// opens a branch or steps into it, Left closes it or steps to the parent, Home and End
/// jump; Shift extends the selection. F2 or Return renames; other keys are the editor's.
fn navigate(
    s: &mut Studio,
    rows: &[Row],
    at: usize,
    key: u16,
    tx: &mut stillkit::el::Tx,
) -> Handled {
    let r = &rows[at];
    let extend = tx.modifiers().shift;
    let go = |s: &mut Studio, to: usize, tx: &mut stillkit::el::Tx| {
        let id = rows[to].id.clone();
        s.select(
            vec![id.clone()],
            extend && !s.editor.state.selection.contains(&id),
        );
        tx.focus(format!("row/{id}").as_str());
    };
    match key {
        0x28 if at + 1 < rows.len() => go(s, at + 1, tx),
        0x26 if at > 0 => go(s, at - 1, tx),
        0x24 => go(s, 0, tx),
        0x23 => go(s, rows.len() - 1, tx),
        0x27 if r.children && !r.open => {
            s.editor.state.collapsed.remove(&r.id);
        }
        0x27 if r.children => go(s, at + 1, tx),
        0x25 if r.children && r.open => {
            s.editor.state.collapsed.insert(r.id.clone());
        }
        0x25 => {
            if let Some(parent) = r
                .parent
                .as_deref()
                .and_then(|p| rows.iter().position(|x| x.id == p))
            {
                go(s, parent, tx);
            }
        }
        0x71 | 0x0D => {
            s.local.rename = Some((r.id.clone(), r.name.clone()));
            tx.focus("hierarchy/rename");
        }
        _ => match super::commands::for_key(s, key, tx.modifiers()) {
            Some(id) => super::commands::run(s, &id, &crate::catalog::Target::None),
            None => return Handled::No,
        },
    }
    Handled::Yes
}

/// Drops the dragged rows (the selection, when the pressed row is in it) relative to
/// `target`: before or after it among its siblings, or inside it. World transforms are kept.
fn drop_rows(s: &mut Studio, source: &str, target: &str, place: ara_ui::drag_drop::Placement) {
    if source == target {
        return;
    }
    let plan = crate::hierarchy_actions::reparent::commands(
        &s.resolved,
        &s.editor.state.selection,
        source,
        Some(target),
        place,
        &s.editor.state.locked,
    );
    match plan {
        Ok(commands) if !commands.is_empty() => s.apply(commands, "Move in hierarchy"),
        Ok(_) => {}
        Err(reason) => s.editor.report_failure(&std::io::Error::other(reason)),
    }
}

/// Forgets the hovered Hierarchy row once the pointer has left it (rows learn they are
/// hovered, never that the pointer left); the host runs it every pass.
pub(super) fn unhover(rt: &mut stillkit::el::Runtime<Studio>) {
    let Some(id) = rt.state().local.hierarchy_hover.clone() else {
        return;
    };
    let still = rt
        .presented(&format!("row/{id}"))
        .is_some_and(|shown| shown.state == stillkit::interact::State::Hover);
    if !still {
        rt.update(|s| s.local.hierarchy_hover = None);
    }
}
