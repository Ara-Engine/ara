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

const ROW: f32 = 22.0;
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
}

/// The icon for what an entity is, by its components.
fn glyph(record: &ara_doc::Record) -> stillkit::icons::Icon {
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
/// `query` (matches and their ancestors, all open).
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
    for list in children.values_mut() {
        list.sort();
    }
    let query = query.trim().to_lowercase();
    let keep: Option<BTreeSet<&str>> = (!query.is_empty()).then(|| {
        let mut keep = BTreeSet::new();
        for (id, e) in resolved.iter() {
            let name = e.record.get("name").and_then(Value::as_str).unwrap_or(id);
            if name.to_lowercase().contains(&query) {
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
        "Filter entities",
        &query,
        None,
        |f| f.on_input(|s: &mut Studio, t| s.local.hierarchy_query = t.into()),
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
            "Search looks at entity names.",
            None,
        )
    } else {
        let rows: std::sync::Arc<Vec<Row>> = std::sync::Arc::new(all);
        let selection: BTreeSet<String> = s.editor.state.selection.iter().cloned().collect();
        let built = rows.clone();
        let (hidden, locked) = (s.editor.state.hidden.clone(), s.editor.state.locked.clone());
        let rename = s.local.rename.clone();
        virtual_list("hierarchy/rows", rows.len(), ROW, move |range| {
            range
                .map(|i| {
                    row_el(
                        l,
                        (&built, i),
                        selection.contains(&built[i].id),
                        hidden.contains(&built[i].id),
                        locked.contains(&built[i].id),
                        rename.as_ref(),
                    )
                })
                .collect()
        })
        .grow(1.0)
        .pad_x(4.0)
    };
    col()
        .grow(1.0)
        .min_h(0.0)
        .child(row().pad(6.0).child(search.grow(1.0)))
        .child(body)
        .child(
            row()
                .h(24.0)
                .pad_x(10.0)
                .align(Align::Center)
                .child(
                    text(format!("{} entities", s.resolved.len()))
                        .text_size(11.0)
                        .color(c.muted_foreground.0),
                )
                .child(el().grow(1.0))
                .child(
                    text(if count != s.resolved.len() {
                        format!("{count} shown")
                    } else {
                        String::new()
                    })
                    .text_size(11.0)
                    .color(c.muted_foreground.0),
                ),
        )
}

fn row_el(
    l: &ara_kit::look::Look,
    (rows, at): (&std::sync::Arc<Vec<Row>>, usize),
    selected: bool,
    hidden: bool,
    locked: bool,
    rename: Option<&(String, String)>,
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
        None => text(r.name.clone())
            .text_size(12.0)
            .grow(1.0)
            .color(if hidden { muted } else { ink }),
    };
    let toggle = |glyph, label: &str, on: bool, method: &'static str, key: String| {
        let id = id.clone();
        el().key(key)
            .size(18.0)
            .align(Align::Center)
            .justify(Justify::Center)
            .radius(4.0)
            .color(muted)
            .cascade_ink()
            .label(label)
            .tooltip(label)
            .opacity(if on { 1.0 } else { 0.0 })
            .hovered(move |e| e.opacity(1.0).color(ink))
            .on_press(move |s: &mut Studio| s.act(method, json!({"entity": id, "enabled": !on})))
            .child(icon(glyph).size(13.0))
    };
    let pid = r.id.clone();
    row()
        .key(format!("row/{}", r.id))
        .h(ROW)
        .gap(4.0)
        .align(Align::Center)
        .radius(4.0)
        .fill(fill)
        .color(ink)
        .cascade_ink()
        .role(stillkit::Role::TreeItem)
        .label(r.name.clone())
        .selected(selected)
        .expanded(r.open)
        .on_pointer(move |s: &mut Studio, e, tx| {
            if e.phase != PointerPhase::Down || e.button != stillkit::Button::Left {
                return false;
            }
            if e.clicks == 2 {
                s.select(vec![pid.clone()], false);
                s.act("editor.frame", json!({}));
            } else if e.mods.shift || e.mods.ctrl {
                s.select(vec![pid.clone()], true);
            } else {
                s.select(vec![pid.clone()], false);
            }
            tx.focus(format!("row/{pid}").as_str());
            true
        })
        .on_context_menu({
            let id = r.id.clone();
            move |s: &mut Studio, x, y| {
                if !s.editor.state.selection.contains(&id) {
                    s.select(vec![id.clone()], false);
                }
                s.local.context = Some((
                    crate::shell::commands::menus::Context::Entity,
                    crate::shell::commands::Target::Entity(id.clone()),
                    (x, y),
                ));
            }
        })
        .focusable()
        .on_key({
            let rows = rows.clone();
            move |s: &mut Studio, key, tx| navigate(s, &rows, at, key.0, tx)
        })
        .child(el().w(4.0 + r.depth as f32 * INDENT))
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
        _ => match super::commands::for_key(key, tx.modifiers()) {
            Some(id) => super::commands::run(s, id, &crate::shell::commands::Target::None),
            None => return Handled::No,
        },
    }
    Handled::Yes
}
