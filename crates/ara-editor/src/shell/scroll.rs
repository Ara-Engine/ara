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
    pub(super) fn sync_scroll_views(&mut self) -> ara_ui::Result<()> {
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
    pub(super) fn hierarchy_key(&mut self, key: i32) -> ara_ui::Result<bool> {
        if !matches!(key, 81 | 82 | 74 | 77 | 75 | 78) {
            return Ok(false);
        }
        let Some(view) = &self.hierarchy_view else {
            return Ok(false);
        };
        let Some(focused) = self.input.focused() else {
            return Ok(false);
        };
        let Some(current) = self
            .ui
            .by_id(focused)
            .and_then(|node| view.visible_nodes().iter().position(|i| *i == node))
        else {
            return Ok(false);
        };
        let Some(i) = self.shown_node("hierarchy-rows") else {
            return Ok(false);
        };
        let height = self.ui.nodes[i].rect.h;
        let row = view.row_height();
        let current = (self.hierarchy_offset / row).floor() as usize + current;
        let page = (height / row).floor().max(1.) as usize;
        let last = self.hierarchy.items.len().saturating_sub(1);
        let next = match key {
            81 => (current + 1).min(last),
            82 => current.saturating_sub(1),
            74 => 0,
            77 => last,
            75 => current.saturating_sub(page),
            _ => (current + page).min(last),
        };
        let top = next as f32 * row;
        let offset = if top < self.hierarchy_offset {
            top
        } else {
            (top + row - height).max(self.hierarchy_offset)
        };
        self.ui.scroll_to(i, [0., offset])?;
        self.sync_scroll_views()?;
        self.ui.frame(&mut self.text)?;
        let view = self
            .hierarchy_view
            .as_ref()
            .expect("hierarchy pool survives scroll");
        self.input
            .focus(&mut self.ui, view.identity(self.hierarchy.items[next].0))?;
        Ok(true)
    }
}
