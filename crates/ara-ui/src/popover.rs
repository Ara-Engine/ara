//! Anchored popovers: menus, context menus, dropdowns, pickers, tooltips and palettes.
//!
//! A `<popover>` is a child of the UI root that [`Ui::frame`] places beside its anchor:
//! the element named by `anchor` (its painted rect, so an anchor inside a scrolled list
//! is followed) or the point `at='x,y'`. `placement` names the preferred side and
//! alignment (`bottom-start`, the default, `top-end`, `right-start`, `left-center`, ...)
//! and `offset` the gap to the anchor (4 px). A popover sizes to its content; it flips to
//! the opposite side when its content does not fit the preferred one and the other has
//! more room, shifts along the other axis to stay [`MARGIN`] inside the window and, when
//! still too tall, takes the room it has and scrolls its content (it is a scroll view).
//! It is a block container: children keep their natural heights.
//!
//! A host owns which popovers are open (it emits their markup); [`Popovers`] gives it the
//! shared behaviour: which ones a click outside dismisses, Escape, stacking (later
//! popovers are above earlier ones) and focus return when one closes.
use crate::{Result, Ui, input::InputState, paint::Box2, source::stable_id};

/// The distance a placed popover keeps from the window's edges, in logical pixels.
pub const MARGIN: f32 = 4.0;
/// The default gap between a popover and its anchor, in logical pixels.
pub const OFFSET: f32 = 4.0;

/// The side of its anchor a popover opens on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    /// Below the anchor (dropdowns, bar menus).
    #[default]
    Bottom,
    /// Above the anchor.
    Top,
    /// Right of the anchor (submenus).
    Right,
    /// Left of the anchor.
    Left,
}

/// How a popover lines up with its anchor along the side's axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Leading edges meet (left for `bottom`/`top`, top for `right`/`left`).
    #[default]
    Start,
    /// Centred on the anchor.
    Center,
    /// Trailing edges meet.
    End,
}

/// A preferred side and alignment, written `side-align` (`bottom-start`) or `side`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    /// Preferred side; the opposite one is used when it has more room.
    pub side: Side,
    /// Alignment along the side.
    pub align: Align,
}

impl std::str::FromStr for Placement {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let (side, align) = s.split_once('-').unwrap_or((s, "start"));
        let side = match side {
            "bottom" => Side::Bottom,
            "top" => Side::Top,
            "right" => Side::Right,
            "left" => Side::Left,
            _ => return Err(format!("ARA4046: unknown popover side `{side}`")),
        };
        let align = match align {
            "start" => Align::Start,
            "center" => Align::Center,
            "end" => Align::End,
            _ => return Err(format!("ARA4046: unknown popover alignment `{align}`")),
        };
        Ok(Self { side, align })
    }
}

/// Where a popover goes: its top-left, the size it may take (its content size, or less
/// when the window is too small) and the side it opened on after flipping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Top-left in logical pixels.
    pub origin: [f32; 2],
    /// Width and height; smaller than the content when it must scroll.
    pub size: [f32; 2],
    /// The side actually used.
    pub side: Side,
}

/// Place `content` (a popover's natural size) beside `anchor` in a `window`-sized
/// surface. A point anchor is an empty box.
pub fn place(
    anchor: Box2,
    content: [f32; 2],
    window: [f32; 2],
    placement: Placement,
    offset: f32,
) -> Placed {
    let vertical = matches!(placement.side, Side::Bottom | Side::Top);
    // Main axis: the side's axis; cross axis: the alignment's.
    let (a_main, a_main_len, a_cross, a_cross_len) = if vertical {
        (anchor.y, anchor.h, anchor.x, anchor.w)
    } else {
        (anchor.x, anchor.w, anchor.y, anchor.h)
    };
    let (w_main, w_cross) = if vertical {
        (window[1], window[0])
    } else {
        (window[0], window[1])
    };
    let (need, cross_need) = if vertical {
        (content[1], content[0])
    } else {
        (content[0], content[1])
    };
    let before = (a_main - offset - MARGIN).max(0.0);
    let after = (w_main - MARGIN - (a_main + a_main_len) - offset).max(0.0);
    let prefers_after = matches!(placement.side, Side::Bottom | Side::Right);
    let (preferred, other) = if prefers_after {
        (after, before)
    } else {
        (before, after)
    };
    let flip = need > preferred && other > preferred;
    let use_after = prefers_after != flip;
    let main_len = need.min(if use_after { after } else { before });
    let main = if use_after {
        a_main + a_main_len + offset
    } else {
        a_main - offset - main_len
    };
    let cross_len = cross_need.min((w_cross - 2.0 * MARGIN).max(0.0));
    let cross = match placement.align {
        Align::Start => a_cross,
        Align::Center => a_cross + (a_cross_len - cross_len) / 2.0,
        Align::End => a_cross + a_cross_len - cross_len,
    };
    let cross = cross.clamp(MARGIN, (w_cross - MARGIN - cross_len).max(MARGIN));
    let side = match (placement.side, flip) {
        (side, false) => side,
        (Side::Bottom, true) => Side::Top,
        (Side::Top, true) => Side::Bottom,
        (Side::Right, true) => Side::Left,
        (Side::Left, true) => Side::Right,
    };
    if vertical {
        Placed {
            origin: [cross, main],
            size: [cross_len, main_len],
            side,
        }
    } else {
        Placed {
            origin: [main, cross],
            size: [main_len, cross_len],
            side,
        }
    }
}

