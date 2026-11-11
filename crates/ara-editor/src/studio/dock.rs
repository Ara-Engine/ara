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

/// Where native floating windows lay out: far right of the main window, one band each,
/// so each shows only its own region (`Runtime::present_region`).
const PORTAL_X: f32 = 100_000.0;
/// The width of each floating window's band.
const PORTAL_BAND: f32 = 20_000.0;
/// A native floating window's title bar height (design px).
pub(super) const PORTAL_BAR: f32 = 28.0;

/// The region floating window `index` lays out in (main-window design px), sized as its
/// native window's client area: what the host presents into that window.
pub(super) fn region(s: &Studio, index: usize, id: &str, rect: [i32; 4]) -> stillkit::Rect {
    let (w, h) = s
        .local
        .desktop
        .sizes
        .get(id)
        .copied()
        .unwrap_or((rect[2] as f32, (rect[3] as f32 - PORTAL_BAR).max(1.0)));
    stillkit::Rect::new(PORTAL_X + index as f32 * PORTAL_BAND, 0.0, w, h)
}

/// The windows floating over the main one: native windows of their own when the host
/// shows them so (`Local::desktop`), else drawn inside the main window.
pub(super) fn floating(s: &Studio) -> Vec<El<Studio>> {
    if s.local.desktop.on {
        return native_floating(s);
    }
    inner_floating(s)
}

/// Native floating windows: each lays out in its own region, a title bar the system drags
/// (`float/{id}/bar`, `float/{id}/close` are the host's chrome parts) above its panels' dock
/// tree. The system moves and resizes them; the host reports both back to the layout.
fn native_floating(s: &Studio) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let t = style::tones(l);
    s.editor
        .state
        .floating
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let id = &f.id;
            let r = region(s, i, id, f.rect);
            let panels = tabs_of(&f.root);
            let title = window_title(&f.root);
            let mut bar = row()
                .key(format!("float/{id}/bar"))
                .h(PORTAL_BAR)
                .pad_x(6.0)
                .gap(6.0)
                .align(Align::Center)
                .fill(t.strip)
                .label(format!("{title} window"))
                .child(el().w(s.local.desktop.inset.max(0.0)))
                .child(
                    row().grow(1.0).justify(Justify::Center).child(
                        text(title)
                            .text_size(style::TEXT)
                            .weight(500)
                            .color(c.muted_foreground.0)
                            .wrap(1),
                    ),
                );
            // Dock back: every panel of the window where the default layout keeps it (the
            // way back where dragging between windows can't work, as on Wayland).
            let home = panels.clone();
            bar = bar.child(
                icon_button(
                    l,
                    Tone::Ghost,
                    Size::Sm,
                    icons::PANEL_BOTTOM,
                    "Dock in main window",
                )
                .size(20.0)
                .key(format!("float/{id}/dock"))
                .on_press(move |s: &mut Studio| {
                    for panel in &home {
                        let target = crate::catalog::Target::Panel(panel.clone());
                        super::commands::run(s, "window.dock", &target);
                    }
                }),
            );
            // macOS draws its own close (the traffic lights, in the inset).
            if !cfg!(target_os = "macos") {
                bar = bar.child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Close window")
                        .size(20.0)
                        .key(format!("float/{id}/close"))
                        .on_press(move |s: &mut Studio| close_panels(s, &panels)),
                );
            } else {
                bar = bar.child(el().w(s.local.desktop.inset.max(0.0)));
            }
            col()
                .key(format!("float/{id}"))
                .absolute(r.left, r.top)
                .w(r.width())
                .h(r.height())
                .fill(t.gap)
                .child(bar)
                .child(
                    node(s, &f.root, &mut vec![2 + i as u8])
                        .grow(1.0)
                        .min_h(0.0)
                        .pad(3.0),
                )
        })
        .collect()
}

/// Closes every panel of a floating window (its own window goes with the last).
pub(super) fn close_panels(s: &mut Studio, panels: &[String]) {
    for panel in panels {
        s.act("editor.layout.close", json!({"panel": panel}));
    }
}

