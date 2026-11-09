//! Scene toolbars as movable overlays (Unity 2021+ Overlays). Each toolbar docks to an
//! edge of the Scene view, horizontal along the top and bottom and vertical along the
//! left and right, measured from one of that edge's corners, and collapses to its
//! grip. Drag a grip to move its toolbar: on release it snaps to the nearest edge, and
//! to the corner when within [`SNAP`] px. Click a grip to collapse or expand it.
//! Layouts persist per toolbar in `viewport.overlays`; `view.overlays.reset` restores
//! the defaults. The orientation widget keeps clear of the toolbars (`axis.rs`).
use super::commands::Target;
use super::*;
use serde_json::{Value, json};

/// Gap between a docked toolbar and the Scene view's edge (theme `viewport-inset`).
pub(super) const INSET: f32 = 8.;
/// Corner snap distance on release, in logical pixels.
const SNAP: f32 = 24.;

/// A Scene view edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}
/// Where a toolbar docks: its edge, which corner its offset counts from (`end`: right
/// or bottom), the offset along the edge, and whether only its grip shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dock {
    pub edge: Edge,
    pub end: bool,
    pub offset: f32,
    pub collapsed: bool,
}
/// The Scene toolbars: element id, settings key, accessible name, collapsed icon and
/// default dock (tools top-left, view options top-right).
pub(crate) const TOOLBARS: &[(&str, &str, &str, &str, Dock)] = &[
    (
        "scene-edit-tools",
        "tools",
        "Tools",
        "move-3d",
        Dock {
            edge: Edge::Top,
            end: false,
            offset: 0.,
            collapsed: false,
        },
    ),
    (
        "scene-view-tools",
        "view",
        "View options",
        "eye",
        Dock {
            edge: Edge::Top,
            end: true,
            offset: 0.,
            collapsed: false,
        },
    ),
];

impl Dock {
    /// Parse `edge:start|end:offset[:collapsed]`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(':');
        let edge = match parts.next()? {
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
        let offset = parts.next()?.parse::<f32>().ok()?;
        let collapsed = match parts.next() {
            None => false,
            Some("collapsed") => true,
            Some(_) => return None,
        };
        (offset.is_finite() && parts.next().is_none()).then_some(Self {
            edge,
            end,
            offset: offset.clamp(0., 10_000.),
            collapsed,
        })
    }
    /// The canonical `viewport.overlays` value.
    pub fn text(&self) -> String {
        let edge = match self.edge {
            Edge::Top => "top",
            Edge::Bottom => "bottom",
            Edge::Left => "left",
            Edge::Right => "right",
        };
        format!(
            "{edge}:{}:{}{}",
            if self.end { "end" } else { "start" },
            self.offset.round(),
            if self.collapsed { ":collapsed" } else { "" }
        )
    }
    /// Vertical toolbars dock to the left and right edges.
    pub fn vertical(&self) -> bool {
        matches!(self.edge, Edge::Left | Edge::Right)
    }
    /// Absolute placement inside the Scene surface.
    pub fn style(&self) -> String {
        let along = INSET + self.offset;
        let (across, along_side) = match (self.edge, self.end) {
            (Edge::Top, false) => ("top", "left"),
            (Edge::Top, true) => ("top", "right"),
            (Edge::Bottom, false) => ("bottom", "left"),
            (Edge::Bottom, true) => ("bottom", "right"),
            (Edge::Left, false) => ("left", "top"),
            (Edge::Left, true) => ("left", "bottom"),
            (Edge::Right, false) => ("right", "top"),
            (Edge::Right, true) => ("right", "bottom"),
        };
        format!(
            "position:absolute;{across}:{INSET}px;{along_side}:{along}px;flex-direction:{}",
            if self.vertical() { "column" } else { "row" }
        )
    }
    /// The dock for a toolbar released at `rect` over the Scene `viewport`: the
    /// nearest edge, counted from the nearer corner of that edge.
    pub fn snap(rect: Box2, viewport: Box2, collapsed: bool) -> Self {
        let centre = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        let distances = [
            (Edge::Top, centre[1] - viewport.y),
            (Edge::Bottom, viewport.y + viewport.h - centre[1]),
            (Edge::Left, centre[0] - viewport.x),
            (Edge::Right, viewport.x + viewport.w - centre[0]),
        ];
        let edge = distances
            .into_iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(Edge::Top, |(edge, _)| edge);
        let (start, end) = if matches!(edge, Edge::Top | Edge::Bottom) {
            (
                rect.x - viewport.x - INSET,
                viewport.x + viewport.w - (rect.x + rect.w) - INSET,
            )
        } else {
            (
                rect.y - viewport.y - INSET,
                viewport.y + viewport.h - (rect.y + rect.h) - INSET,
            )
        };
        let from_end = end < start;
        let offset = if from_end { end } else { start }.max(0.);
        Self {
            edge,
            end: from_end,
            offset: if offset < SNAP { 0. } else { offset.round() },
            collapsed,
        }
    }
}