/// The position and limits `Ui::frame` gives a placed popover's layout style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Applied {
    origin: [f32; 2],
    /// Set when the content is larger than the room; it then scrolls.
    clamp: [Option<f32>; 2],
}

impl Applied {
    /// Write the placement into a layout style computed from CSS: the position, and a
    /// size only on a clamped axis (CSS sizes otherwise stand).
    pub(crate) fn apply(&self, style: &mut taffy::Style) {
        use taffy::prelude::*;
        style.position = Position::Absolute;
        style.inset.left = LengthPercentageAuto::length(self.origin[0]);
        style.inset.top = LengthPercentageAuto::length(self.origin[1]);
        if let Some(width) = self.clamp[0] {
            style.size.width = Dimension::length(width);
        }
        if let Some(height) = self.clamp[1] {
            style.size.height = Dimension::length(height);
        }
    }
}

impl Ui {
    /// The popover nodes, root children in stacking order (last is on top).
    pub fn popover_nodes(&self) -> impl Iterator<Item = usize> + '_ {
        self.nodes[0]
            .element
            .children
            .iter()
            .copied()
            .filter(|i| self.nodes[*i].element.tag == "popover" && self.shown(*i))
    }
    /// Place every shown popover against its anchor's painted rect; true when one
    /// moved or resized, so the frame lays out and paints again. Popovers whose anchor
    /// is missing or hidden stay where they were.
    pub(crate) fn place_popovers(&mut self) -> Result<bool> {
        let popovers: Vec<usize> = self.popover_nodes().collect();
        // Closed popovers forget their placement; a reopened one is placed afresh.
        let open: std::collections::BTreeSet<u64> = popovers
            .iter()
            .map(|i| stable_id(&self.nodes[*i].element.id))
            .collect();
        self.placed.retain(|id, _| open.contains(id));
        let mut moved = false;
        for i in popovers {
            let element = &self.nodes[i].element;
            let anchor = match (element.attrs.get("anchor"), element.attrs.get("at")) {
                (Some(id), _) => match self.find(id).filter(|a| self.shown(*a)) {
                    Some(a) => self.nodes[a].rect,
                    None => continue,
                },
                (None, Some(at)) => {
                    let mut xy = at.split(',').map(|v| v.trim().parse::<f32>());
                    match (xy.next(), xy.next()) {
                        (Some(Ok(x)), Some(Ok(y))) if x.is_finite() && y.is_finite() => Box2 {
                            x,
                            y,
                            w: 0.0,
                            h: 0.0,
                        },
                        _ => {
                            return Err(format!("ARA4046: popover `at` must be `x,y`: {at}").into());
                        }
                    }
                }
                (None, None) => continue,
            };
            let placement = element
                .attrs
                .get("placement")
                .map(|p| p.parse::<Placement>())
                .transpose()?
                .unwrap_or_default();
            let offset = element
                .attrs
                .get("offset")
                .map(|o| o.parse::<f32>())
                .transpose()?
                .unwrap_or(OFFSET);
            let content = self.natural_size(i)?;
            let placed = place(anchor, content, self.size, placement, offset);
            let applied = Applied {
                origin: placed.origin,
                clamp: [
                    (placed.size[0] + 0.5 < content[0]).then_some(placed.size[0]),
                    (placed.size[1] + 0.5 < content[1]).then_some(placed.size[1]),
                ],
            };
            let id = stable_id(&self.nodes[i].element.id);
            let node = &mut self.nodes[i];
            let current = self.tree.style(node.layout_id)?;
            // From the CSS, so an earlier clamp does not stick once there is room.
            let mut style = node.computed.layout.clone();
            applied.apply(&mut style);
            // A rebuilt node starts from its CSS; compare the style, not just the cache.
            if self.placed.get(&id) == Some(&applied) && style == *current {
                continue;
            }
            self.placed.insert(id, applied);
            self.tree.set_style(node.layout_id, style)?;
            node.dirty.layout = true;
            moved = true;
        }
        Ok(moved)
    }
    /// A popover's content size: its CSS width and height where it sets them (a fixed
    /// palette), otherwise its laid-out width (it sizes to content) and the bottom of its
    /// lowest child, which keeps its natural height in a block container even when the
    /// popover is clamped.
    fn natural_size(&self, index: usize) -> Result<[f32; 2]> {
        let node = &self.nodes[index];
        let css = &node.computed.layout.size;
        let natural = self.content_size(index)?;
        Ok([
            css.width.into_option().unwrap_or(natural[0]),
            css.height.into_option().unwrap_or(natural[1]),
        ])
    }
    /// The laid-out width and the bottom of the lowest child (see `natural_size`).
    fn content_size(&self, index: usize) -> Result<[f32; 2]> {
        let node = &self.nodes[index];
        let layout = self.tree.layout(node.layout_id)?;
        let mut size = [layout.size.width, 0.0_f32];
        for child in &node.element.children {
            let l = self.tree.layout(self.nodes[*child].layout_id)?;
            size[0] = size[0]
                .max(l.location.x + l.size.width + layout.padding.right + layout.border.right);
            size[1] = size[1].max(l.location.y + l.size.height + l.margin.bottom);
        }
        size[1] += layout.padding.bottom + layout.border.bottom;
        if node.element.children.is_empty() {
            size[1] = layout.size.height;
        }
        Ok(size)
    }
}