/// The windows floating inside the main one (as Unreal's can): each a title bar that moves
/// it, its panels' dock tree, and a corner that resizes it; tabs drag in and out as between
/// any areas. Their rects are window design px, kept inside the window.
fn inner_floating(s: &Studio) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let (sw, sh) = s.size;
    s.editor
        .state
        .floating
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let id = f.id.clone();
            let mut r = f.rect.map(|v| v as f32);
            if let Some(drag) = s.local.float_drag.as_ref().filter(|d| d.id == id) {
                r = drag.rect();
            }
            let [x, y, w, h] = fit([r[0], r[1], r[2], r[3]], (sw, sh));
            let gesture = |resize: bool| {
                let id = id.clone();
                move |s: &mut Studio, e: &stillkit::el::PointerEvent, _: &mut stillkit::el::Tx| {
                    match e.phase {
                        PointerPhase::Down => {
                            s.local.float_drag = Some(super::state::FloatDrag {
                                id: id.clone(),
                                resize,
                                start: [x, y, w, h],
                                travel: (0.0, 0.0),
                            })
                        }
                        PointerPhase::Move => {
                            if let Some(drag) = s.local.float_drag.as_mut() {
                                drag.travel = (e.travel().x, e.travel().y);
                            }
                        }
                        PointerPhase::Up => {
                            if let Some(drag) = s.local.float_drag.take() {
                                place_window(s, &drag.id, fit(drag.rect(), s.size));
                            }
                        }
                        PointerPhase::Cancel => s.local.float_drag = None,
                        PointerPhase::Hover => return false,
                    }
                    true
                }
            };
            let panels: Vec<String> = tabs_of(&f.root);
            let close = panels.clone();
            let title = panels
                .first()
                .map_or("Window", |p| panel_title(p).0)
                .to_owned();
            // A slim drag strip: the tabs below already name what the window holds.
            let bar = row()
                .key(format!("float/{id}/bar"))
                .h(18.0)
                .pad_x(4.0)
                .align(Align::Center)
                .cursor(Cursor::Move)
                .label(format!("{title} window: drag to move"))
                .on_pointer(gesture(false))
                .child(el().size(18.0))
                .child(
                    row().grow(1.0).justify(Justify::Center).child(
                        icon(icons::GRIP_HORIZONTAL)
                            .size(12.0)
                            .color(c.muted_foreground.0),
                    ),
                )
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Close window")
                        .size(18.0)
                        .key(format!("float/{id}/close"))
                        .on_press(move |s: &mut Studio| close_panels(s, &close)),
                );
            let grip = el()
                .key(format!("float/{id}/resize"))
                .absolute(w - 14.0, h - 14.0)
                .size(14.0)
                .cursor(Cursor::Resize)
                .label("Resize")
                .on_pointer(gesture(true));
            style::float(l, col())
                .key(format!("float/{id}"))
                .absolute(x, y)
                .w(w)
                .h(h)
                .radius(style::RADIUS + 2.0)
                .child(bar)
                .child(
                    node(s, &f.root, &mut vec![2 + i as u8])
                        .grow(1.0)
                        .min_h(0.0)
                        .pad(3.0),
                )
                .child(grip)
        })
        .collect()
}

/// A floating window's rect kept usable inside the main window: at least 240 × 160, its
/// title bar always reachable.
fn fit([x, y, w, h]: [f32; 4], (sw, sh): (f32, f32)) -> [f32; 4] {
    let (w, h) = (w.clamp(240.0, sw.max(240.0)), h.clamp(160.0, sh.max(160.0)));
    [
        x.clamp(0.0, (sw - 80.0).max(0.0)),
        y.clamp(0.0, (sh - 40.0).max(0.0)),
        w,
        h,
    ]
}

/// Moves or resizes floating window `id` to `rect` (design px), kept with the layout.
pub(super) fn place_window(s: &mut Studio, id: &str, rect: [f32; 4]) {
    s.act(
        "editor.window",
        json!({"id": id, "rect": rect.map(f32::round)}),
    );
}

