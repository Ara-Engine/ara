//! Scroll geometry shared by painting, pointer input, virtual views and focus reveal.
use crate::{
    Result, Ui,
    paint::{Box2, Draw},
    source::stable_id,
    widgets::Widget,
};

const GUTTER: f32 = 12.;
/// Geometry of one overflowing scroll axis, in logical viewport coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Scrollbar {
    /// Zero for horizontal, one for vertical.
    pub axis: usize,
    /// Full clickable track (larger than the quiet thumb).
    pub track: Box2,
    /// Draggable thumb, with a minimum hit size.
    pub thumb: Box2,
    /// Full content extent minus the viewport extent.
    pub maximum: f32,
}
impl Ui {
    /// Current retained scroll position, including virtual view offsets.
    pub fn scroll_position(&self, index: usize) -> Option<[f32; 2]> {
        match self
            .widgets
            .get(&stable_id(&self.nodes.get(index)?.element.id))
        {
            Some(Widget::Scroll(p)) => Some(*p),
            _ => None,
        }
    }
    /// Full content size, including a virtual model's unmounted rows.
    pub fn scroll_extent(&self, index: usize) -> Result<[f32; 2]> {
        let node = self.nodes.get(index).ok_or("ARA4044: not a scroll view")?;
        if let Some(extent) = node.virtual_extent {
            return Ok(extent);
        }
        let viewport = self.tree.layout(node.layout_id)?;
        let mut extent = [viewport.size.width, viewport.size.height];
        for child in &node.element.children {
            let l = self.tree.layout(self.nodes[*child].layout_id)?;
            extent[0] = extent[0].max(l.location.x + l.size.width + viewport.padding.right);
            extent[1] = extent[1].max(l.location.y + l.size.height + viewport.padding.bottom);
        }
        Ok(extent)
    }
    /// Publish a virtual viewport's model size; rows remain bounded to its visible pool.
    /// The pool owns vertical translation; the widget applies horizontal scrolling.
    pub fn set_virtual_extent(&mut self, index: usize, extent: [f32; 2]) -> Result<()> {
        if extent.iter().any(|n| !n.is_finite() || *n < 0.) || self.scroll_position(index).is_none()
        {
            return Err("ARA4044: invalid virtual scroll extent".into());
        }
        if self.nodes[index].virtual_extent != Some(extent) {
            self.nodes[index].virtual_extent = Some(extent);
            self.nodes[index].dirty.paint = true;
        }
        self.scroll_to(
            index,
            self.scroll_position(index)
                .expect("virtual extent requires a scroll widget"),
        )
    }
    /// Visible scrollbars. No geometry is created when content fits.
    pub fn scrollbars(&self, index: usize) -> Result<Vec<Scrollbar>> {
        let Some(position) = self.scroll_position(index) else {
            return Ok(Vec::new());
        };
        let n = &self.nodes[index];
        let extent = self.scroll_extent(index)?;
        let viewport = [n.rect.w, n.rect.h];
        let overflow = [extent[0] > viewport[0] + 0.5, extent[1] > viewport[1] + 0.5];
        let mut bars = Vec::with_capacity(2);
        for axis in 0..2 {
            if !overflow[axis] || viewport[axis] <= 0. {
                continue;
            }
            let length = (viewport[axis] - if overflow[1 - axis] { GUTTER } else { 0. }).max(0.);
            let thumb_length = (length * viewport[axis] / extent[axis])
                .max(24.)
                .min(length);
            let maximum = (extent[axis] - viewport[axis]).max(0.);
            let offset = position[axis].clamp(0., maximum) / maximum * (length - thumb_length);
            let (track, thumb) = if axis == 1 {
                let track = Box2 {
                    x: n.rect.x + n.rect.w - GUTTER,
                    y: n.rect.y,
                    w: GUTTER,
                    h: length,
                };
                (
                    track,
                    Box2 {
                        y: track.y + offset,
                        h: thumb_length,
                        ..track
                    },
                )
            } else {
                let track = Box2 {
                    x: n.rect.x,
                    y: n.rect.y + n.rect.h - GUTTER,
                    w: length,
                    h: GUTTER,
                };
                (
                    track,
                    Box2 {
                        x: track.x + offset,
                        w: thumb_length,
                        ..track
                    },
                )
            };
            bars.push(Scrollbar {
                axis,
                track,
                thumb,
                maximum,
            });
        }
        Ok(bars)
    }
    /// Hit the topmost visible scrollbar, respecting overlays and ancestor clips.
    pub fn scrollbar_at(&self, p: [f32; 2]) -> Option<(usize, Scrollbar)> {
        let mut i = self.paint_order.iter().copied().rev().find(|i| {
            let n = &self.nodes[*i];
            n.rect.contains(p[0], p[1])
                && n.clip.contains(p[0], p[1])
                && self.rounded_hit(*i, p[0], p[1])
        });
        let mut hit = None;
        while let Some(index) = i {
            if let Some(bar) = self
                .scrollbars(index)
                .ok()?
                .into_iter()
                .find(|b| b.track.contains(p[0], p[1]))
            {
                // Ancestor bars paint after their children, so win overlapping tracks.
                hit = Some((index, bar));
            }
            i = self.nodes[index].element.parent;
        }
        hit
    }
    pub(crate) fn pointer_at(&mut self, p: [f32; 2]) {
        let previous = self
            .pointer
            .and_then(|p| self.scrollbar_at(p))
            .map(|(i, _)| i);
        let next = self.scrollbar_at(p).map(|(i, _)| i);
        if previous != next {
            for index in [previous, next].into_iter().flatten() {
                self.nodes[index].dirty.paint = true;
            }
        }
        self.pointer = Some(p);
    }
    pub(crate) fn paint_scrollbars(&mut self, index: usize, translation: [f32; 2]) -> Result<()> {
        let id = stable_id(&self.nodes[index].element.id);
        let hovered = self.nodes[index].state.active
            || self
                .pointer
                .and_then(|p| self.scrollbar_at(p))
                .is_some_and(|(i, _)| i == index);
        let color = self
            .sheet
            .color_token("--muted-foreground")
            .unwrap_or(self.nodes[index].computed.color);
        for bar in self.scrollbars(index)? {
            let mut rect = bar.thumb;
            let inset = if hovered { 2. } else { 4. };
            if bar.axis == 1 {
                rect.x += inset;
                rect.w -= inset * 2.;
            } else {
                rect.y += inset;
                rect.h -= inset * 2.;
            }
            rect.x -= translation[0];
            rect.y -= translation[1];
            self.display.commands.push(Draw::RoundedRect {
                node: id,
                rect,
                radius: 4.,
                color: crate::style::Color(
                    color.0,
                    color.1,
                    color.2,
                    if hovered { 210 } else { 120 },
                ),
            });
        }
        Ok(())
    }
    /// Reveal a retained focused row/field in every containing scroll view, with minimal movement.
    pub fn scroll_into_view(&mut self, index: usize) -> Result<()> {
        let mut target = self
            .nodes
            .get(index)
            .ok_or("ARA4044: invalid focus target")?
            .rect;
        let mut ancestor = self.nodes[index].element.parent;
        while let Some(i) = ancestor {
            if let Some(position) = self.scroll_position(i) {
                let r = self.nodes[i].rect;
                let dx = if target.x <= r.x && target.x + target.w >= r.x + r.w {
                    0.
                } else if target.x < r.x {
                    target.x - r.x
                } else {
                    (target.x + target.w - r.x - r.w).max(0.)
                };
                let dy = if target.y <= r.y && target.y + target.h >= r.y + r.h {
                    0.
                } else if target.y < r.y {
                    target.y - r.y
                } else {
                    (target.y + target.h - r.y - r.h).max(0.)
                };
                self.scroll_to(i, [position[0] + dx, position[1] + dy])?;
                if let Some(next) = self.scroll_position(i) {
                    target.x -= next[0] - position[0];
                    target.y -= next[1] - position[1];
                }
            }
            ancestor = self.nodes[i].element.parent;
        }
        Ok(())
    }
    pub(crate) fn clamp_scrolls(&mut self) -> Result<()> {
        for i in 0..self.nodes.len() {
            if matches!(
                self.widgets.get(&stable_id(&self.nodes[i].element.id)),
                Some(Widget::Scroll(_))
            ) && self.shown(i)
            {
                let p = self.scroll_position(i).expect("scroll widget checked");
                self.scroll_to(i, p)?;
            }
        }
        Ok(())
    }
}