/// What a click outside the open popovers dismisses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dismissal {
    /// Authored IDs of the popovers to close, topmost first.
    pub close: Vec<String>,
    /// The click landed on the top popover's anchor: hosts that toggle a popover from
    /// its anchor (a bar menu title, a dropdown) close it and should not reopen it.
    pub on_anchor: bool,
}

/// Open-popover bookkeeping shared by every host popover: stacking, dismissal and the
/// focus each one returns to when it closes. Call [`Popovers::sync`] after mounting new
/// markup; it notices popovers that opened (remembering the focused control and moving
/// focus to an `autofocus` child) and closed (restoring that focus).
#[derive(Default)]
pub struct Popovers {
    open: Vec<Open>,
}

struct Open {
    id: String,
    /// The control focused when the popover opened.
    focus: Option<u64>,
}

impl Popovers {
    /// Track popovers opened and closed since the last call and move focus accordingly:
    /// an opened popover takes focus on its first text field (a filter or search box),
    /// and a closed one gives focus back to the control that had it, when that control
    /// can still take it.
    pub fn sync(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<()> {
        let shown: Vec<(usize, String)> = ui
            .popover_nodes()
            .map(|i| (i, ui.nodes[i].element.id.clone()))
            .collect();
        // Closed: restore the focus each one took, innermost first.
        while let Some(top) = self.open.last() {
            if shown.iter().any(|(_, id)| *id == top.id) {
                break;
            }
            let focus = top.focus;
            self.open.pop();
            // Best effort: a control that has since been hidden, clipped or scoped out
            // (another modal) simply keeps no focus.
            if let Some(focus) = focus
                && ui.by_id(focus).is_some()
            {
                let _ = input.focus(ui, focus);
            }
        }
        // Any other closed popover (below an open one) just leaves the stack.
        self.open
            .retain(|o| shown.iter().any(|(_, id)| *id == o.id));
        for (index, id) in shown {
            if self.open.iter().any(|o| o.id == id) {
                continue;
            }
            self.open.push(Open {
                id,
                focus: input.focused(),
            });
            // Best effort too: a field scrolled out of a clamped popover is not focusable.
            if let Some(field) = first_field(ui, index) {
                let _ = input.focus(ui, field);
            }
        }
        Ok(())
    }
    /// The open popovers' authored IDs, bottom to top.
    pub fn open(&self) -> impl Iterator<Item = &str> {
        self.open.iter().map(|o| o.id.as_str())
    }
    /// Whether any popover is open.
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
    /// The topmost open popover, which Escape closes.
    pub fn top(&self) -> Option<&str> {
        self.open.last().map(|o| o.id.as_str())
    }
    /// What a pointer press at `point` dismisses: every popover above the one it lands
    /// in (a click in a menu closes its submenus), or all of them when it lands in none.
    pub fn pointer_down(&self, ui: &Ui, point: [f32; 2]) -> Dismissal {
        let inside = self.open.iter().rposition(|o| {
            ui.find(&o.id)
                .is_some_and(|i| ui.shown(i) && ui.nodes[i].rect.contains(point[0], point[1]))
        });
        let from = inside.map_or(0, |i| i + 1);
        let on_anchor = inside.is_none()
            && self.open.last().is_some_and(|top| {
                ui.find(&top.id)
                    .and_then(|i| ui.nodes[i].element.attrs.get("anchor"))
                    .and_then(|a| ui.find(a))
                    .is_some_and(|a| ui.nodes[a].rect.contains(point[0], point[1]))
            });
        Dismissal {
            close: self.open[from..]
                .iter()
                .rev()
                .map(|o| o.id.clone())
                .collect(),
            on_anchor,
        }
    }
    /// Whether `point` is inside any open popover.
    pub fn contains(&self, ui: &Ui, point: [f32; 2]) -> bool {
        self.open.iter().any(|o| {
            ui.find(&o.id)
                .is_some_and(|i| ui.shown(i) && ui.nodes[i].rect.contains(point[0], point[1]))
        })
    }
}

/// The first text field inside a popover, in document order.
fn first_field(ui: &Ui, index: usize) -> Option<u64> {
    let mut stack = vec![index];
    while let Some(i) = stack.pop() {
        let node = &ui.nodes[i];
        if matches!(node.element.tag.as_str(), "field" | "input") && ui.shown(i) {
            return Some(stable_id(&node.element.id));
        }
        stack.extend(node.element.children.iter().rev());
    }
    None
}

#[cfg(test)]
mod tests;