/// A floating window's name: its panels' titles, as its tabs read.
pub(super) fn window_title(root: &DockNode) -> String {
    let titles: Vec<&str> = tabs_of(root).iter().map(|p| panel_title(p).0).collect();
    titles.join(", ")
}

/// Every panel in `node`, in order.
pub(super) fn tabs_of(node: &DockNode) -> Vec<String> {
    match node {
        DockNode::Area { tabs, .. } => tabs.clone(),
        DockNode::Split { first, second, .. } => {
            let mut out = tabs_of(first);
            out.extend(tabs_of(second));
            out
        }
    }
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

/// [`drag_keys`] of the main tree and every floating window's.
pub(super) fn all_drag_keys(s: &Studio) -> Vec<String> {
    let mut keys = drag_keys(&s.editor.state.layout);
    for f in &s.editor.state.floating {
        keys.extend(drag_keys(&f.root));
    }
    keys
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
    // Floating windows lie over the main one: they take the pointer first.
    let mut all = Vec::new();
    for f in &s.editor.state.floating {
        areas(&f.root, &mut all);
    }
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

/// A tab dropped over no area becomes a window of its own under the pointer: on the desktop
/// (native windows: wherever the pointer left the editor's windows), else in the main one.
fn tear_out(s: &mut Studio, panel: &str, at: (f32, f32)) {
    let (x, y) = match (s.local.desktop.on, s.local.desktop.pointer) {
        (true, Some(p)) => p,
        (true, None) => (
            s.local.desktop.origin.0 + at.0,
            s.local.desktop.origin.1 + at.1,
        ),
        (false, _) => at,
    };
    let rect = [x - 60.0, y - 12.0, 480.0, 360.0].map(f32::round);
    s.act(
        "editor.layout.move",
        json!({"panel": panel, "slot": "float", "rect": rect}),
    );
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
            let taken = all_drag_keys(s);
            let new_area = (1..)
                .map(|n| format!("{panel}-{n}"))
                .find(|id| !taken.contains(&format!("area/{id}")))
                .unwrap_or_else(|| panel.to_owned());
            json!({"panel": panel, "area": area, "newArea": new_area, "vertical": vertical, "before": before})
        }
    };
    s.act("editor.layout", params);
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
            // A floating window's splits (their paths start at 2) are fixed for now: the
            // drag below resizes the main window's tree.
            if path.first().is_some_and(|p| *p >= 2) {
                return false;
            }
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
                    // The ratio the drag left, kept by the authority (and the layout file).
                    let ratio = match split_at(&mut s.editor.state.layout, &path) {
                        Some(DockNode::Split { ratio, .. }) => Some(*ratio),
                        _ => None,
                    };
                    if let Some(ratio) = ratio {
                        let path: Vec<bool> = path.iter().map(|p| *p == 1).collect();
                        s.act("editor.layout", json!({"path": path, "ratio": ratio}));
                    }
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
pub(super) const PANELS: [&str; 18] = [
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
    "mesh-paint",
];

/// The id of the area whose tabs hold `panel`, in the main window or a floating one.
pub(super) fn area_holding(s: &Studio, panel: &str) -> Option<String> {
    area_of(&s.editor.state.layout, panel).or_else(|| {
        s.editor
            .state
            .floating
            .iter()
            .find_map(|f| area_of(&f.root, panel))
    })
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
        "mesh-paint" => ("Mesh Paint", icons::BRUSH),
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
    // One Tab stop per strip, on its open tab; Left and Right move along it.
    let strip = row()
        .h(style::STRIP)
        .pad_x(2.0)
        .gap(1.0)
        .align(Align::End)
        .role(stillkit::Role::TabList)
        .focus_group(stillkit::el::Orientation::Horizontal)
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
                            if let Some((panel, at)) = s.local.tab_drag.take() {
                                match drop_at(s, &panel, at) {
                                    Some((drop, _)) => place(s, &panel, drop),
                                    None => tear_out(s, &panel, at),
                                }
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
