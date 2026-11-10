//! The Scene view's overlays, as Unity's Overlays work: each toolbar (Tools, Transform,
//! Snapping, View, Visibility, Camera, Orientation) docks to an edge of the view,
//! horizontal along the top and bottom and vertical along the sides, counted from one
//! of that edge's corners. Drag an overlay's grip to move it: on release it docks to
//! the nearest edge, against the corner when within [`SNAP`] px. Click the grip to
//! collapse it to its icon (the icon opens it as a popup); right-click for its menu
//! (collapse, dock to an edge, hide). The Scene tab's Overlays menu, or the ` key, shows
//! and hides each and resets them. Let go away from every edge, an overlay floats where it
//! lands, laid out as a titled Panel, a Horizontal or a Vertical strip (its menu switches);
//! dragged back to an edge it docks again. Layouts persist in the `viewport.overlays`
//! setting, and a named window layout (Window › Layouts) saves and restores them too.
use std::collections::BTreeMap;

use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, El, Justify, PointerPhase, col, el, icon, menu, popover, row, text};

use super::Studio;
use super::commands;
use crate::catalog::Target;
use crate::state::Tool;

/// Gap between a docked overlay and the view's edge, and between overlays.
const INSET: f32 = 8.0;
/// Corner snap distance on release, design px.
const SNAP: f32 = 24.0;
/// How far from every edge a release floats an overlay instead of docking it, design px.
const FLOAT: f32 = 48.0;

/// How a floating overlay lays out, as Unity's floating overlays can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    /// Its title above its tools.
    Panel,
    /// Its tools in a row.
    Horizontal,
    /// Its tools in a column.
    Vertical,
}

impl Layout {
    const ALL: [(Layout, &'static str, &'static str); 3] = [
        (Layout::Panel, "panel", "Panel"),
        (Layout::Horizontal, "horizontal", "Horizontal"),
        (Layout::Vertical, "vertical", "Vertical"),
    ];
}

/// A floating overlay: where its top-left is in the view (design px), and its layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Float {
    pub at: (f32, f32),
    pub layout: Layout,
}

/// A Scene view edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Where an overlay docks: its edge, which corner its offset counts from (`end`: right or
/// bottom), how far from that corner, whether only its icon shows, and whether it shows.
/// A floating one (`float`) sits where it was let go instead, keeping its last edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Dock {
    pub edge: Edge,
    pub end: bool,
    pub offset: f32,
    pub collapsed: bool,
    pub hidden: bool,
    pub float: Option<Float>,
}

impl Dock {
    const fn at(edge: Edge, end: bool) -> Self {
        Self {
            edge,
            end,
            offset: 0.0,
            collapsed: false,
            hidden: false,
            float: None,
        }
    }

    /// Parses `edge:start|end:offset[:collapsed][:hidden]`, or a floating overlay's
    /// `float:x:y:panel|horizontal|vertical[:collapsed][:hidden]`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(':');
        let head = parts.next()?;
        if head == "float" {
            let mut number = || {
                parts
                    .next()?
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 10_000.0))
            };
            let at = (number()?, number()?);
            let layout = parts.next()?;
            let layout = Layout::ALL.iter().find(|(_, t, _)| *t == layout)?.0;
            let mut dock = Self {
                float: Some(Float { at, layout }),
                ..Self::at(Edge::Top, false)
            };
            for flag in parts {
                match flag {
                    "collapsed" => dock.collapsed = true,
                    "hidden" => dock.hidden = true,
                    _ => return None,
                }
            }
            return Some(dock);
        }
        let edge = match head {
            "top" => Edge::Top,
            "bottom" => Edge::Bottom,
            "left" => Edge::Left,
            "right" => Edge::Right,
            _ => return None,
        };
        let end = match parts.next()? {
            "start" => false,
            "end" => true,
            _ => return None,
        };
        let offset = parts
            .next()?
            .parse::<f32>()
            .ok()
            .filter(|o| o.is_finite())?;
        let mut dock = Self {
            offset: offset.clamp(0.0, 10_000.0),
            ..Self::at(edge, end)
        };
        for flag in parts {
            match flag {
                "collapsed" => dock.collapsed = true,
                "hidden" => dock.hidden = true,
                _ => return None,
            }
        }
        Some(dock)
    }

    /// The canonical `viewport.overlays` value.
    pub fn text(&self) -> String {
        let flags = format!(
            "{}{}",
            if self.collapsed { ":collapsed" } else { "" },
            if self.hidden { ":hidden" } else { "" }
        );
        if let Some(f) = self.float {
            let layout = Layout::ALL
                .iter()
                .find(|(l, ..)| *l == f.layout)
                .map_or("panel", |(_, t, _)| *t);
            return format!(
                "float:{}:{}:{layout}{flags}",
                f.at.0.round(),
                f.at.1.round()
            );
        }
        let edge = match self.edge {
            Edge::Top => "top",
            Edge::Bottom => "bottom",
            Edge::Left => "left",
            Edge::Right => "right",
        };
        format!(
            "{edge}:{}:{}{flags}",
            if self.end { "end" } else { "start" },
            self.offset.round(),
        )
    }

    /// Side docks lay their overlays out vertically; a floating one as its layout says
    /// (a Panel's tools run in a row under its title).
    pub fn vertical(&self) -> bool {
        match self.float {
            Some(f) => f.layout == Layout::Vertical,
            None => matches!(self.edge, Edge::Left | Edge::Right),
        }
    }

    /// The dock a release at overlay box (`x`, `y`, `w`, `h`) inside a `view` of `size`
    /// lands in: floating where it is when it is [`FLOAT`] from every edge (as it was laid
    /// out, a Panel at first), else the edge nearest the box's centre (the box kept inside
    /// the view), its nearer corner, the distance from it (zero within [`SNAP`]); flags
    /// carry over.
    pub fn drop_at(self, (x, y, w, h): (f32, f32, f32, f32), size: (f32, f32)) -> Self {
        // Kept inside the view first, so an overlay pushed past an edge counts from it.
        let x = x.clamp(0.0, (size.0 - w).max(0.0));
        let y = y.clamp(0.0, (size.1 - h).max(0.0));
        if x.min(y).min(size.0 - (x + w)).min(size.1 - (y + h)) > FLOAT {
            let layout = self.float.map_or(Layout::Panel, |f| f.layout);
            return Self {
                float: Some(Float { at: (x, y), layout }),
                ..self
            };
        }
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let edge = [
            (cy, Edge::Top),
            (size.1 - cy, Edge::Bottom),
            (cx, Edge::Left),
            (size.0 - cx, Edge::Right),
        ]
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Edge::Top, |(_, e)| e);
        let (start, length, extent) = match edge {
            Edge::Top | Edge::Bottom => (x, w, size.0),
            Edge::Left | Edge::Right => (y, h, size.1),
        };
        let end = start + length * 0.5 > extent * 0.5;
        let from_corner = if end {
            extent - (start + length)
        } else {
            start
        } - INSET;
        let offset = if from_corner < SNAP {
            0.0
        } else {
            from_corner.round()
        };
        Self {
            edge,
            end,
            offset,
            float: None,
            ..self
        }
    }
}

