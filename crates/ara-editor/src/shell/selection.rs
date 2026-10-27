//! Hierarchy and Assets adapters over the shared stable-key selection model.
use super::*;
use ara_ui::selection::{Modifiers, Navigation};
use ara_ui::{paint::Draw, source::stable_id};
use std::collections::HashSet;

fn modifiers(value: [bool; 3]) -> Modifiers {
    Modifiers {
        extend: value[0],
        additive: value[1],
    }
}

fn navigation(key: i32, page: usize) -> Option<Navigation> {
    match key {
        81 => Some(Navigation::Next),
        82 => Some(Navigation::Previous),
        74 => Some(Navigation::First),
        77 => Some(Navigation::Last),
        75 => Some(Navigation::PreviousPage(page)),
        78 => Some(Navigation::NextPage(page)),
        _ => None,
    }
}

impl Shell {
    fn hierarchy_selection_order(&mut self, editor: &Editor) -> ara_ui::Result<Vec<u64>> {
        let live: HashSet<_> = self
            .resolved(editor)?
            .keys()
            .map(|id| crate::world::render_id(id))
            .collect();
        let order: Vec<_> = self
            .hierarchy
            .items
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| live.contains(key))
            .collect();
        self.hierarchy_selection.set_selected(
            editor
                .state
                .selection
                .iter()
                .map(|id| crate::world::render_id(id)),
        );
        self.hierarchy_selection
            .reconcile(&order, |key| live.contains(key));
        Ok(order)
    }

    /// Pointer ranges use the complete filtered tree order, including unmounted
    /// rows. Disclosure/eye/lock and reference-picking routes run before this helper.
    pub(super) fn select_hierarchy_item(
        &mut self,
        editor: &mut Editor,
        entity: &str,
    ) -> ara_ui::Result<()> {
        let order = self.hierarchy_selection_order(editor)?;
        self.hierarchy_selection.select(
            &order,
            &crate::world::render_id(entity),
            modifiers(self.modifiers),
        );
        self.commit_hierarchy_selection(editor)
    }

    fn commit_hierarchy_selection(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let selected = self.hierarchy_selection.selected();
        let mut entities = Vec::with_capacity(selected.len());
        let mut seen = HashSet::with_capacity(selected.len());
        if let Some(entity) = self
            .hierarchy_selection
            .active()
            .filter(|key| selected.contains(key))
            .and_then(|key| self.hierarchy_entities.get(key))
        {
            seen.insert(crate::world::render_id(entity));
            entities.push(entity.clone());
        }
        for (key, _) in &self.hierarchy.items {
            if selected.contains(key)
                && let Some(entity) = self.hierarchy_entities.get(key)
                && seen.insert(*key)
            {
                entities.push(entity.clone());
            }
        }
        // Filtering does not silently deselect valid entities outside the projection.
        for entity in &editor.state.selection {
            let key = crate::world::render_id(entity);
            if selected.contains(&key) && seen.insert(key) {
                entities.push(entity.clone());
            }
        }
        editor.action("editor.select", json!({"entities":entities}))?;
        Ok(())
    }

    /// Full-model keyboard selection and reveal, independent of row-pool slots.
    pub(super) fn hierarchy_selection_key(
        &mut self,
        editor: &mut Editor,
        key: i32,
    ) -> ara_ui::Result<bool> {
        if self.hierarchy_surface != self.current {
            return Ok(false);
        }
        let focused = match self.input.focused().and_then(|id| self.bindings.get(&id)) {
            Some(Binding::Select(entity)) => crate::world::render_id(entity),
            _ => return Ok(false),
        };
        let Some(container) = self.shown_node("hierarchy-rows") else {
            return Ok(false);
        };
        let Some(view) = &self.hierarchy_view else {
            return Ok(false);
        };
        let page = (self.ui.nodes[container].rect.h / view.row_height())
            .floor()
            .max(1.) as usize;
        let Some(navigation) = navigation(key, page) else {
            return Ok(false);
        };
        let order = self.hierarchy_selection_order(editor)?;
        if self.hierarchy_selection.active().is_none() {
            self.hierarchy_selection.focus(focused);
        }
        let intent = modifiers(self.modifiers);
        let Some(target) = self
            .hierarchy_selection
            .navigate(&order, navigation, intent)
        else {
            return Ok(true);
        };
        if !intent.additive || intent.extend {
            self.commit_hierarchy_selection(editor)?;
            self.rebuild(editor)?;
        }
        self.reveal_hierarchy_selection(target)?;
        Ok(true)
    }

    fn reveal_hierarchy_selection(&mut self, target: u64) -> ara_ui::Result<()> {
        let Some(next) = self
            .hierarchy
            .items
            .iter()
            .position(|(key, _)| *key == target)
        else {
            return Ok(());
        };
        let Some(container) = self.shown_node("hierarchy-rows") else {
            return Ok(());
        };
        let Some(view) = &self.hierarchy_view else {
            return Ok(());
        };
        let row = view.row_height();
        let height = self.ui.nodes[container].rect.h;
        let top = next as f32 * row;
        let offset = if top < self.hierarchy_offset {
            top
        } else {
            (top + row - height).max(self.hierarchy_offset)
        };
        self.ui.scroll_to(container, [0., offset])?;
        self.sync_scroll_views()?;
        self.ui.frame(&mut self.text)?;
        if let Some(view) = &self.hierarchy_view {
            self.input.focus(&mut self.ui, view.identity(target))?;
        }
        Ok(())
    }

    pub(super) fn asset_selection_order(&mut self, editor: &Editor) -> Vec<u64> {
        self.observe_asset_history(editor);
        if self.asset_selection_path != editor.state.asset_path {
            self.asset_selection.set_selected(
                editor
                    .state
                    .asset_path
                    .iter()
                    .map(|path| ara_ui::source::stable_id(path)),
            );
            self.asset_selection_path
                .clone_from(&editor.state.asset_path);
        }
        let live: HashSet<_> = self
            .browser
            .items
            .iter()
            .map(|asset| ara_ui::source::stable_id(&asset.path))
            .chain(self.asset_ops.preserved_keys())
            .collect();
        let order: Vec<_> = self
            .asset_rows
            .items
            .iter()
            .map(|(key, _, _)| *key)
            .collect();
        self.asset_selection.reconcile(&order, |key| {
            live.contains(key) || self.asset_entries.contains_key(key)
        });
        order
    }

    /// Select asset files with the same range/toggle semantics as Hierarchy. The
    /// active file still goes through the typed workspace command for Inspector.
    pub(super) fn select_asset_item(
        &mut self,
        editor: &mut Editor,
        asset: &assets::Asset,
    ) -> ara_ui::Result<()> {
        if asset.kind == "folder" {
            let now = std::time::Instant::now();
            let double = self
                .asset_folder_click
                .as_ref()
                .is_some_and(|(previous, at)| {
                    previous == &asset.path && now.duration_since(*at).as_millis() < 400
                });
            self.asset_folder_click = Some((asset.path.clone(), now));
            if double {
                self.asset_folder_click = None;
                editor.action("editor.workspace", json!({"folder":asset.path}))?;
                return Ok(());
            }
        }
        let order = self.asset_selection_order(editor);
        self.asset_selection.select(
            &order,
            &ara_ui::source::stable_id(&asset.path),
            modifiers(self.modifiers),
        );
        self.commit_asset_selection(editor)
    }

    fn commit_asset_selection(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let selected = self.asset_selection.selected();
        let primary = self
            .asset_selection
            .active()
            .filter(|key| selected.contains(key))
            .copied()
            .or_else(|| {
                self.asset_rows
                    .items
                    .iter()
                    .find(|(key, _, _)| selected.contains(key))
                    .map(|(key, _, _)| *key)
            });
        let path = primary
            .and_then(|key| {
                self.asset_entries.get(&key).or_else(|| {
                    self.browser
                        .items
                        .iter()
                        .find(|asset| ara_ui::source::stable_id(&asset.path) == key)
                })
            })
            .map(|asset| asset.path.clone());
        editor.action(
            "editor.workspace",
            json!({"asset":path.as_deref().unwrap_or("")}),
        )?;
        self.asset_selection_path = path;
        self.asset_grid_dirty = true;
        Ok(())
    }

    /// Keyboard ranges over all listed assets, including unmounted list/grid rows.
    pub(super) fn asset_selection_key(
        &mut self,
        editor: &mut Editor,
        key: i32,
    ) -> ara_ui::Result<bool> {
        let focused = match self.input.focused().and_then(|id| self.bindings.get(&id)) {
            Some(Binding::Asset(asset)) => ara_ui::source::stable_id(&asset.path),
            _ => return Ok(false),
        };
        let container = self
            .shown_node("asset-rows")
            .or_else(|| self.shown_node("asset-grid"));
        let Some(container) = container else {
            return Ok(false);
        };
        let grid = editor
            .state
            .asset_grid
            .then(|| self.asset_grid_selection_stride())
            .flatten();
        let row = grid
            .map(|(_, row)| row)
            .or_else(|| self.asset_view.as_ref().map(|view| view.row_height()))
            .unwrap_or(22.);
        let page = (self.ui.nodes[container].rect.h / row).floor().max(1.) as usize;
        let movement = if let Some((columns, _)) = grid {
            match key {
                79 => Some(Navigation::Next),
                80 => Some(Navigation::Previous),
                81 => Some(Navigation::NextPage(columns)),
                82 => Some(Navigation::PreviousPage(columns)),
                75 => Some(Navigation::PreviousPage(page.saturating_mul(columns))),
                78 => Some(Navigation::NextPage(page.saturating_mul(columns))),
                _ => navigation(key, page),
            }
        } else {
            navigation(key, page)
        };
        let Some(navigation) = movement else {
            return Ok(false);
        };
        let order = self.asset_selection_order(editor);
        if self.asset_selection.active().is_none() {
            self.asset_selection.focus(focused);
        }
        let intent = modifiers(self.modifiers);
        let Some(target) = self.asset_selection.navigate(&order, navigation, intent) else {
            return Ok(true);
        };
        if !intent.additive || intent.extend {
            self.commit_asset_selection(editor)?;
        }
        self.reveal_asset_selection(editor, target)?;
        Ok(true)
    }

    fn asset_grid_selection_stride(&self) -> Option<(usize, f32)> {
        let card = self
            .ui
            .nodes
            .iter()
            .find(|node| {
                node.element.tag == "button"
                    && node
                        .element
                        .id
                        .strip_prefix("asset-card-")
                        .is_some_and(|n| n.parse::<usize>().is_ok())
            })?
            .rect;
        // Read card size from the mounted grid so compact and full-size tiles use
        // their real row stride; the grid's shared 8 px gutters separate cards.
        let gap = 8.;
        let columns = ((self.asset_grid_size[0] - 12. - gap) / (card.w + gap))
            .floor()
            .max(1.) as usize;
        Some((columns, card.h + gap))
    }

    pub(super) fn reveal_asset_selection(
        &mut self,
        editor: &mut Editor,
        target: u64,
    ) -> ara_ui::Result<()> {
        let Some(next) = self
            .asset_rows
            .items
            .iter()
            .position(|(key, _, _)| *key == target)
        else {
            return Ok(());
        };
        let (container, top, row) = if !editor.state.asset_grid {
            let Some(container) = self.shown_node("asset-rows") else {
                return Ok(());
            };
            let Some(view) = &self.asset_view else {
                return Ok(());
            };
            let row = view.row_height();
            (container, next as f32 * row, row)
        } else {
            let Some(container) = self.shown_node("asset-grid") else {
                return Ok(());
            };
            let Some((columns, row)) = self.asset_grid_selection_stride() else {
                return Ok(());
            };
            (container, (next / columns) as f32 * row, row)
        };
        let height = self.ui.nodes[container].rect.h;
        let offset = if top < self.asset_offset {
            top
        } else {
            (top + row - height).max(self.asset_offset)
        };
        self.ui.scroll_to(container, [0., offset])?;
        self.sync_scroll_views()?;
        self.rebuild(editor)?;
        if editor.state.asset_grid {
            let id = format!("asset-card-{next}");
            if let Some(index) = self.shown_node(&id) {
                let id = ara_ui::source::stable_id(&self.ui.nodes[index].element.id);
                self.input.focus(&mut self.ui, id)?;
            }
        } else if let Some(view) = &self.asset_view {
            self.ui.frame(&mut self.text)?;
            self.input.focus(&mut self.ui, view.identity(target))?;
        }
        Ok(())
    }

    /// Escape restores a live range, otherwise deselects the focused collection.
    pub(super) fn selection_escape(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let (asset, target) = match self.input.focused().and_then(|id| self.bindings.get(&id)) {
            Some(Binding::Select(_)) => {
                self.hierarchy_selection_order(editor)?;
                if !self.hierarchy_selection.cancel_range() {
                    self.hierarchy_selection.clear();
                }
                self.commit_hierarchy_selection(editor)?;
                (false, self.hierarchy_selection.active().copied())
            }
            Some(Binding::Asset(_)) => {
                self.asset_selection_order(editor);
                if !self.asset_selection.cancel_range() {
                    self.asset_selection.clear();
                }
                self.commit_asset_selection(editor)?;
                (true, self.asset_selection.active().copied())
            }
            _ => return Ok(false),
        };
        self.rebuild(editor)?;
        if let Some(target) = target {
            if asset {
                self.reveal_asset_selection(editor, target)?;
            } else {
                self.reveal_hierarchy_selection(target)?;
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

impl Shell {
    /// Retain a pointer-only selection rectangle on the surface where it began.
    pub(crate) fn set_marquee(&mut self, rect: Option<Box2>) {
        let next = rect.map(|rect| (self.current.clone(), rect));
        if self.marquee != next {
            self.marquee = next;
            if let Some(root) = self.ui.nodes.first_mut() {
                root.dirty.paint = true;
            }
        }
    }

    pub(super) fn paint_marquee(&mut self) {
        let Some((surface, rect)) = &self.marquee else {
            return;
        };
        if *surface != self.current {
            return;
        }
        let color = self
            .ui
            .sheet
            .color_token("--primary")
            .unwrap_or(ara_ui::style::Color(52, 107, 241, 255));
        let mut fill = color;
        fill.3 = 38;
        let node = stable_id("scene-marquee");
        self.ui.display.commands.push(Draw::Rect {
            node,
            rect: *rect,
            color: fill,
        });
        self.ui.display.commands.push(Draw::Border {
            node: node ^ 1,
            rect: *rect,
            radius: 0.,
            color,
            width: 1.,
        });
    }
}