/// A toolbar's dock from `viewport.overlays`, else its default.
pub(crate) fn dock(state: &crate::state::EditorState, id: &str) -> Dock {
    let Some((_, key, _, _, default)) = TOOLBARS.iter().find(|t| t.0 == id) else {
        return TOOLBARS[0].4;
    };
    state
        .settings
        .get("viewport.overlays")
        .and_then(|layouts| layouts.get(*key))
        .and_then(Value::as_str)
        .and_then(Dock::parse)
        .unwrap_or(*default)
}

impl Shell {
    /// Write toolbar `id` docked at `dock` around its controls (`body`), or its grip
    /// and an expand button when collapsed. Vertical toolbars stack their controls.
    pub(super) fn scene_toolbar(&mut self, out: &mut String, id: &str, dock: Dock, body: &str) {
        let Some((_, _, name, icon, _)) = TOOLBARS.iter().find(|t| t.0 == id) else {
            return;
        };
        let target = Target::Panel(id.into());
        self.bind_command(
            &format!("{id}-grip"),
            "view.overlays.collapse",
            target.clone(),
        );
        let vertical = if dock.vertical() {
            " scene-toolbar-vertical"
        } else {
            ""
        };
        let grip = if dock.vertical() {
            " scene-toolbar-grip-across"
        } else {
            ""
        };
        out.push_str(&format!("<panel id='{id}' class='viewport-cluster scene-toolbar{vertical}' style='{}' label='{name} toolbar'><button id='{id}-grip' class='scene-toolbar-grip{grip}' label='Move {name} toolbar' description='{name}: drag to dock on any edge · click to {}' action='Dispatch'>{}</button>",dock.style(),if dock.collapsed {"expand"} else {"collapse"},icons::icon_in(&format!("{id}-grip"),"grip-vertical","icon-xs")));
        if dock.collapsed {
            self.bind_command(&format!("{id}-expand"), "view.overlays.collapse", target);
            out.push_str(&format!("<button id='{id}-expand' class='viewport-button' label='Expand {name} toolbar' description='Expand the {name} toolbar' action='Dispatch'>{}</button>",icons::icon_in(&format!("{id}-expand"),icon,"viewport-icon")));
        } else if dock.vertical() {
            out.push_str(&body.replace(
                "class='viewport-divider'",
                "class='viewport-divider-across'",
            ));
        } else {
            out.push_str(body);
        }
        out.push_str("</panel>");
    }

    /// Store toolbar `id`'s dock and show it.
    fn set_toolbar_dock(
        &mut self,
        editor: &mut Editor,
        id: &str,
        dock: Dock,
    ) -> ara_ui::Result<()> {
        let Some((_, key, ..)) = TOOLBARS.iter().find(|t| t.0 == id) else {
            return Ok(());
        };
        let mut layouts = editor
            .state
            .settings
            .get("viewport.overlays")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        layouts.insert((*key).into(), json!(dock.text()));
        editor.action(
            "editor.settings.set",
            json!({"key": "viewport.overlays", "value": layouts}),
        )?;
        self.rebuild(editor)
    }

    /// Collapse or expand toolbar `id` (`view.overlays.collapse`).
    pub(crate) fn toggle_overlay_collapsed(
        &mut self,
        editor: &mut Editor,
        id: &str,
    ) -> ara_ui::Result<()> {
        let mut dock = dock(&editor.state, id);
        dock.collapsed = !dock.collapsed;
        self.set_toolbar_dock(editor, id, dock)
    }

    /// Start moving the toolbar whose grip is under the pointer.
    pub(super) fn begin_toolbar_drag(&mut self) -> bool {
        let [x, y] = self.pointer;
        let Some(index) = self.ui.hit_test(x, y) else {
            return false;
        };
        let hit = &self.ui.nodes[index].element.id;
        let hit = hit.strip_suffix("-icon").unwrap_or(hit);
        let Some(id) = hit
            .strip_suffix("-grip")
            .filter(|id| TOOLBARS.iter().any(|t| t.0 == *id))
        else {
            return false;
        };
        let Some(node) = self.shown_node(id) else {
            return false;
        };
        let rect = self.ui.nodes[node].rect;
        self.drag = Some(super::Drag::Toolbar {
            id: id.into(),
            grab: [x - rect.x, y - rect.y],
            start: self.pointer,
            moved: false,
        });
        true
    }

