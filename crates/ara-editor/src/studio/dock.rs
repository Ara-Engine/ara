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

pub(super) fn view(s: &Studio) -> El<Studio> {
    // Panels sit 3 px apart on the window's darkest tone: one rhythm for gutters and splitters.
    node(s, &s.editor.state.layout, &mut Vec::new())
        .pad(3.0)
        .gap(0.0)
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
                .on_press(move |s: &mut Studio| {
                    s.act("editor.layout.show", json!({"panel": panel}))
                })
                .child(icon(glyph).size(12.0))
                .child(
                    text(title)
                        .text_size(style::TEXT)
                        .weight(if on { 600 } else { 500 }),
                );
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
        "hierarchy" => vec![
            icon_button(l, Tone::Ghost, Size::Sm, icons::PLUS, "Create (Shift+A)")
                .key("hierarchy/create")
                .on_press(|s: &mut Studio| {
                    super::create::toggle(s, super::create::Anchor::Key("hierarchy/create"), None)
                }),
        ],
        _ => Vec::new(),
    }
}
