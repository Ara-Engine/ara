//! Measured, non-interactive drag feedback; inline status messages use `<banner>`.
use crate::{Result, Ui, paint::Box2, source::stable_id};

/// Empty and short feedback retains a useful hit-independent visual extent.
pub(crate) const MIN_WIDTH: f32 = 48.0;
const GAP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub point: [f32; 2],
    pub avoid: Option<Box2>,
}

impl Ui {
    /// Queue a root-child `<feedback>` beside this surface's logical pointer.
    /// Call after changing its text and before [`Self::frame`]. The frame measures
    /// and wraps its text to at most 360 logical pixels, then moves the whole subtree
    /// inside the viewport. `avoid` is the hovered row's full viewport rectangle:
    /// feedback prefers its right, then left, before covering rows below or above.
    /// Text children inherit this surface's font/color and always wrap normally.
    /// Feedback never captures input or participates in popup focus/dismissal.
    pub fn place_feedback(
        &mut self,
        index: usize,
        point: [f32; 2],
        avoid: Option<Box2>,
    ) -> Result<()> {
        let node = self
            .nodes
            .get_mut(index)
            .ok_or("ARA4046: invalid feedback handle")?;
        if node.element.tag != "feedback" || node.element.parent != Some(0) {
            return Err("ARA4046: feedback must be a root-child feedback primitive".into());
        }
        if !point.into_iter().all(f32::is_finite) || avoid.is_some_and(|rect| !valid(rect)) {
            return Err("ARA4046: feedback geometry must be finite and non-negative".into());
        }
        let id = stable_id(&node.element.id);
        let anchor = Anchor { point, avoid };
        if self.feedback.get(&id) != Some(&anchor) {
            self.feedback.insert(id, anchor);
            self.placed.remove(&id);
            node.dirty.paint = true;
        }
        Ok(())
    }

    /// Position one measured subtree before paint; stable-label pointer motion
    /// never invalidates Taffy or Parley. Layout changes settle before this runs.
    pub(crate) fn place_feedback_nodes(&mut self) -> bool {
        let mut changed = false;
        for position in 0..self.nodes[0].element.children.len() {
            let index = self.nodes[0].element.children[position];
            if self.nodes[index].element.tag != "feedback" || !self.shown(index) {
                continue;
            }
            let id = stable_id(&self.nodes[index].element.id);
            let Some(anchor) = self.feedback.get(&id).copied() else {
                continue;
            };
            let node = &mut self.nodes[index];
            let placed = place(anchor, [node.rect.w, node.rect.h], self.size);
            let offset = [
                placed.x - (node.rect.x - node.paint_offset[0]),
                placed.y - (node.rect.y - node.paint_offset[1]),
            ];
            if node.paint_offset != offset {
                node.paint_offset = offset;
                changed = true;
            }
        }
        changed
    }
}

fn valid(rect: Box2) -> bool {
    [rect.x, rect.y, rect.w, rect.h]
        .into_iter()
        .all(f32::is_finite)
        && rect.w >= 0.0
        && rect.h >= 0.0
}

/// Use measured geometry only; text remeasurement remains in the frame pipeline.
pub(crate) fn place(anchor: Anchor, content: [f32; 2], window: [f32; 2]) -> Box2 {
    let margin = crate::popover::MARGIN.min(window[0].min(window[1]) * 0.5);
    let w = content[0]
        .min(crate::popover::MAX_WIDTH)
        .min((window[0] - 2.0 * margin).max(0.0));
    let h = content[1].min((window[1] - 2.0 * margin).max(0.0));
    let x = |value: f32| value.clamp(margin, (window[0] - margin - w).max(margin));
    let y = |value: f32| value.clamp(margin, (window[1] - margin - h).max(margin));
    let [px, py] = anchor.point;
    let rect = |x, y| Box2 { x, y, w, h };
    if let Some(avoid) = anchor.avoid {
        let right = avoid.x + avoid.w + GAP;
        if right + w <= window[0] - margin {
            return rect(right.max(margin), y(py + 18.0));
        }
        let left = avoid.x - GAP - w;
        if left >= margin {
            return rect(left, y(py + 18.0));
        }
        let below = avoid.y + avoid.h + GAP;
        if below + h <= window[1] - margin {
            return rect(x(px + 14.0), below.max(margin));
        }
        let above = avoid.y - GAP - h;
        if above >= margin {
            return rect(x(px + 14.0), above);
        }
    }
    // Point-only hints choose the roomier vertical side and flip horizontally
    // before the final clamp, keeping the pointer clear near viewport corners.
    let right = px + 14.0;
    let left = px - 14.0 - w;
    let below = py + 18.0;
    let above = py - 18.0 - h;
    let origin_x = if right + w <= window[0] - margin {
        right
    } else {
        left
    };
    let origin_y = if below + h <= window[1] - margin {
        below
    } else {
        above
    };
    rect(x(origin_x), y(origin_y))
}

#[cfg(test)]
mod tests;