    /// Follow the pointer with the dragged toolbar (a paint offset; no relayout).
    pub(crate) fn drag_toolbar(&mut self) -> bool {
        let Some(super::Drag::Toolbar {
            id,
            grab,
            start,
            moved,
        }) = &mut self.drag
        else {
            return false;
        };
        let pointer = self.pointer;
        if !*moved && (pointer[0] - start[0]).abs() + (pointer[1] - start[1]).abs() < 4. {
            return false;
        }
        *moved = true;
        let (id, grab) = (id.clone(), *grab);
        let (Some(node), Some(viewport)) = (self.shown_node(&id), self.viewport()) else {
            return false;
        };
        let node = &mut self.ui.nodes[node];
        let base = [
            node.rect.x - node.paint_offset[0],
            node.rect.y - node.paint_offset[1],
        ];
        let x = (pointer[0] - grab[0]).clamp(
            viewport.x,
            (viewport.x + viewport.w - node.rect.w).max(viewport.x),
        );
        let y = (pointer[1] - grab[1]).clamp(
            viewport.y,
            (viewport.y + viewport.h - node.rect.h).max(viewport.y),
        );
        node.paint_offset = [x - base[0], y - base[1]];
        node.dirty.paint = true;
        true
    }

    /// Finish a toolbar drag: snap and store the new dock. False for a click (the
    /// grip's command then collapses the toolbar).
    pub(super) fn drop_toolbar(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(super::Drag::Toolbar { id, moved, .. }) = self.drag.take() else {
            return Ok(false);
        };
        let (Some(node), Some(viewport)) = (self.shown_node(&id), self.viewport()) else {
            return Ok(moved);
        };
        let rect = self.ui.nodes[node].rect;
        self.ui.nodes[node].paint_offset = [0.; 2];
        self.ui.nodes[node].dirty.paint = true;
        if !moved {
            return Ok(false);
        }
        let collapsed = dock(&editor.state, &id).collapsed;
        self.set_toolbar_dock(editor, &id, Dock::snap(rect, viewport, collapsed))?;
        Ok(true)
    }

    /// Rectangles of the docked toolbars, not the one being dragged (the orientation
    /// widget keeps clear of them).
    pub(super) fn toolbar_rects(&self) -> Vec<Box2> {
        let dragged = match &self.drag {
            Some(super::Drag::Toolbar { id, .. }) => Some(id.as_str()),
            _ => None,
        };
        TOOLBARS
            .iter()
            .filter(|(id, ..)| dragged != Some(*id))
            .filter_map(|(id, ..)| Some(self.ui.nodes[self.shown_node(id)?].rect))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const VIEW: Box2 = Box2 {
        x: 100.,
        y: 50.,
        w: 800.,
        h: 600.,
    };

    #[test]
    fn docks_round_trip_and_reject_malformed_text() {
        for text in [
            "top:start:0",
            "right:end:40:collapsed",
            "bottom:end:12",
            "left:start:300",
        ] {
            assert_eq!(Dock::parse(text).map(|d| d.text()).as_deref(), Some(text));
        }
        for text in [
            "",
            "top",
            "top:middle:0",
            "top:start:x",
            "top:start:0:open",
            "up:start:0",
        ] {
            assert_eq!(Dock::parse(text), None, "{text}");
        }
        assert!(Dock::parse("left:end:0").is_some_and(|d| d.vertical()));
        assert!(
            Dock::parse("top:end:0")
                .unwrap()
                .style()
                .contains("right:8px")
        );
    }

    #[test]
    fn releases_snap_to_the_nearest_edge_and_corner() {
        // Near the top-right corner: the top edge, counted from the right, snapped flush.
        let rect = Box2 {
            x: 600.,
            y: 60.,
            w: 280.,
            h: 32.,
        };
        assert_eq!(
            Dock::snap(rect, VIEW, false),
            Dock {
                edge: Edge::Top,
                end: true,
                offset: 0.,
                collapsed: false
            }
        );
        // Down the left side: vertical, its offset kept beyond the corner snap.
        let rect = Box2 {
            x: 104.,
            y: 200.,
            w: 32.,
            h: 200.,
        };
        let dock = Dock::snap(rect, VIEW, true);
        assert_eq!(
            (dock.edge, dock.end, dock.collapsed),
            (Edge::Left, false, true)
        );
        assert_eq!(dock.offset, 142.);
        // Bottom middle-right.
        let rect = Box2 {
            x: 500.,
            y: 610.,
            w: 200.,
            h: 32.,
        };
        let dock = Dock::snap(rect, VIEW, false);
        assert_eq!((dock.edge, dock.end), (Edge::Bottom, true));
        assert_eq!(dock.offset, 192.);
    }

    #[test]
    fn settings_choose_each_toolbars_dock() {
        let mut state = crate::state::EditorState::default();
        assert_eq!(dock(&state, "scene-view-tools"), TOOLBARS[1].4);
        state.settings.insert(
            "viewport.overlays".into(),
            json!({"view":"left:start:20:collapsed","tools":"nonsense"}),
        );
        assert_eq!(
            dock(&state, "scene-view-tools").text(),
            "left:start:20:collapsed"
        );
        assert_eq!(dock(&state, "scene-edit-tools"), TOOLBARS[0].4);
    }
}
