//! Bounded editable collection windows shared by native Inspector and game controls.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Stable item identity. Keyed lists and maps use their model key rather than a pool slot.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKey {
    /// A positional list with no model identity.
    Index(usize),
    /// A keyed item's ID or a map entry's key.
    Key(String),
}

/// The visible row window and spacers needed to retain the whole scrollable extent.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// Model indices to mount, including a small overscan on both sides.
    pub range: Range<usize>,
    /// Empty space above the first mounted row, in logical pixels.
    pub before: f32,
    /// Empty space below the last mounted row, in logical pixels.
    pub after: f32,
    /// Full collection extent, including unmounted rows.
    pub extent: f32,
    /// Validated scroll offset, clamped to the current collection length.
    pub offset: f32,
}

/// Fixed-height item headers; hosts mount a selected item's typed details separately.
/// Model length is unbounded by this primitive. Mounted row count depends only on
/// viewport height and overscan, so an editor can reach item 100 or item 100,000.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    row_height: f32,
    height: f32,
    overscan: usize,
}

impl Viewport {
    /// Construct an editable surface in logical pixels; two overscan rows prevent gaps.
    pub fn new(row_height: f32, height: f32) -> Result<Self> {
        if !row_height.is_finite() || row_height <= 0. || !height.is_finite() || height <= 0. {
            return Err(
                "ARA4041: collection row height and viewport must be finite and positive".into(),
            );
        }
        Ok(Self {
            row_height,
            height,
            overscan: 2,
        })
    }

    /// Logical height of the collection's own scroll surface.
    pub fn height(self) -> f32 {
        self.height
    }

    /// Compute the small range to mount after a model notification or scroll event.
    pub fn window(self, len: usize, offset: f32) -> Window {
        let extent = len as f32 * self.row_height;
        let maximum = (extent - self.height).max(0.);
        let offset = if offset.is_finite() {
            offset.clamp(0., maximum)
        } else {
            0.
        };
        let first = (offset / self.row_height).floor() as usize;
        let count = (self.height / self.row_height).ceil() as usize;
        let start = first.saturating_sub(self.overscan).min(len);
        let end = first
            .saturating_add(count)
            .saturating_add(self.overscan)
            .min(len);
        Window {
            range: start..end,
            before: start as f32 * self.row_height,
            after: len.saturating_sub(end) as f32 * self.row_height,
            extent,
            offset,
        }
    }

    /// Nearest offset that reveals a requested index, for search and keyboard navigation.
    pub fn reveal(self, index: usize, len: usize, offset: f32) -> Option<f32> {
        if index >= len {
            return None;
        }
        let offset = self.window(len, offset).offset;
        let top = index as f32 * self.row_height;
        let bottom = top + self.row_height;
        let target = if top < offset {
            top
        } else if bottom > offset + self.height {
            bottom - self.height
        } else {
            offset
        };
        Some(self.window(len, target).offset)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn item_one_hundred_and_last_item_are_reachable_with_a_bounded_pool() {
        let viewport = Viewport::new(24., 240.).unwrap();
        for len in [101, 10_000, 1_000_000] {
            for index in [100, len - 1] {
                let offset = viewport.reveal(index, len, 0.).unwrap();
                let window = viewport.window(len, offset);
                assert!(window.range.contains(&index));
                assert!(window.range.len() <= 14);
                assert_eq!(
                    window.before + window.range.len() as f32 * 24. + window.after,
                    window.extent
                );
            }
        }
    }

    #[test]
    fn shrink_and_nonfinite_scroll_never_produce_invalid_ranges() {
        let viewport = Viewport::new(24., 240.).unwrap();
        for offset in [f32::NAN, f32::INFINITY, -100., 1e30] {
            let window = viewport.window(3, offset);
            assert_eq!(window.range, 0..3);
            assert_eq!(window.offset, 0.);
            assert!(window.before.is_finite() && window.after.is_finite());
        }
        assert_eq!(viewport.window(0, 0.).range, 0..0);
        assert_eq!(viewport.reveal(3, 3, 0.), None);
    }
}