/// One overlay: its id, title, icon and default dock.
pub(super) struct Def {
    pub id: &'static str,
    pub title: &'static str,
    glyph: stillkit::icons::Icon,
    default: Dock,
}

/// Every Scene overlay, in default order (tools along the top-left, view options on the
/// right beside the orientation gizmo).
pub(super) const OVERLAYS: [Def; 7] = [
    Def {
        id: "orientation",
        title: "Orientation",
        glyph: icons::AXIS_3D,
        default: Dock::at(Edge::Top, true),
    },
    Def {
        id: "tools",
        title: "Tools",
        glyph: icons::MOVE_3D,
        default: Dock::at(Edge::Top, false),
    },
    Def {
        id: "transform",
        title: "Transform",
        glyph: icons::GLOBE,
        default: Dock::at(Edge::Top, false),
    },
    Def {
        id: "snapping",
        title: "Snapping",
        glyph: icons::MAGNET,
        default: Dock::at(Edge::Top, false),
    },
    Def {
        id: "view",
        title: "View",
        glyph: icons::VIDEO,
        default: Dock::at(Edge::Top, false),
    },
    Def {
        id: "visibility",
        title: "Gizmos and visibility",
        glyph: icons::EYE,
        default: Dock::at(Edge::Top, true),
    },
    Def {
        id: "camera",
        title: "Camera",
        glyph: icons::CAMERA,
        default: Dock::at(Edge::Top, true),
    },
];

fn def(id: &str) -> Option<&'static Def> {
    OVERLAYS.iter().find(|d| d.id == id)
}

/// The saved layouts (`viewport.overlays`).
fn saved(s: &Studio) -> BTreeMap<String, Value> {
    s.editor
        .state
        .settings
        .get("viewport.overlays")
        .and_then(Value::as_object)
        .map(|m| m.clone().into_iter().collect())
        .unwrap_or_default()
}

/// Where overlay `id` docks now: its saved layout, else its default.
pub(super) fn dock(s: &Studio, id: &str) -> Dock {
    let default = def(id).map_or(Dock::at(Edge::Top, false), |d| d.default);
    s.editor
        .state
        .settings
        .get("viewport.overlays")
        .and_then(|m| m.get(id))
        .and_then(Value::as_str)
        .and_then(Dock::parse)
        .unwrap_or(default)
}

/// Saves overlay `id`'s dock (one settings change).
fn save(s: &mut Studio, id: &str, dock: Dock) {
    let mut layouts = saved(s);
    layouts.insert(id.to_owned(), json!(dock.text()));
    s.act(
        "editor.settings.set",
        json!({"key": "viewport.overlays", "value": layouts}),
    );
}

/// Changes overlay `id`'s dock with `change` and saves it.
pub(super) fn update(s: &mut Studio, id: &str, change: impl FnOnce(&mut Dock)) {
    let mut d = dock(s, id);
    change(&mut d);
    save(s, id, d);
}

/// The saved overlay presets' names (`viewport.overlayPresets`).
pub(super) fn presets(s: &Studio) -> Vec<String> {
    s.editor
        .state
        .settings
        .get("viewport.overlayPresets")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Saves the overlays' layout as preset `name`, loads it, or deletes it (`action`).
pub(super) fn preset(s: &mut Studio, action: &str, name: &str) -> ara_ui::Result<()> {
    let mut presets = s
        .editor
        .state
        .settings
        .get("viewport.overlayPresets")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // A preset is one string: `id=edge:corner:offset[:collapsed][:hidden]` pairs, `;` apart.
    match action {
        "save" => {
            let text = saved(s)
                .iter()
                .filter_map(|(id, dock)| Some(format!("{id}={}", dock.as_str()?)))
                .collect::<Vec<_>>()
                .join(";");
            presets.insert(name.to_owned(), json!(text));
        }
        "load" => {
            let text = presets
                .get(name)
                .and_then(Value::as_str)
                .ok_or("That preset is gone")?;
            let layout: serde_json::Map<String, Value> = text
                .split(';')
                .filter_map(|pair| pair.split_once('='))
                .filter(|(id, dock)| def(id).is_some() && Dock::parse(dock).is_some())
                .map(|(id, dock)| (id.to_owned(), json!(dock)))
                .collect();
            set(s, "viewport.overlays", Value::Object(layout));
            return Ok(());
        }
        _ => {
            presets.remove(name).ok_or("That preset is gone")?;
        }
    }
    set(s, "viewport.overlayPresets", Value::Object(presets));
    Ok(())
}

/// Puts every overlay back where it starts.
pub(super) fn reset(s: &mut Studio) {
    s.act(
        "editor.settings.set",
        json!({"key": "viewport.overlays", "value": {}}),
    );
}

/// An overlay being dragged by its grip: which, where in it the grip was taken (overlay
/// px), and where the pointer is (window px).
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub id: &'static str,
    grab: (f32, f32),
    at: (f32, f32),
}

