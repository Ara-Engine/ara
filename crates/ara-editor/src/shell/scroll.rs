//! Keep bounded row pools synchronized with the scroll widget's authoritative offset.
use super::*;
impl Shell {
    /// Compatibility route for scripted wheel interactions; scroll widgets own offsets.
    pub fn scroll_hierarchy(&mut self, delta: f32) -> ara_ui::Result<bool> {
        for id in ["hierarchy-rows", "asset-rows", "asset-grid"] {
            if let Some(i) = self.shown_node(id) {
                let r = self.ui.nodes[i].rect;
                if r.contains(self.pointer[0], self.pointer[1]) {
                    if let Some(p) = self.ui.scroll_position(i) {
                        self.ui.scroll_to(i, [p[0], p[1] + delta])?;
                        if id == "asset-grid"
                            && let Some(next) = self.ui.scroll_position(i)
                            && self.asset_offset != next[1]
                        {
                            self.asset_offset = next[1];
                            self.asset_grid_dirty = true;
                        }
                        self.sync_scroll_views()?;
                    } else if id == "asset-grid" {
                        // The grid's owner migrates its manual card pool to <scroll>.
                        self.asset_offset = (self.asset_offset + delta)
                            .clamp(0., (self.asset_grid_extent - r.h).max(0.));
                        self.asset_grid_dirty = true;
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    /// The asset grid's card window follows its scroll view's offset (its cards are
    /// pooled by the Assets panel, which rebuilds them when the offset moves).
    pub(super) fn sync_asset_grid_scroll(&mut self) {
        if let Some(grid) = self.shown_node("asset-grid")
            && let Some([_, offset]) = self.ui.scroll_position(grid)
            && (offset - self.asset_offset).abs() > 0.01
        {
            self.asset_offset = offset;
            self.asset_grid_dirty = true;
        }
    }
    /// Bring the bounded row pools (Hierarchy, asset list and grid) to their scroll
    /// views' offsets.
    pub(super) fn sync_scroll_views(&mut self) -> ara_ui::Result<()> {
        self.sync_asset_grid_scroll();
        if self.hierarchy_surface == self.current
            && let Some(i) = self.shown_node("hierarchy-rows")
            && let Some(p) = self.ui.scroll_position(i)
            && p[1] != self.hierarchy_offset
            && let Some(view) = &mut self.hierarchy_view
        {
            self.hierarchy_offset = p[1];
            let height = self.ui.nodes[i].rect.h;
            view.scroll(&mut self.ui, &self.hierarchy, p[1], height)?;
            self.bind_hierarchy_rows();
            self.decorate_hierarchy();
        }
        if self.asset_surface == self.current
            && let Some(i) = self.shown_node("asset-rows")
            && let Some(p) = self.ui.scroll_position(i)
            && p[1] != self.asset_offset
            && let Some(view) = &mut self.asset_view
        {
            self.asset_offset = p[1];
            let height = self.ui.nodes[i].rect.h;
            view.scroll(&mut self.ui, &self.asset_rows, p[1], height)?;
            self.bind_asset_rows();
        }
        Ok(())
    }
    /// Keyboard navigation over the full Hierarchy model, materializing only the target viewport.
    pub(super) fn hierarchy_key(&mut self, editor: &mut Editor, key: i32) -> ara_ui::Result<bool> {
        self.hierarchy_selection_key(editor, key)
    }
}
