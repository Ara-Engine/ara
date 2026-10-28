//! Reserve one readable status card before an otherwise flexible rail scrolls.
use crate::{Result, Ui};

impl Ui {
    /// Feed measured first-card height into a scroll's runtime minimum. The source
    /// limits remain authoritative, so dismissing/reflowing cards releases old fits.
    pub(super) fn fit_banner_rails(&mut self) -> Result<bool> {
        let mut changed = false;
        for index in 0..self.nodes.len() {
            let node = &self.nodes[index];
            if node.element.tag != "scroll" {
                continue;
            }
            let authored = node.computed.layout.min_size.height;
            let minimum = authored
                .resolve_to_option(self.size[1], |_, _| 0.0)
                .unwrap_or(0.0);
            let maximum = node
                .computed
                .layout
                .max_size
                .height
                .resolve_to_option(self.size[1], |_, _| 0.0)
                .unwrap_or(f32::MAX)
                .max(0.0);
            let height = self.first_banner_bottom(index)?;
            let desired = match height {
                Some(height) => {
                    taffy::LengthPercentageAuto::length(height.max(minimum).min(maximum))
                }
                None => authored,
            };
            let current = self.tree.style(node.layout_id)?;
            if current.min_size.height == desired {
                continue;
            }
            let mut style = current.clone();
            style.min_size.height = desired;
            self.tree.set_style(self.nodes[index].layout_id, style)?;
            self.nodes[index].dirty.layout = true;
            changed = true;
        }
        Ok(changed)
    }

    /// Measured bottom of the first in-flow banner, including wrapper insets and
    /// spacing before the next card. Nested scrolls own their own first-card fit.
    fn first_banner_bottom(&self, index: usize) -> Result<Option<f32>> {
        let node = &self.nodes[index];
        if !self.shown(index) {
            return Ok(None);
        }
        let layout = self.tree.layout(node.layout_id)?;
        if node.element.tag == "banner" {
            return Ok(Some(layout.size.height));
        }
        for (position, child) in node.element.children.iter().copied().enumerate() {
            let child_node = &self.nodes[child];
            if child_node.element.tag == "scroll"
                || child_node.computed.layout.position == taffy::Position::Absolute
                || !self.shown(child)
            {
                continue;
            }
            let Some(bottom) = self.first_banner_bottom(child)? else {
                continue;
            };
            let child_layout = self.tree.layout(child_node.layout_id)?;
            let followed = node.element.children[position + 1..]
                .iter()
                .copied()
                .any(|next| {
                    self.shown(next)
                        && self.nodes[next].computed.layout.position != taffy::Position::Absolute
                });
            let gap = if followed
                && node.computed.layout.display == taffy::Display::Flex
                && node.computed.layout.flex_direction == taffy::FlexDirection::Column
            {
                taffy::LengthPercentageAuto::from(node.computed.layout.gap.height)
                    .resolve_to_option(layout.size.height, |_, _| 0.0)
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            return Ok(Some(
                child_layout.location.y
                    + bottom
                    + child_layout.margin.bottom
                    + gap
                    + layout.padding.bottom
                    + layout.border.bottom,
            ));
        }
        Ok(None)
    }
}