/// A user setting as a flag.
fn flag(s: &Studio, key: &str, default: bool) -> bool {
    s.editor
        .state
        .settings
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

/// A user setting as a number.
fn number(s: &Studio, key: &str, default: f64) -> f64 {
    s.editor
        .state
        .settings
        .get(key)
        .and_then(Value::as_f64)
        .unwrap_or(default)
}

fn set(s: &mut Studio, key: &str, value: Value) {
    s.act("editor.settings.set", json!({"key": key, "value": value}));
}

/// An overlay's length along its edge while collapsed to its icon (grip and icon).
const COLLAPSED: f32 = 44.0;

/// Records each expanded overlay's length along its edge from the rects the host just
/// reported (a squeezed or collapsed one keeps its last expanded length).
pub(super) fn measure(s: &mut Studio) {
    for d in &OVERLAYS {
        let vertical = dock(s, d.id).vertical();
        if let Some(r) = s.local.rects.get(&format!("scene/overlay/{}", d.id)) {
            let length = if vertical { r.height() } else { r.width() };
            if length > COLLAPSED + 4.0 {
                s.local.overlay_lengths.insert(d.id, length);
            }
        }
    }
}

/// Overlays that do not fit along their edge, shown collapsed to their icons until the
/// view has room again (Unity's overflow): the ones farthest from their corners go first.
/// An edge holds two stacks, one from each corner, each laid out as `stack` lays it out
/// (an overlay's offset is its gap from the corner), so both must fit side by side.
fn squeezed(s: &Studio) -> std::collections::BTreeSet<&'static str> {
    let mut out = std::collections::BTreeSet::new();
    let Some(view) = s.local.rects.get("scene/view") else {
        return out;
    };
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        let room = match edge {
            Edge::Top | Edge::Bottom => view.width(),
            Edge::Left | Edge::Right => view.height(),
        } - 2.0 * INSET;
        // This edge's shown overlays: which corner, offset from it, id, natural length.
        let mut items: Vec<(bool, f32, &'static str, f32)> = OVERLAYS
            .iter()
            .filter_map(|d| {
                let at = dock(s, d.id);
                (at.edge == edge && !at.hidden && at.float.is_none()).then(|| {
                    let length = match at.collapsed {
                        true => COLLAPSED,
                        false => s
                            .local
                            .overlay_lengths
                            .get(d.id)
                            .copied()
                            .unwrap_or(COLLAPSED),
                    };
                    (at.end, at.offset, d.id, length)
                })
            })
            .collect();
        items.sort_by(|a, b| a.1.total_cmp(&b.1));
        // How far both stacks reach along the edge with `out` collapsed.
        let reach = |out: &std::collections::BTreeSet<&'static str>| -> f32 {
            let side = |end: bool| {
                let mut at = 0.0_f32;
                let mut first = true;
                for (_, offset, id, length) in items.iter().filter(|i| i.0 == end) {
                    let gap = (offset - at).max(if first { 0.0 } else { 6.0 });
                    at += gap + if out.contains(id) { COLLAPSED } else { *length };
                    first = false;
                }
                at
            };
            let (start, end) = (side(false), side(true));
            start + end + if start > 0.0 && end > 0.0 { 6.0 } else { 0.0 }
        };
        // Farthest from its corner first: where each starts along its stack, as laid out.
        let mut order: Vec<(f32, &'static str)> = Vec::new();
        for end in [false, true] {
            let mut at = 0.0_f32;
            for (i, (_, offset, id, length)) in items.iter().filter(|i| i.0 == end).enumerate() {
                at += (offset - at).max(if i == 0 { 0.0 } else { 6.0 });
                if *length > COLLAPSED {
                    order.push((at, id));
                }
                at += length;
            }
        }
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, id) in order {
            if reach(&out) <= room {
                break;
            }
            out.insert(id);
        }
    }
    out
}

