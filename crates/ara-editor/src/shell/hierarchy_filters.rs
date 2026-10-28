//! Explicit reveal and saved searches use typed navigation/preferences commands.
use super::commands::Target;
use super::*;
mod query;
mod saved;
pub(super) use query::{Projection, Query};
pub(crate) use saved::View;
#[cfg(test)]
mod tests;

impl Shell {
    /// Explicitly reveal the targeted selection without changing its members or
    /// camera. Background transactions never invoke this focus/visibility action.
    pub(crate) fn reveal_hierarchy_entities(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let ids = target.entities(&editor.state);
        if ids.is_empty() {
            return Err("Select an entity to reveal in Hierarchy".into());
        }
        let resolved = self.resolved(editor)?;
        let mut ancestors = std::collections::BTreeSet::new();
        for id in &ids {
            if !resolved.contains_key(id) {
                return Err(format!(
                    "Entity `{id}` no longer exists; choose a live entity to reveal"
                )
                .into());
            }
            let mut next = Some(id.as_str());
            while let Some(id) = next {
                if !ancestors.insert(id.to_owned()) {
                    break;
                }
                next = resolved
                    .get(id)
                    .and_then(|entity| entity.parent.as_deref())
                    .filter(|parent| resolved.contains_key(*parent));
            }
        }
        let query = Query::new(
            editor
                .state
                .searches
                .get("hierarchy")
                .map_or("", String::as_str),
        );
        let projection = Projection::new(&resolved, &query);
        if ids.iter().any(|id| !projection.included.contains(id)) {
            editor.action("editor.search", json!({"panel":"hierarchy","query":""}))?;
        }
        for id in ancestors {
            if editor.state.hidden.contains(&id) {
                editor.action("editor.visibility", json!({"entity":id,"enabled":false}))?;
            }
            editor.action("editor.collapse", json!({"entity":id,"enabled":false}))?;
        }
        editor.action("editor.layout.show", json!({"panel":"hierarchy"}))?;
        editor.action("editor.focus", json!({"panel":"hierarchy"}))?;
        self.hierarchy_filters.reveal =
            ids.first().map(|id| (editor.state.doc.clone(), id.clone()));
        self.rebuild(editor)
    }
    /// Poll shared dialog results and materialize an explicitly requested stable
    /// row after any coalesced panel rebuild has completed.
    pub(crate) fn poll_hierarchy_filters(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let changed = self.poll_saved_hierarchy_filters(editor)?;
        if changed {
            self.rebuild(editor)?;
        }
        if self.hierarchy_filters.reveal.is_some() {
            let resolved = self.resolved(editor)?;
            if self
                .hierarchy_filters
                .reveal
                .as_ref()
                .is_some_and(|(doc, entity)| {
                    doc != &editor.state.doc || !resolved.contains_key(entity)
                })
            {
                self.hierarchy_filters.reveal = None;
                self.status_note =
                    Some("The requested hierarchy entity is no longer in the active scene".into());
                return Ok(true);
            }
        }
        Ok(self.reveal_pending_hierarchy_entity()? || changed)
    }
    fn reveal_pending_hierarchy_entity(&mut self) -> ara_ui::Result<bool> {
        let Some((_, entity)) = self.hierarchy_filters.reveal.as_ref() else {
            return Ok(false);
        };
        let key = crate::world::render_id(entity);
        let Some(position) = self.hierarchy.items.iter().position(|(id, _)| *id == key) else {
            return Ok(false);
        };
        if self.hierarchy_view.is_none() {
            return Ok(false);
        }
        let returning = self.current.clone();
        let hosting = self.hierarchy_surface.clone();
        self.leave();
        if let Some(id) = &hosting
            && !self.enter(id)
        {
            if let Some(id) = returning {
                self.enter(&id);
            }
            return Ok(false);
        }
        let result = (|| -> ara_ui::Result<bool> {
            let Some(container) = self.shown_node("hierarchy-rows") else {
                return Ok(false);
            };
            let Some(view) = &self.hierarchy_view else {
                return Ok(false);
            };
            let row = view.row_height();
            let top = position as f32 * row;
            let height = self.ui.nodes[container].rect.h;
            let offset = if top < self.hierarchy_offset {
                top
            } else {
                (top + row - height).max(self.hierarchy_offset)
            };
            self.ui.scroll_to(container, [0., offset])?;
            self.sync_scroll_views()?;
            self.ui.frame(&mut self.text)?;
            if let Some(view) = &self.hierarchy_view {
                self.input.focus(&mut self.ui, view.identity(key))?;
            }
            self.hierarchy_selection.focus(key);
            self.hierarchy_filters.reveal = None;
            if let Some(id) = &hosting {
                self.window_request = Some(WindowRequest::Raise(id.clone()));
            }
            Ok(true)
        })();
        self.leave();
        if let Some(id) = returning {
            self.enter(&id);
        }
        result
    }
}
