//! Semantic helpers for UI tests and scripted verification: walk menus, read
//! rows, filter, drag a tab to an area and open context menus at real controls.
use super::commands::{Avail, menus::MENUS};
use super::menu::Row;
use super::*;

/// A rendered menu row: command id, title as shown, and the disabled reason.
pub type MenuRow = (String, String, Option<String>);
impl Shell {
    /// Open every menu and submenu; returns `(menu path, command id)` for each row,
    /// failing if a rendered row is not bound to the command it shows.
    pub fn walk_menus(&mut self, editor: &mut Editor) -> ara_ui::Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        for (index, menu) in MENUS.iter().enumerate() {
            self.open_bar(index, false);
            self.walk_level(editor, menu.title.to_owned(), &mut out)?;
        }
        self.menu = None;
        self.rebuild(editor)?;
        Ok(out)
    }
    fn walk_level(
        &mut self,
        editor: &mut Editor,
        path: String,
        out: &mut Vec<(String, String)>,
    ) -> ara_ui::Result<()> {
        self.rebuild(editor)?;
        let levels = self.menu_levels(editor);
        let level = levels.len() - 1;
        let rows = levels[level].clone();
        for (i, row) in rows.iter().enumerate() {
            let node = format!("m{level}-{i}");
            match row {
                Row::Command { id, .. } => {
                    match self.bindings.get(&ara_ui::source::stable_id(&node)) {
                        Some(Binding::Command(bound, _)) if bound == id => {}
                        _ => return Err(format!("{path}: row {node} is not bound to {id}").into()),
                    }
                    out.push((path.clone(), (*id).to_owned()));
                }
                Row::Sub(title, _) => {
                    if let Some(menu) = &mut self.menu {
                        menu.open.truncate(level);
                        menu.open.push(i);
                    }
                    self.walk_level(editor, format!("{path} › {title}"), out)?;
                    if let Some(menu) = &mut self.menu {
                        menu.open.truncate(level);
                    }
                }
                Row::Sep => {}
            }
        }
        Ok(())
    }
    /// Rows of a top-level menu as rendered: id, title and disabled reason.
    pub fn menu_rows(&mut self, editor: &mut Editor, index: usize) -> ara_ui::Result<Vec<MenuRow>> {
        self.open_bar(index, false);
        self.rebuild(editor)?;
        let rows = self.rows(editor);
        self.menu = None;
        self.rebuild(editor)?;
        Ok(rows)
    }
    /// IDs of menu bar menu `index`'s nodes (labels, keycaps) that extend past its right edge.
    pub fn menu_overflow(
        &mut self,
        editor: &mut Editor,
        index: usize,
    ) -> ara_ui::Result<Vec<String>> {
        self.open_bar(index, false);
        self.rebuild(editor)?;
        let menu = self.ui.nodes[self.ui.find("menu-pop-0").ok_or("menu not open")?].rect;
        let overflow = self
            .ui
            .nodes
            .iter()
            .filter(|n| {
                n.element.id.starts_with("m0-") && n.rect.x + n.rect.w > menu.x + menu.w + 0.5
            })
            .map(|n| n.element.id.clone())
            .collect();
        self.menu = None;
        self.rebuild(editor)?;
        Ok(overflow)
    }
    /// Rows of a menu filtered by typed text.
    pub fn filter_menu(
        &mut self,
        editor: &mut Editor,
        index: usize,
        query: &str,
    ) -> ara_ui::Result<Vec<MenuRow>> {
        self.open_bar(index, false);
        if let Some(menu) = &mut self.menu {
            menu.query = query.into();
        }
        self.rebuild(editor)?;
        let rows = self.rows(editor);
        self.menu = None;
        self.rebuild(editor)?;
        Ok(rows)
    }
    fn rows(&self, editor: &Editor) -> Vec<MenuRow> {
        self.menu_levels(editor)
            .first()
            .into_iter()
            .flatten()
            .filter_map(|row| match row {
                Row::Command {
                    id, target, label, ..
                } => {
                    let spec = commands::get(id)?;
                    let cx = self.ctx(editor, target);
                    let reason = match spec.avail(&cx) {
                        Avail::Blocked(r) => Some(r),
                        _ => None,
                    };
                    Some((
                        (*id).to_owned(),
                        label.clone().unwrap_or_else(|| spec.title(&cx)),
                        reason,
                    ))
                }
                _ => None,
            })
            .collect()
    }
    /// Drag a panel's tab and drop it on the centre of an area in the main window.
    pub fn drag_tab_to(
        &mut self,
        editor: &mut Editor,
        panel: &str,
        area: &str,
    ) -> ara_ui::Result<()> {
        self.leave();
        let index = self
            .ui
            .find(&format!("area-{area}"))
            .ok_or("area not in the main window")?;
        let r = self.ui.nodes[index].rect;
        self.drag = Some(Drag::Tab {
            panel: panel.into(),
            start: [0., 0.],
        });
        self.pointer = [r.x + r.w * 0.5, r.y + r.h * 0.5];
        self.drag_over();
        if !self.finish_tab_drag(editor, None)? {
            return Err("no drop target under the pointer".into());
        }
        Ok(())
    }
    /// Poll the background asset scan once; rebuilds and returns true when it arrived.
    pub fn poll_assets_for_test(&mut self, editor: &mut Editor) -> bool {
        if self.poll_assets() || !self.browser.items.is_empty() {
            let _ = self.rebuild(editor);
            return !self.browser.items.is_empty();
        }
        false
    }
    /// Palette rows for `query` as (group, title, command id) without opening it.
    pub fn palette_rows(
        &mut self,
        editor: &mut Editor,
        query: &str,
    ) -> Vec<(&'static str, String, Option<&'static str>)> {
        self.palette_items(editor, query)
            .into_iter()
            .map(|item| {
                let command = match item.binding {
                    Binding::Command(id, _) => Some(id),
                    _ => None,
                };
                (item.group, item.title, command)
            })
            .collect()
    }
    /// Open the context menu at a control (or the first hierarchy row) and list its commands.
    pub fn context_rows_at(
        &mut self,
        editor: &mut Editor,
        id: &str,
    ) -> ara_ui::Result<Vec<String>> {
        if !self.open_context_at(editor, id)? {
            return Ok(Vec::new());
        }
        let ids = self.rows(editor).into_iter().map(|r| r.0).collect();
        self.menu = None;
        self.rebuild(editor)?;
        Ok(ids)
    }
    /// Right-click control `id` (`hierarchy-row` is the first visible row) and leave
    /// its context menu open; false when the control has no menu.
    pub fn open_context_at(&mut self, editor: &mut Editor, id: &str) -> ara_ui::Result<bool> {
        let index = if id == "hierarchy-row" {
            self.hierarchy_view
                .as_ref()
                .and_then(|v| v.visible_nodes().first().copied())
        } else {
            self.ui.find(id)
        }
        .ok_or("control not found")?;
        let r = self.ui.nodes[index].rect;
        self.pointer = [r.x + r.w.min(60.) * 0.5, r.y + r.h * 0.5];
        self.open_context(editor)
    }
}
