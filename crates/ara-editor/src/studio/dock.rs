//! The dock: the editor's persisted layout (`EditorState.layout`, the classic editor's
//! format, so layouts carry over) drawn as nested splits and tabbed areas. Splitters drag
//! with the pointer; the host reports split rects back (`Local::rects`) so a drag knows the
//! split's extent. Tabs switch through `editor.layout.show`, so agents see the same layout.
use ara_kit::icons;
use ara_kit::ui::*;
use ara_ui::docking::{Axis as DockAxis, DockNode};
use serde_json::json;
use stillkit::Cursor;
use stillkit::el::{Align, El, Justify, PointerPhase, col, el, icon, row, text};

use super::state::SplitDrag;
use super::{Studio, game, hierarchy, inspector, panels, scene, style};

/// Keys of every split container, for the host's rect feedback.
pub(super) fn rect_keys(root: &DockNode) -> Vec<String> {
    fn walk(node: &DockNode, path: &mut Vec<u8>, out: &mut Vec<String>) {
        if let DockNode::Split { first, second, .. } = node {
            out.push(split_key(path));
            path.push(0);
            walk(first, path, out);
            path.pop();
            path.push(1);
            walk(second, path, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(root, &mut Vec::new(), &mut out);
    out
}

fn split_key(path: &[u8]) -> String {
    format!(
        "split/{}",
        path.iter()
            .map(|b| char::from(b'0' + b))
            .collect::<String>()
    )
}

/// Keys of every area and tab, for the host to report while a tab is dragged.
pub(super) fn drag_keys(root: &DockNode) -> Vec<String> {
    fn walk(node: &DockNode, out: &mut Vec<String>) {
        match node {
            DockNode::Area { id, tabs, .. } => {
                out.push(format!("area/{id}"));
                out.extend(tabs.iter().map(|t| format!("tab/{t}")));
            }
            DockNode::Split { first, second, .. } => {
                walk(first, out);
                walk(second, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// Where a dragged tab lands: among an area's tabs (`index`), or beside it in a new area
/// split off one edge (Unity's and Unreal's dock targets).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Drop {
    /// Into `area`'s tabs at `index`.
    Tab { area: String, index: usize },
    /// A new area beside `area`: above or below it (`vertical`), else left or right;
    /// `before` is above or left.
    Split {
        area: String,
        vertical: bool,
        before: bool,
    },
}

/// The drop under window point `at` for `panel`, and the rect it would fill: over a tab
/// strip, among its tabs; over an area's outer quarter, a split on that edge; elsewhere
/// in an area, its tabs' end.
pub(super) fn drop_at(s: &Studio, panel: &str, at: (f32, f32)) -> Option<(Drop, stillkit::Rect)> {
    fn areas(node: &DockNode, out: &mut Vec<(String, Vec<String>)>) {
        match node {
            DockNode::Area { id, tabs, .. } => out.push((id.clone(), tabs.clone())),
            DockNode::Split { first, second, .. } => {
                areas(first, out);
                areas(second, out);
            }
        }
    }
    let mut all = Vec::new();
    areas(&s.editor.state.layout, &mut all);
    let inside =
        |r: &stillkit::Rect| at.0 >= r.left && at.0 <= r.right && at.1 >= r.top && at.1 <= r.bottom;
    let (id, tabs, r) = all.into_iter().find_map(|(id, tabs)| {
        let r = *s.local.rects.get(&format!("area/{id}"))?;
        inside(&r).then_some((id, tabs, r))
    })?;
    let rect = |l: f32, t: f32, w: f32, h: f32| stillkit::Rect::new(l, t, w, h);
    // The tab strip: before the first tab whose middle is right of the pointer.
    if at.1 <= r.top + style::STRIP {
        let index = tabs
            .iter()
            .filter(|t| t.as_str() != panel)
            .position(|t| {
                s.local
                    .rects
                    .get(&format!("tab/{t}"))
                    .is_some_and(|tr| at.0 < tr.left + tr.width() / 2.0)
            })
            .unwrap_or(tabs.len());
        return Some((
            Drop::Tab { area: id, index },
            rect(r.left, r.top, r.width(), r.height()),
        ));
    }
    let (fx, fy) = (
        (at.0 - r.left) / r.width().max(1.0),
        (at.1 - r.top) / r.height().max(1.0),
    );
    // The nearest edge within the outer quarter splits; a tab alone in its own area has
    // nothing to split off from.
    let alone = tabs.len() == 1 && tabs[0] == panel;
    let edge = [
        (fx, false, true),
        (1.0 - fx, false, false),
        (fy, true, true),
        (1.0 - fy, true, false),
    ]
    .into_iter()
    .filter(|(d, ..)| *d < 0.25)
    .min_by(|a, b| a.0.total_cmp(&b.0));
    match edge {
        Some((_, vertical, before)) if !alone => {
            let (w, h) = (r.width(), r.height());
            let target = match (vertical, before) {
                (false, true) => rect(r.left, r.top, w / 2.0, h),
                (false, false) => rect(r.left + w / 2.0, r.top, w / 2.0, h),
                (true, true) => rect(r.left, r.top, w, h / 2.0),
                (true, false) => rect(r.left, r.top + h / 2.0, w, h / 2.0),
            };
            Some((
                Drop::Split {
                    area: id,
                    vertical,
                    before,
                },
                target,
            ))
        }
        _ => Some((
            Drop::Tab {
                area: id,
                index: tabs.len(),
            },
            rect(r.left, r.top, r.width(), r.height()),
        )),
    }
}

/// Moves `panel` to `drop` (one layout change, saved like any other).
fn place(s: &mut Studio, panel: &str, drop: Drop) {
    let params = match drop {
        Drop::Tab { area, index } => json!({"panel": panel, "area": area, "index": index}),
        Drop::Split {
            area,
            vertical,
            before,
        } => {
            // A fresh area id: the panel's name, numbered past any area already called so.
            let taken = drag_keys(&s.editor.state.layout);
            let new_area = (1..)
                .map(|n| format!("{panel}-{n}"))
                .find(|id| !taken.contains(&format!("area/{id}")))
                .unwrap_or_else(|| panel.to_owned());
            json!({"panel": panel, "area": area, "newArea": new_area, "vertical": vertical, "before": before})
        }
    };
    s.act("editor.layout", params);
    s.act("editor.layout.save", json!({}));
}

/// While a tab is dragged: where it would land, shaded, and its name at the pointer.
pub(super) fn drag_preview(s: &Studio) -> Vec<El<Studio>> {
    let Some((panel, at)) = &s.local.tab_drag else {
        return Vec::new();
    };
    let l = &s.look;
    let c = &l.c;
    let mut out = Vec::new();
    if let Some((_, r)) = drop_at(s, panel, *at) {
        out.push(
            el().key("dock/drop")
                .absolute(r.left, r.top)
                .w(r.width())
                .h(r.height())
                .radius(style::RADIUS)
                .fill(c.primary.0.with_alpha(0.18)),
        );
    }
    let (title, glyph) = panel_title(panel);
    out.push(
        style::float(l, row())
            .absolute(at.0 + 12.0, at.1 + 10.0)
            .h(26.0)
            .pad_x(10.0)
            .gap(6.0)
            .align(Align::Center)
            .radius(l.radius.sm)
            .child(icon(glyph).size(12.0).color(c.primary.0))
            .child(text(title).text_size(12.0).color(c.foreground.0)),
    );
    out
}

pub(super) fn view(s: &Studio) -> El<Studio> {
    // A maximized panel (Window › Maximize, Game's Maximize on Play) fills the dock alone,
    // in its own area, until it is restored.
    if let Some(panel) = s
        .editor
        .state
        .maximized
        .as_deref()
        .filter(|p| !p.is_empty())
        && let Some(id) = area_of(&s.editor.state.layout, panel)
    {
        return area(s, &id, &[panel.to_owned()], 0).grow(1.0).pad(3.0);
    }
    // Panels sit 3 px apart on the window's darkest tone: one rhythm for gutters and splitters.
    node(s, &s.editor.state.layout, &mut Vec::new())
        .pad(3.0)
        .gap(0.0)
}

/// The id of the area whose tabs hold `panel`.
fn area_of(n: &DockNode, panel: &str) -> Option<String> {
    match n {
        DockNode::Area { id, tabs, .. } => tabs.iter().any(|t| t == panel).then(|| id.clone()),
        DockNode::Split { first, second, .. } => {
            area_of(first, panel).or_else(|| area_of(second, panel))
        }
    }
}

fn node(s: &Studio, n: &DockNode, path: &mut Vec<u8>) -> El<Studio> {
    match n {
        DockNode::Area { id, tabs, active } => area(s, id, tabs, *active),
        DockNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let horizontal = matches!(axis, DockAxis::Horizontal);
            path.push(0);
            let a = node(s, first, path);
            path.pop();
            path.push(1);
            let b = node(s, second, path);
            path.pop();
            let ratio = ratio.clamp(0.1, 0.9);
            // A zero base size and grow by ratio: the children share exactly the split's room.
            let (a, b) = if horizontal {
                (a.w(0.0), b.w(0.0))
            } else {
                (a.h(0.0), b.h(0.0))
            };
            let container = if horizontal { row() } else { col() };
            container
                .key(split_key(path))
                .grow(1.0)
                .min_w(0.0)
                .min_h(0.0)
                .child(a.grow(ratio))
                .child(splitter(s, path.clone(), horizontal))
                .child(b.grow(1.0 - ratio))
        }
    }
}

/// The 3 px gap between two children; a hairline lights only while hovered or dragged.
fn splitter(s: &Studio, path: Vec<u8>, horizontal: bool) -> El<Studio> {
    let l = &s.look;
    let dragging = s.local.drag.as_ref().is_some_and(|d| d.path == path);
    let line = if dragging {
        l.c.ring.0
    } else {
        stillkit::Rgba::TRANSPARENT
    };
    let hot = l.c.ring.0.with_alpha(0.6);
    let key = format!("splitter/{}", split_key(&path));
    let hair = if horizontal {
        el().w(1.0).h_full()
    } else {
        el().h(1.0).w_full()
    };
    let hit = if horizontal {
        row().w(3.0).h_full().justify(Justify::Center)
    } else {
        col().h(3.0).w_full().justify(Justify::Center)
    };
    hit.key(key)
        .align(Align::Center)
        .cursor(if horizontal {
            Cursor::ResizeHorizontal
        } else {
            Cursor::ResizeVertical
        })
        .label("Resize")
        .child(hair.fill(line).hovered(move |e| e.fill(hot)))
        .on_pointer(move |s: &mut Studio, e, _| {
            match e.phase {
                PointerPhase::Down => {
                    s.local.drag = Some(SplitDrag {
                        path: path.clone(),
                        horizontal,
                    })
                }
                PointerPhase::Move => {
                    let Some(rect) = s.local.rects.get(&split_key(&path)).copied() else {
                        return false;
                    };
                    let (at, start, len) = if horizontal {
                        (e.at.x, rect.left, rect.width())
                    } else {
                        (e.at.y, rect.top, rect.height())
                    };
                    if len < 1.0 {
                        return false;
                    }
                    let ratio = ((at - start) / len).clamp(0.1, 0.9);
                    if let Some(DockNode::Split { ratio: r, .. }) =
                        split_at(&mut s.editor.state.layout, &path)
                    {
                        *r = ratio;
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    s.local.drag = None;
                    s.act("editor.layout.save", json!({}));
                }
                PointerPhase::Hover => return false,
            }
            true
        })
}

/// The split at `path` in the layout.
fn split_at<'a>(node: &'a mut DockNode, path: &[u8]) -> Option<&'a mut DockNode> {
    let Some((&first, rest)) = path.split_first() else {
        return Some(node);
    };
    match node {
        DockNode::Split {
            first: a,
            second: b,
            ..
        } => split_at(if first == 0 { a } else { b }, rest),
        DockNode::Area { .. } => None,
    }
}

/// Every panel a dock area can hold, in the Window menu's order.
pub(super) const PANELS: [&str; 17] = [
    "scene",
    "game",
    "hierarchy",
    "inspector",
    "assets",
    "console",
    "problems",
    "agents",
    "history",
    "material",
    "vfx",
    "animation",
    "profiler",
    "lighting",
    "build",
    "import",
    "settings",
];

/// The id of the area whose tabs hold `panel`, in the main window's layout.
pub(super) fn area_holding(s: &Studio, panel: &str) -> Option<String> {
    area_of(&s.editor.state.layout, panel)
}

/// A panel's title and icon.
pub(super) fn panel_title(id: &str) -> (&'static str, stillkit::icons::Icon) {
    match id {
        "hierarchy" => ("Hierarchy", icons::LIST_TREE),
        "inspector" => ("Inspector", icons::SLIDERS_HORIZONTAL),
        "scene" => ("Scene", icons::BOX),
        "game" => ("Game", icons::GAMEPAD_2),
        "assets" => ("Assets", icons::FOLDER),
        "console" => ("Console", icons::SQUARE_TERMINAL),
        "problems" => ("Problems", icons::CIRCLE_ALERT),
        "agents" => ("Agents", icons::BOT),
        "history" => ("History", icons::HOURGLASS),
        "material" => ("Material", icons::PALETTE),
        "vfx" => ("VFX", icons::SPARKLES),
        "animation" => ("Animation", icons::PERSON_STANDING),
        "profiler" => ("Profiler", icons::ACTIVITY),
        "lighting" => ("Lighting", icons::LIGHTBULB),
        "build" => ("Build", icons::HAMMER),
        "import" => ("Import", icons::DOWNLOAD),
        "settings" => ("Settings", icons::SETTINGS),
        _ => ("Panel", icons::SQUARE),
    }
}

/// A tabbed area: a tab strip a step above the window's gaps, its active tab joined to the
/// panel body below it. No borders or rules: tone tells the surfaces apart.
fn area(s: &Studio, id: &str, tabs: &[String], active: usize) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let t = style::tones(l);
    let active_id = tabs
        .get(active)
        .or(tabs.first())
        .cloned()
        .unwrap_or_default();
    let strip = row()
        .h(style::STRIP)
        .pad_x(2.0)
        .gap(1.0)
        .align(Align::End)
        .children(tabs.iter().enumerate().map(|(i, tab)| {
            let on = i == active;
            let (title, glyph) = panel_title(tab);
            let (hover_fill, hover_ink) = (t.hover, c.foreground.0);
            let panel = tab.clone();
            let mut tab_el = row()
                .key(format!("tab/{tab}"))
                .h(style::STRIP - 3.0)
                .pad_x(10.0)
                .gap(6.0)
                .align(Align::Center)
                .radius(style::RADIUS)
                .role(stillkit::Role::Tab)
                .label(title)
                .selected(on)
                .fill(if on {
                    t.panel
                } else {
                    stillkit::Rgba::TRANSPARENT
                })
                .color(if on {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                })
                .cascade_ink()
                // A press shows it; a drag carries it to another area or an area's edge.
                .on_pointer(move |s: &mut Studio, e, _| {
                    if e.button != stillkit::Button::Left {
                        return false;
                    }
                    match e.phase {
                        PointerPhase::Down => {
                            s.act("editor.layout.show", json!({"panel": panel}));
                        }
                        PointerPhase::Move
                            if s.local.tab_drag.is_some()
                                || e.travel().x.abs() + e.travel().y.abs() >= 6.0 =>
                        {
                            s.local.tab_drag = Some((panel.clone(), (e.at.x, e.at.y)));
                        }
                        PointerPhase::Up => {
                            if let Some((panel, at)) = s.local.tab_drag.take()
                                && let Some((drop, _)) = drop_at(s, &panel, at)
                            {
                                place(s, &panel, drop);
                            }
                        }
                        PointerPhase::Cancel => s.local.tab_drag = None,
                        _ => return false,
                    }
                    true
                })
                .on_context_menu({
                    let tab = tab.clone();
                    move |s: &mut Studio, x, y| {
                        use crate::catalog::{Target, menus::Context};
                        s.local.context = Some((Context::Tab, Target::Panel(tab.clone()), (x, y)));
                    }
                })
                .child(icon(glyph).size(12.0))
                .child(
                    text(title)
                        .text_size(style::TEXT)
                        .weight(if on { 600 } else { 500 }),
                );
            // The open tab closes from its ×, as Unreal's do (the Scene view stays).
            if on && tab != "scene" {
                let close = crate::catalog::Target::Panel(tab.clone());
                tab_el = tab_el.child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Close tab")
                        .size(16.0)
                        .key(format!("tab/{tab}/close"))
                        .on_press(move |s: &mut Studio| {
                            super::commands::run(s, "window.close_panel", &close)
                        }),
                );
            }
            tab_el = if on {
                // Square off the bottom corners so the tab runs into the body.
                tab_el.child(
                    el().absolute(0.0, style::STRIP - 3.0 - style::RADIUS)
                        .w_full()
                        .h(style::RADIUS)
                        .fill(t.panel),
                )
            } else {
                tab_el.hovered(move |e| e.color(hover_ink).fill(hover_fill.with_alpha(0.6)))
            };
            tab_el
        }))
        .child(el().grow(1.0))
        .children(area_tools(s, &active_id));
    let body = match active_id.as_str() {
        "scene" => scene::view(s),
        "game" => game::view(s),
        "hierarchy" => hierarchy::view(s),
        "inspector" => inspector::view(s),
        other => panels::view(s, other),
    };
    let surface = if matches!(active_id.as_str(), "scene" | "game") {
        c.background.0
    } else {
        t.panel
    };
    col()
        .key(format!("area/{id}"))
        .min_w(0.0)
        .min_h(0.0)
        .radius(style::RADIUS)
        .fill(t.strip)
        .child(strip)
        .child(
            col()
                .grow(1.0)
                .min_h(0.0)
                .radius(style::RADIUS)
                .fill(surface)
                .child(body.grow(1.0)),
        )
}

/// Tools at the end of an area's tab strip for its active panel.
fn area_tools(s: &Studio, panel: &str) -> Vec<El<Studio>> {
    let l = &s.look;
    match panel {
        "scene" => {
            let mut button = icon_button(l, Tone::Ghost, Size::Sm, icons::LAYERS, "Overlays (`)")
                .key("scene/overlays")
                .on_press(|s: &mut Studio| s.local.overlays_menu = !s.local.overlays_menu);
            if s.local.overlays_menu {
                button = button.overlay(
                    stillkit::el::menu(menu_panel(l, super::scene_overlays::menu_items(s)))
                        .side(stillkit::el::Side::Bottom)
                        .align(stillkit::el::Align::End)
                        .on_dismiss(|s: &mut Studio| s.local.overlays_menu = false),
                );
            }
            vec![button]
        }
        "hierarchy" => vec![
            icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, "Create (Shift+A)")
                .key("hierarchy/create")
                .on_press(|s: &mut Studio| {
                    super::create::toggle(s, super::create::Anchor::Key("hierarchy/create"), None)
                })
                .overlay(super::create::overlay(
                    s,
                    super::create::Anchor::Key("hierarchy/create"),
                )),
        ],
        _ => Vec::new(),
    }
}