/// The overlays over the Scene view: one stack per edge and corner, then the overlay
/// being dragged at the pointer, with its landing edge marked.
pub(super) fn layer(s: &Studio) -> Vec<El<Studio>> {
    // View › Hide Overlays (Shift+G) clears the image of every one.
    if s.editor.state.hide_overlays {
        return Vec::new();
    }
    let dragging = s.local.overlay_drag;
    let mut stacks: BTreeMap<(u8, bool), Vec<(f32, &'static Def)>> = BTreeMap::new();
    let mut floating = Vec::new();
    for d in &OVERLAYS {
        let at = dock(s, d.id);
        if at.hidden {
            continue;
        }
        if let Some(f) = at.float {
            floating.push((f, d));
            continue;
        }
        stacks
            .entry((at.edge as u8, at.end))
            .or_default()
            .push((at.offset, d));
    }
    let squeezed = squeezed(s);
    let mut out = Vec::new();
    for ((edge, end), mut items) in stacks {
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        let edge = [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right][edge as usize];
        out.push(stack(s, edge, end, &items, &squeezed));
    }
    // Floating overlays where they were let go, kept inside the view as it resizes.
    let view = s.local.rects.get("scene/view").copied();
    for (f, d) in floating {
        let size = s
            .local
            .rects
            .get(&format!("scene/overlay/{}", d.id))
            .map_or((0.0, 0.0), |r| (r.width(), r.height()));
        let (x, y) = view.map_or(f.at, |v| {
            (
                f.at.0.min((v.width() - size.0).max(0.0)),
                f.at.1.min((v.height() - size.1).max(0.0)),
            )
        });
        out.push(overlay(s, d, f.layout == Layout::Vertical, false).absolute(x, y));
    }
    if let Some(drag) = dragging {
        out.extend(ghost(s, drag));
    }
    out
}

/// One edge corner's overlays, laid out from the corner; each offset is kept as a gap
/// from the overlay before it (measured last frame), so overlays never overlap.
fn stack(
    s: &Studio,
    edge: Edge,
    end: bool,
    items: &[(f32, &'static Def)],
    squeezed: &std::collections::BTreeSet<&'static str>,
) -> El<Studio> {
    let vertical = matches!(edge, Edge::Left | Edge::Right);
    let mut reach = 0.0;
    let mut children = Vec::new();
    for (offset, d) in items {
        let gap = (offset - reach).max(if children.is_empty() { 0.0 } else { 6.0 });
        let length = s
            .local
            .rects
            .get(&format!("scene/overlay/{}", d.id))
            .map_or(0.0, |r| if vertical { r.height() } else { r.width() });
        reach += gap + length;
        if gap > 0.0 {
            children.push(if vertical { el().h(gap) } else { el().w(gap) });
        }
        children.push(overlay(s, d, vertical, squeezed.contains(d.id)));
    }
    if end {
        children.reverse();
    }
    let line = if vertical { col() } else { row() }
        .align(Align::Start)
        .children(children);
    // A full-size, empty box puts the line in its corner; empty space passes the pointer
    // through to the Scene.
    let (main, cross) = match (edge, end) {
        (Edge::Top | Edge::Left, false) => (Justify::Start, Justify::Start),
        (Edge::Top, true) => (Justify::End, Justify::Start),
        (Edge::Bottom, false) => (Justify::Start, Justify::End),
        (Edge::Bottom, true) => (Justify::End, Justify::End),
        (Edge::Left, true) => (Justify::End, Justify::Start),
        (Edge::Right, false) => (Justify::Start, Justify::End),
        (Edge::Right, true) => (Justify::End, Justify::End),
    };
    let along = if vertical {
        col().justify(main).align(match cross {
            Justify::End => Align::End,
            _ => Align::Start,
        })
    } else {
        col().justify(cross).align(match main {
            Justify::End => Align::End,
            _ => Align::Start,
        })
    };
    along
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .pad(INSET)
        .child(line)
}

/// The dragged overlay at the pointer, and the edge it would dock to.
fn ghost(s: &Studio, drag: Drag) -> Vec<El<Studio>> {
    let (Some(d), Some(view)) = (def(drag.id), s.local.rects.get("scene/view")) else {
        return Vec::new();
    };
    let size = s
        .local
        .rects
        .get(&format!("scene/overlay/{}", drag.id))
        .map_or((120.0, 30.0), |r| (r.width(), r.height()));
    let (x, y) = (
        drag.at.0 - view.left - drag.grab.0,
        drag.at.1 - view.top - drag.grab.1,
    );
    let target = dock(s, drag.id).drop_at((x, y, size.0, size.1), (view.width(), view.height()));
    let c = &s.look.c;
    let band = c.primary.0.with_alpha(0.35);
    let hint = match target.edge {
        _ if target.float.is_some() => el(),
        Edge::Top => el().absolute(0.0, 0.0).w_full().h(3.0),
        Edge::Bottom => el().absolute(0.0, view.height() - 3.0).w_full().h(3.0),
        Edge::Left => el().absolute(0.0, 0.0).w(3.0).h_full(),
        Edge::Right => el().absolute(view.width() - 3.0, 0.0).w(3.0).h_full(),
    }
    .fill(band);
    // A box of the overlay's size, its icon and title, where it would go; the overlay
    // itself stays put (dimmed) so its grip keeps the pointer until release.
    let (w, h) = if target.vertical() == dock(s, drag.id).vertical() {
        size
    } else {
        (size.1, size.0)
    };
    let ghost = row()
        .absolute(x, y)
        .w(w.max(26.0))
        .h(h.max(26.0))
        .pad_x(6.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(super::style::RADIUS + 1.0)
        .fill(super::style::tones(&s.look).strip.with_alpha(0.8))
        .border(1.0, c.primary.0.with_alpha(0.6))
        .child(icon(d.glyph).size(13.0).color(c.foreground.0));
    let ghost = if w > 90.0 {
        ghost.child(
            text(d.title)
                .text_size(11.5)
                .color(c.muted_foreground.0)
                .wrap(1),
        )
    } else {
        ghost
    };
    vec![hint, ghost]
}

/// One overlay: its grip, then its tools (or only its icon when collapsed, or `squeezed`
/// for room).
fn overlay(s: &Studio, d: &'static Def, vertical: bool, squeezed: bool) -> El<Studio> {
    let l = &s.look;
    let id = d.id;
    let mut at = dock(s, id);
    at.collapsed |= squeezed;
    let body = if at.collapsed {
        collapsed(s, d)
    } else {
        content(s, id, vertical)
    };
    let moving = s.local.overlay_drag.is_some_and(|g| g.id == id);
    // The axis gizmo sizes itself and floats over the image, as Unity's does; toolbars
    // are a 26 px band.
    let panel = at.float.is_some_and(|f| f.layout == Layout::Panel) && !at.collapsed;
    let base = if panel {
        // A Panel: its grip and title in a header, its tools in a row beneath.
        col()
            .pad(3.0)
            .gap(2.0)
            .radius(super::style::RADIUS + 1.0)
            .fill(super::style::tones(l).strip.with_alpha(0.94))
    } else if id == "orientation" && !at.collapsed {
        if vertical { col() } else { row() }
            .gap(2.0)
            .align(Align::Start)
            .radius(super::style::RADIUS + 1.0)
    } else {
        group(l, vertical)
    };
    // A toolbar: one Tab stop, arrows along it (WAI toolbar pattern).
    let base = base
        .key(format!("scene/overlay/{id}"))
        .role(stillkit::Role::Toolbar)
        .focus_group(if vertical {
            stillkit::el::Orientation::Vertical
        } else {
            stillkit::el::Orientation::Horizontal
        })
        .opacity(if moving { 0.4 } else { 1.0 })
        .label(d.title)
        .on_context_menu(move |s: &mut Studio, x, y| s.local.overlay_menu = Some((id, (x, y))));
    let mut out = if panel {
        base.child(
            row()
                .gap(4.0)
                .align(Align::Center)
                .child(grip(s, id, false))
                .child(
                    text(d.title)
                        .text_size(11.0)
                        .weight(600)
                        .color(l.c.muted_foreground.0),
                ),
        )
        .child(body)
    } else {
        base.child(grip(s, id, vertical)).child(body)
    };
    if let Some((_, (x, y))) = s.local.overlay_menu.filter(|(m, _)| *m == id) {
        out = out.overlay(
            menu(context_menu(s, d, at))
                .at(x, y)
                .on_dismiss(|s: &mut Studio| s.local.overlay_menu = None),
        );
    }
    out
}

/// A collapsed overlay: its icon, which opens its tools as a popup.
fn collapsed(s: &Studio, d: &'static Def) -> El<Studio> {
    let l = &s.look;
    let id = d.id;
    let open = s.local.overlay_popup == Some(id);
    let mut button = toggle(l, d.glyph, d.title, open)
        .key(format!("scene/overlay/{id}/open"))
        .on_press(move |s: &mut Studio| {
            s.local.overlay_popup = if s.local.overlay_popup == Some(id) {
                None
            } else {
                Some(id)
            }
        });
    if open {
        button = button.overlay(
            popover(content(s, id, false))
                .side(stillkit::el::Side::Bottom)
                .align(Align::Start)
                .offset(4.0)
                .on_dismiss(|s: &mut Studio| s.local.overlay_popup = None),
        );
    }
    button
}

/// The grip: drag to move the overlay, click to collapse or expand it.
fn grip(s: &Studio, id: &'static str, vertical: bool) -> El<Studio> {
    let c = &s.look.c;
    let (hover, ink) = (super::style::tones(&s.look).hover, c.foreground.0);
    el().key(format!("scene/overlay/{id}/grip"))
        .w(if vertical { 22.0 } else { 10.0 })
        .h(if vertical { 10.0 } else { 22.0 })
        .radius(3.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .color(c.muted_foreground.0.with_alpha(0.7))
        .cascade_ink()
        .label("Move, or click to collapse")
        .tooltip("Drag to move · click to collapse")
        .cursor(stillkit::Cursor::Move)
        .hovered(move |e| e.fill(hover).color(ink))
        .on_pointer(move |s: &mut Studio, e, _| {
            match e.phase {
                PointerPhase::Down => {
                    s.local.overlay_drag = None;
                    s.local.overlay_grab = Some((id, (e.local.x + 2.0, e.local.y + 2.0)));
                }
                PointerPhase::Move => {
                    let Some((_, grab)) = s.local.overlay_grab.filter(|(g, _)| *g == id) else {
                        return false;
                    };
                    if e.travel().x.abs() + e.travel().y.abs() > 4.0 {
                        s.local.overlay_drag = Some(Drag {
                            id,
                            grab,
                            at: (e.at.x, e.at.y),
                        });
                    }
                }
                PointerPhase::Up => {
                    s.local.overlay_grab = None;
                    match s.local.overlay_drag.take() {
                        Some(drag) => drop(s, drag),
                        None => update(s, id, |d| d.collapsed = !d.collapsed),
                    }
                }
                PointerPhase::Cancel => {
                    s.local.overlay_grab = None;
                    s.local.overlay_drag = None;
                }
                _ => return false,
            }
            true
        })
        .child(
            icon(if vertical {
                icons::GRIP_HORIZONTAL
            } else {
                icons::GRIP_VERTICAL
            })
            .size(12.0),
        )
}

/// Docks a dragged overlay where it was let go.
fn drop(s: &mut Studio, drag: Drag) {
    let Some(view) = s.local.rects.get("scene/view").copied() else {
        return;
    };
    let size = s
        .local
        .rects
        .get(&format!("scene/overlay/{}", drag.id))
        .map_or((120.0, 30.0), |r| (r.width(), r.height()));
    let (x, y) = (
        drag.at.0 - view.left - drag.grab.0,
        drag.at.1 - view.top - drag.grab.1,
    );
    let (w, h) = (view.width(), view.height());
    update(s, drag.id, |d| {
        *d = d.drop_at((x, y, size.0, size.1), (w, h))
    });
}

/// An overlay's right-click menu: collapse, dock to an edge, hide.
fn context_menu(s: &Studio, d: &'static Def, at: Dock) -> El<Studio> {
    let l = &s.look;
    let id = d.id;
    let mut items = vec![
        menu_heading(l, d.title),
        menu_item(
            l,
            &format!("scene/overlay/{id}/collapse"),
            Some(if at.collapsed {
                icons::MAXIMIZE_2
            } else {
                icons::MINIMIZE_2
            }),
            if at.collapsed { "Expand" } else { "Collapse" },
            None,
            false,
        )
        .on_press(move |s: &mut Studio| {
            s.local.overlay_menu = None;
            update(s, id, |d| d.collapsed = !d.collapsed);
        }),
        menu_separator(l),
    ];
    for (label, edge, end) in [
        ("Dock top left", Edge::Top, false),
        ("Dock top right", Edge::Top, true),
        ("Dock bottom left", Edge::Bottom, false),
        ("Dock bottom right", Edge::Bottom, true),
        ("Dock left", Edge::Left, false),
        ("Dock right", Edge::Right, false),
    ] {
        let here = at.float.is_none() && at.edge == edge && at.end == end;
        items.push(
            menu_check(l, &format!("scene/overlay/{id}/{label}"), label, here).on_press(
                move |s: &mut Studio| {
                    s.local.overlay_menu = None;
                    update(s, id, |d| {
                        // Last in its new stack, so it never lands under another.
                        *d = Dock {
                            edge,
                            end,
                            offset: 10_000.0,
                            float: None,
                            ..*d
                        };
                    });
                },
            ),
        );
    }
    items.push(menu_separator(l));
    // Floating layouts: a docked overlay floats where it is when given one.
    for (layout, slug, label) in Layout::ALL {
        let here = at.float.is_some_and(|f| f.layout == layout);
        items.push(
            menu_check(l, &format!("scene/overlay/{id}/{slug}"), label, here).on_press(
                move |s: &mut Studio| {
                    s.local.overlay_menu = None;
                    let place = floating_at(s, id);
                    update(s, id, |d| {
                        d.float = Some(Float {
                            at: d.float.map_or(place, |f| f.at),
                            layout,
                        });
                    });
                },
            ),
        );
    }
    items.push(menu_separator(l));
    items.push(
        menu_item(
            l,
            &format!("scene/overlay/{id}/hide"),
            Some(icons::EYE_OFF),
            "Hide",
            Some("`"),
            false,
        )
        .on_press(move |s: &mut Studio| {
            s.local.overlay_menu = None;
            update(s, id, |d| d.hidden = true);
        }),
    );
    menu_panel(l, items)
}

/// Where overlay `id` is in the Scene view now (design px from its top-left), for floating
/// it there.
fn floating_at(s: &Studio, id: &str) -> (f32, f32) {
    match (
        s.local.rects.get("scene/view"),
        s.local.rects.get(&format!("scene/overlay/{id}")),
    ) {
        (Some(v), Some(r)) => ((r.left - v.left).max(0.0), (r.top - v.top).max(0.0)),
        _ => (FLOAT + INSET, FLOAT + INSET),
    }
}

/// The Overlays menu (the Scene tab's button, or `): each overlay's visibility, and
/// Reset.
pub(super) fn menu_items(s: &Studio) -> Vec<El<Studio>> {
    let l = &s.look;
    let mut items = vec![menu_heading(l, "Overlays")];
    for d in &OVERLAYS {
        let id = d.id;
        let shown = !dock(s, id).hidden;
        items.push(
            menu_check(l, &format!("scene/overlays/{id}"), d.title, shown)
                .on_press(move |s: &mut Studio| update(s, id, |d| d.hidden = !d.hidden)),
        );
    }
    items.push(menu_separator(l));
    // Presets (Unity's overlay presets): every saved layout, and Save Preset As.
    let presets = presets(s);
    if !presets.is_empty() {
        items.push(menu_heading(l, "Presets"));
    }
    for name in presets {
        let (load, remove) = (Target::Text(name.clone()), Target::Text(name.clone()));
        items.push(
            menu_item(
                l,
                &format!("scene/overlays/preset/{name}"),
                Some(icons::LAYOUT_GRID),
                &name,
                None,
                false,
            )
            .on_press(move |s: &mut Studio| {
                s.local.overlays_menu = false;
                commands::run(s, "view.overlays.preset.load", &load);
            })
            .child(
                icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Delete this preset")
                    .size(18.0)
                    .key(format!("scene/overlays/preset/{name}/delete"))
                    .on_press(move |s: &mut Studio| {
                        commands::run(s, "view.overlays.preset.delete", &remove)
                    }),
            ),
        );
    }
    items.push(
        menu_item(
            l,
            "scene/overlays/save",
            Some(icons::BOOKMARK),
            "Save Preset As…",
            None,
            false,
        )
        .on_press(|s: &mut Studio| {
            s.local.overlays_menu = false;
            commands::run(s, "view.overlays.preset.save", &Target::None);
        }),
    );
    items.push(
        menu_item(
            l,
            "scene/overlays/reset",
            Some(icons::ROTATE_CCW),
            "Reset all overlays",
            None,
            false,
        )
        .on_press(|s: &mut Studio| {
            s.local.overlays_menu = false;
            reset(s);
        }),
    );
    items
}

/// The tools of overlay `id`.
fn content(s: &Studio, id: &str, vertical: bool) -> El<Studio> {
    let l = &s.look;
    let st = &s.editor.state;
    let line = || {
        if vertical {
            col().gap(1.0).align(Align::Center)
        } else {
            row().gap(1.0).align(Align::Center)
        }
    };
    match id {
        "orientation" => super::scene::orientation_widget(s),
        "tools" => {
            let tool = |id: &'static str, glyph, label: &str, on: bool| {
                toggle(l, glyph, label, on)
                    .key(format!("scene/{id}"))
                    .on_press(move |s: &mut Studio| commands::run(s, id, &Target::None))
            };
            line()
                .child(tool(
                    "view.tool.move",
                    icons::MOVE_3D,
                    "Move (W)",
                    st.tool == Tool::Translate,
                ))
                .child(tool(
                    "view.tool.rotate",
                    icons::ROTATE_3D,
                    "Rotate (E)",
                    st.tool == Tool::Rotate,
                ))
                .child(tool(
                    "view.tool.scale",
                    icons::SCALE_3D,
                    "Scale (R)",
                    st.tool == Tool::Scale,
                ))
                .child(tool(
                    "view.tool.universal",
                    icons::MOVE,
                    "Universal (T)",
                    st.tool == Tool::Universal,
                ))
        }
        "transform" => line()
            .child(
                labelled(
                    l,
                    if st.local { icons::BOX } else { icons::GLOBE },
                    if st.local { "Local" } else { "World" },
                    vertical,
                )
                .key("scene/space")
                .tooltip("Handle space: World or Local (X)")
                .on_press(|s: &mut Studio| commands::run(s, "view.space", &Target::None)),
            )
            .child(
                labelled(
                    l,
                    icons::CROSSHAIR,
                    if st.centre { "Centre" } else { "Pivot" },
                    vertical,
                )
                .key("scene/pivot")
                .tooltip("Handle position: Pivot or Centre (Z)")
                .on_press(|s: &mut Studio| commands::run(s, "view.pivot", &Target::None)),
            ),
        "snapping" => line()
            .child(
                toggle(l, icons::MAGNET, "Snapping", st.snapping)
                    .key("scene/snap")
                    .on_press(|s: &mut Studio| commands::run(s, "snap.toggle", &Target::None)),
            )
            .child(settings_button(s, "snapping", snapping_panel)),
        "view" => {
            let projection = if st.camera.projection == crate::state::CameraProjection::Perspective
            {
                "Perspective"
            } else {
                "Orthographic"
            };
            line()
                .child(draw_mode_button(s, vertical))
                .child(
                    labelled(l, icons::VIDEO, projection, vertical)
                        .key("scene/projection")
                        .tooltip("Switch projection")
                        .on_press(|s: &mut Studio| {
                            commands::run(s, "view.camera.perspective", &Target::None)
                        }),
                )
                .child(
                    labelled(l, icons::FOCUS, "Frame", vertical)
                        .key("scene/frame")
                        .tooltip("Frame the selection (F)")
                        .on_press(|s: &mut Studio| commands::run(s, "view.frame", &Target::None)),
                )
        }
        "visibility" => line()
            .child(
                toggle(l, icons::GRID_3X3, "Grid", flag(s, "viewport.grid", true))
                    .key("scene/grid")
                    .on_press(|s: &mut Studio| {
                        let on = flag(s, "viewport.grid", true);
                        set(s, "viewport.grid", json!(!on));
                    }),
            )
            .child(
                toggle(
                    l,
                    icons::SPARKLES,
                    "Gizmos",
                    flag(s, "viewport.gizmos", true),
                )
                .key("scene/gizmos")
                .on_press(|s: &mut Studio| {
                    let on = flag(s, "viewport.gizmos", true);
                    set(s, "viewport.gizmos", json!(!on));
                }),
            )
            .child(settings_button(s, "visibility", visibility_panel)),
        "camera" => line()
            .child(
                text(format!("{:.1} m/s", number(s, "viewport.cameraSpeed", 4.0)))
                    .text_size(11.0)
                    .family(mono())
                    .color(l.c.muted_foreground.0)
                    .pad_x(6.0),
            )
            .child(settings_button(s, "camera", camera_panel)),
        _ => el(),
    }
}

/// The render debug switches the draw-mode menu offers: title, and how to read and
/// flip it.
type Switch = (
    &'static str,
    fn(&ara_render::RenderDebug) -> bool,
    fn(&mut ara_render::RenderDebug),
);
const EFFECTS: [Switch; 9] = [
    (
        "Ambient occlusion",
        |d| d.ambient_occlusion,
        |d| d.ambient_occlusion = !d.ambient_occlusion,
    ),
    ("Shadows", |d| d.shadows, |d| d.shadows = !d.shadows),
    (
        "Soft shadows",
        |d| d.soft_shadows,
        |d| d.soft_shadows = !d.soft_shadows,
    ),
    (
        "Screen reflections",
        |d| d.screen_reflections,
        |d| d.screen_reflections = !d.screen_reflections,
    ),
    (
        "Reflection captures",
        |d| d.reflection_captures,
        |d| d.reflection_captures = !d.reflection_captures,
    ),
    (
        "Global illumination",
        |d| d.global_illumination,
        |d| d.global_illumination = !d.global_illumination,
    ),
    ("Screen GI", |d| d.screen_gi, |d| d.screen_gi = !d.screen_gi),
    (
        "Volumetric fog",
        |d| d.volumetric_fog,
        |d| d.volumetric_fog = !d.volumetric_fog,
    ),
    (
        "Temporal jitter",
        |d| d.temporal_jitter,
        |d| d.temporal_jitter = !d.temporal_jitter,
    ),
];

/// Flips one render debug switch: the Scene view reads it each frame, a running Play
/// gets it at once, and each Play start carries it.
pub(super) fn flip_effect(s: &mut Studio, flip: fn(&mut ara_render::RenderDebug)) {
    flip(&mut s.editor.state.render_debug);
    if let Some(world) = s.editor.play_world.clone() {
        let debug = s.editor.state.render_debug;
        s.act(
            "play.renderDebug",
            json!({"world": world, "renderDebug": debug}),
        );
    }
}

/// Unity's draw mode menu: the current view mode, opening every mode by group and the
/// effect switches.
fn draw_mode_button(s: &Studio, vertical: bool) -> El<Studio> {
    let l = &s.look;
    let current = crate::app::viewport::VIEW_MODES
        .iter()
        .find(|(name, ..)| *name == s.editor.state.view_mode)
        .map_or("Lit", |(_, title, ..)| *title);
    let effects_off = EFFECTS
        .iter()
        .filter(|(_, on, _)| !on(&s.editor.state.render_debug))
        .count();
    let label = if effects_off > 0 {
        format!("{current} · {effects_off} off")
    } else {
        current.to_owned()
    };
    let open = s.local.overlay_settings == Some("draw");
    let mut button = labelled(l, icons::SUN, &label, vertical)
        .key("scene/draw-mode")
        .tooltip("Draw mode and effects")
        .expanded(open)
        .on_press(|s: &mut Studio| {
            s.local.overlay_settings = if s.local.overlay_settings == Some("draw") {
                None
            } else {
                Some("draw")
            }
        });
    if open {
        button = button.overlay(
            popover(
                super::style::float(l, row())
                    .key("scene/draw-mode/panel")
                    .pad(8.0)
                    .gap(12.0)
                    .align(Align::Start)
                    .children(draw_mode_items(s)),
            )
            .side(stillkit::el::Side::Bottom)
            .align(Align::Start)
            .offset(6.0)
            .on_dismiss(|s: &mut Studio| s.local.overlay_settings = None),
        );
    }
    button
}

/// The draw mode menu's two columns: Shading and Material modes; Lighting and
/// Performance modes, then the effect switches.
fn draw_mode_items(s: &Studio) -> Vec<El<Studio>> {
    let l = &s.look;
    let column = |groups: &[&str]| {
        let mut out = col().w(190.0).gap(1.0);
        for group in groups {
            out = out.child(menu_heading(l, group));
            for (name, title, _, _) in crate::app::viewport::VIEW_MODES
                .iter()
                .filter(|(.., heading, _)| heading == group)
            {
                let on = s.editor.state.view_mode == *name
                    || (s.editor.state.view_mode.is_empty() && *name == "lit");
                let name = *name;
                out = out.child(
                    menu_check(l, &format!("scene/draw-mode/{name}"), title, on).on_press(
                        move |s: &mut Studio| {
                            s.local.overlay_settings = None;
                            s.act("editor.view", json!({"viewMode": name}));
                        },
                    ),
                );
            }
        }
        out
    };
    let mut second = column(&["Lighting", "Performance"]).child(menu_heading(l, "Effects"));
    for (title, on, flip) in EFFECTS {
        let checked = on(&s.editor.state.render_debug);
        second = second.child(
            menu_check(l, &format!("scene/effect/{title}"), title, checked)
                .on_press(move |s: &mut Studio| flip_effect(s, flip)),
        );
    }
    vec![column(&["Shading", "Material"]), second]
}

/// A chevron that opens overlay `id`'s settings panel.
fn settings_button(s: &Studio, id: &'static str, panel: fn(&Studio) -> El<Studio>) -> El<Studio> {
    let l = &s.look;
    let open = s.local.overlay_settings == Some(id);
    let mut button = toggle(l, icons::CHEVRON_DOWN, "Settings", open)
        .w(16.0)
        .key(format!("scene/overlay/{id}/settings"))
        .on_press(move |s: &mut Studio| {
            s.local.overlay_settings = if s.local.overlay_settings == Some(id) {
                None
            } else {
                Some(id)
            }
        });
    if open {
        button = button.overlay(
            popover(
                super::style::float(l, col())
                    .w(260.0)
                    .pad(12.0)
                    .gap(8.0)
                    .child(panel(s)),
            )
            .side(stillkit::el::Side::Bottom)
            .align(Align::Start)
            .offset(6.0)
            .on_dismiss(|s: &mut Studio| s.local.overlay_settings = None),
        );
    }
    button
}

/// A labelled row in a settings panel.
fn setting(s: &Studio, label: &str, control: El<Studio>) -> El<Studio> {
    row()
        .min_h(26.0)
        .gap(8.0)
        .align(Align::Center)
        .child(
            text(label.to_owned())
                .text_size(12.0)
                .color(s.look.c.muted_foreground.0)
                .w(0.0)
                .grow(1.0),
        )
        .child(control)
}

/// A checkbox row for a flag setting.
fn flag_row(s: &Studio, key: &'static str, label: &str, default: bool) -> El<Studio> {
    let on = flag(s, key, default);
    setting(
        s,
        label,
        checkbox(&s.look, format!("scene/setting/{key}"), label, on)
            .on_press(move |s: &mut Studio| set(s, key, json!(!on))),
    )
}

/// A number row: typed, committed on Enter, clamped by the setting itself.
fn number_row(
    s: &Studio,
    key: &'static str,
    label: &str,
    value: f64,
    unit: &str,
    commit: impl Fn(&mut Studio, f64) + 'static,
) -> El<Studio> {
    let shown = format!("{}", (value * 1000.0).round() / 1000.0);
    setting(
        s,
        label,
        row()
            .w(96.0)
            .gap(4.0)
            .align(Align::Center)
            .child(
                field(
                    &s.look,
                    Size::Sm,
                    format!("scene/setting/{key}"),
                    None,
                    "",
                    &shown,
                    move |x| {
                        x.select_all_on_focus().on_submit(move |s: &mut Studio, t| {
                            if let Ok(v) = t.trim().parse::<f64>()
                                && v.is_finite()
                            {
                                commit(s, v);
                            }
                        })
                    },
                )
                .w(0.0)
                .grow(1.0),
            )
            .child(
                text(unit.to_owned())
                    .text_size(11.0)
                    .color(s.look.c.muted_foreground.0),
            ),
    )
}

/// Snapping: the move, rotate and scale steps, surface and vertex snapping.
fn snapping_panel(s: &Studio) -> El<Studio> {
    let st = &s.editor.state;
    let step = |s: &mut Studio, field: &str, v: f64| {
        if v > 0.0 {
            let tool = s.editor.state.tool;
            s.act("editor.tool", json!({"tool": tool, field: v}));
        }
    };
    col()
        .gap(2.0)
        .child(
            text("Snapping")
                .text_size(12.5)
                .weight(600)
                .color(s.look.c.foreground.0),
        )
        .child(number_row(
            s,
            "move",
            "Move step",
            f64::from(st.move_snap),
            "m",
            move |s, v| step(s, "moveSnap", v),
        ))
        .child(number_row(
            s,
            "rotate",
            "Rotate step",
            f64::from(st.rotate_snap),
            "°",
            move |s, v| step(s, "rotateSnap", v),
        ))
        .child(number_row(
            s,
            "scale",
            "Scale step",
            f64::from(st.scale_snap),
            "×",
            move |s, v| step(s, "scaleSnap", v),
        ))
        .child(flag_row(
            s,
            "viewport.surfaceSnap",
            "Surface snapping",
            false,
        ))
        .child(flag_row(
            s,
            "viewport.vertexSnap",
            "Vertex snapping (V)",
            false,
        ))
}

/// Gizmos and visibility: what draws over the Scene, and how big.
fn visibility_panel(s: &Studio) -> El<Studio> {
    col()
        .gap(2.0)
        .child(
            text("Gizmos and visibility")
                .text_size(12.5)
                .weight(600)
                .color(s.look.c.foreground.0),
        )
        .child(flag_row(s, "viewport.grid", "Grid", true))
        .child(flag_row(s, "viewport.outline", "Selection outline", true))
        .child(flag_row(s, "viewport.gizmos", "Component gizmos", true))
        .child(flag_row(s, "viewport.icons", "Icons", true))
        .child(flag_row(s, "viewport.icons3d", "3D icons", true))
        .child(flag_row(s, "viewport.bounds", "Mesh bounds", false))
        .child(flag_row(s, "viewport.colliders", "Collider shapes", false))
        .child(flag_row(s, "viewport.stats", "Scene statistics", false))
        .child(number_row(
            s,
            "viewport.gizmoSize",
            "Gizmo size",
            number(s, "viewport.gizmoSize", 100.0),
            "%",
            |s, v| set(s, "viewport.gizmoSize", json!(v.clamp(60.0, 140.0))),
        ))
        .child(number_row(
            s,
            "viewport.iconSize",
            "Icon size",
            number(s, "viewport.iconSize", 100.0),
            "%",
            |s, v| set(s, "viewport.iconSize", json!(v.clamp(25.0, 200.0))),
        ))
}

/// Camera: fly speed, field of view, and the Scene's render quality.
fn camera_panel(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let quality = s
        .editor
        .state
        .settings
        .get("viewport.quality")
        .and_then(Value::as_str)
        .unwrap_or("project")
        .to_owned();
    let qualities = crate::settings::QUALITY_OVERRIDES
        .iter()
        .map(|(value, title)| {
            let on = *value == quality;
            let value = *value;
            super::style::chosen(l, row(), on)
                .key(format!("scene/quality/{value}"))
                .h(24.0)
                .pad_x(8.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(*title)
                .selected(on)
                .on_press(move |s: &mut Studio| set(s, "viewport.quality", json!(value)))
                .child(text(*title).text_size(11.5).color(if on {
                    l.c.foreground.0
                } else {
                    l.c.muted_foreground.0
                }))
        });
    col()
        .gap(2.0)
        .child(
            text("Camera")
                .text_size(12.5)
                .weight(600)
                .color(l.c.foreground.0),
        )
        .child(number_row(
            s,
            "viewport.cameraSpeed",
            "Fly speed",
            number(s, "viewport.cameraSpeed", 4.0),
            "m/s",
            |s, v| set(s, "viewport.cameraSpeed", json!(v.clamp(0.5, 50.0))),
        ))
        .child(number_row(
            s,
            "fov",
            "Field of view",
            f64::from(s.editor.state.camera.fov_y_degrees),
            "°",
            |s, v| {
                s.act(
                    "editor.camera",
                    json!({"fovYDegrees": v.clamp(10.0, 120.0)}),
                )
            },
        ))
        .child(
            text("Scene quality")
                .text_size(12.0)
                .color(l.c.muted_foreground.0)
                .pad_y(4.0),
        )
        .child(stillkit::el::grid(3).gap(2.0).children(qualities))
        .children(bookmarks(s))
}

/// Saved views of the open scene (Unreal's bookmarks): recall one, delete one, or save
/// the current view as the next `View N`.
fn bookmarks(s: &Studio) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let mut out = vec![
        row()
            .pad_y(4.0)
            .align(Align::Center)
            .child(
                text("Saved views")
                    .text_size(12.0)
                    .color(c.muted_foreground.0)
                    .grow(1.0),
            )
            .child(
                button(l, Tone::Ghost, Size::Sm, Some(icons::PLUS), "Save view")
                    .key("scene/bookmark/save")
                    .on_press(|s: &mut Studio| {
                        s.act("editor.camera.bookmark", json!({"action": "save"}))
                    }),
            ),
    ];
    let saved = s.editor.state.camera_bookmarks.get(&s.editor.state.doc);
    if saved.is_none_or(|b| b.is_empty()) {
        out.push(
            text("None yet: save the view you are looking at to come back to it.")
                .text_size(11.5)
                .wrap(2)
                .color(c.muted_foreground.0),
        );
    }
    for name in saved.into_iter().flat_map(|b| b.keys()) {
        let (recall, delete, hover) = (name.clone(), name.clone(), c.hover_surface.0);
        out.push(
            row()
                .key(format!("scene/bookmark/{name}"))
                .h(26.0)
                .pad_x(6.0)
                .gap(6.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(name.clone())
                .hovered(move |e| e.fill(hover))
                .on_press(move |s: &mut Studio| {
                    s.act(
                        "editor.camera.bookmark",
                        json!({"action": "recall", "name": recall}),
                    )
                })
                .child(icon(icons::BOOKMARK).size(12.0).color(c.muted_foreground.0))
                .child(
                    text(name.clone())
                        .text_size(12.0)
                        .color(c.foreground.0)
                        .grow(1.0),
                )
                .child(
                    toggle(l, icons::X, "Delete", false)
                        .size(20.0)
                        .key(format!("scene/bookmark/{name}/delete"))
                        .on_press(move |s: &mut Studio| {
                            s.act(
                                "editor.camera.bookmark",
                                json!({"action": "delete", "name": delete}),
                            )
                        }),
                ),
        );
    }
    out
}

/// An overlay's surface: the panels' strip tone, nearly opaque, no border, so it reads
/// as the same UI as the docks around it.
pub(super) fn group(l: &ara_kit::look::Look, vertical: bool) -> El<Studio> {
    let base = if vertical {
        col().w(26.0).pad_y(2.0)
    } else {
        row().h(26.0).pad_x(2.0)
    };
    base.gap(1.0)
        .align(Align::Center)
        .radius(super::style::RADIUS + 1.0)
        .fill(super::style::tones(l).strip.with_alpha(0.94))
}

/// An icon toggle in an overlay: the chosen tone while on, like a selected row.
pub(super) fn toggle(
    l: &ara_kit::look::Look,
    glyph: stillkit::icons::Icon,
    label: &str,
    on: bool,
) -> El<Studio> {
    let c = &l.c;
    let (hover, ink) = (super::style::tones(l).hover, c.foreground.0);
    super::style::chosen(l, row(), on)
        .size(22.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(super::style::RADIUS)
        .color(if on { ink } else { c.muted_foreground.0 })
        .cascade_ink()
        .label(label)
        .tooltip(label)
        .checked(on)
        .hovered(move |e| if on { e } else { e.fill(hover).color(ink) })
        .child(icon(glyph).size(14.0))
}

/// An icon and label button in an overlay; vertical overlays show the icon only.
fn labelled(
    l: &ara_kit::look::Look,
    glyph: stillkit::icons::Icon,
    label: &str,
    vertical: bool,
) -> El<Studio> {
    let c = &l.c;
    let (hover, ink) = (super::style::tones(l).hover, c.foreground.0);
    let mut out = row()
        .h(22.0)
        .pad_x(if vertical { 4.0 } else { 7.0 })
        .gap(5.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(super::style::RADIUS)
        .color(c.muted_foreground.0)
        .cascade_ink()
        .label(label)
        .hovered(move |e| e.fill(hover).color(ink))
        .child(icon(glyph).size(13.0));
    if !vertical {
        out = out.child(text(label.to_owned()).text_size(super::style::TEXT));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docks_round_trip_and_reject_malformed_text() {
        for text in [
            "top:start:0",
            "right:end:40:collapsed",
            "bottom:start:12:hidden",
            "left:end:5:collapsed:hidden",
        ] {
            assert_eq!(Dock::parse(text).map(|d| d.text()).as_deref(), Some(text));
        }
        for bad in [
            "",
            "top",
            "top:middle:0",
            "top:start:x",
            "top:start:1:sideways",
        ] {
            assert!(Dock::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_release_docks_to_the_nearest_edge_and_snaps_into_its_corner() {
        let start = Dock::at(Edge::Top, false);
        let size = (800.0, 600.0);
        // Near the bottom right corner.
        let d = start.drop_at((680.0, 560.0, 100.0, 26.0), size);
        assert_eq!((d.edge, d.end, d.offset), (Edge::Bottom, true, 0.0));
        // Along the left side, well below the corner.
        let d = start.drop_at((4.0, 200.0, 26.0, 90.0), size);
        assert_eq!((d.edge, d.end), (Edge::Left, false));
        assert!((d.offset - 192.0).abs() < 1.0, "{d:?}");
        // In the lower half it counts from the bottom corner.
        let d = start.drop_at((4.0, 400.0, 26.0, 90.0), size);
        assert_eq!((d.edge, d.end), (Edge::Left, true));
        assert!((d.offset - 102.0).abs() < 1.0, "{d:?}");
        // Flags carry over.
        let d = Dock {
            collapsed: true,
            ..start
        }
        .drop_at((10.0, 10.0, 80.0, 26.0), size);
        assert!(d.collapsed && d.offset == 0.0);
    }
}
