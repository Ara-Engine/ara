//! Native drag sources. The shared kit owns gesture state; commands own every committed edit.
use super::*;
use ara_ui::source::stable_id;
#[path = "drag_drop.rs"]
mod content;

pub(crate) enum Drag {
    Content(Box<content::ContentDrag>),
    Tab {
        panel: String,
        start: [f32; 2],
        /// The client surface owning `start`, independent of the current drop surface.
        surface: Option<String>,
    },
    /// Resizing the inspector's label column.
    Column,
}
impl Shell {
    /// Capture an entity, asset, component section or dock tab as a typed drag payload.
    pub fn begin_drag(&mut self) {
        let [x, y] = self.pointer;
        if let Some(panel) = self
            .ui
            .nodes
            .iter()
            .rev()
            .find(|n| {
                n.visible
                    && n.element
                        .attrs
                        .get("class")
                        .is_some_and(|c| c == "panel-header")
                    && n.rect.contains(x, y)
            })
            .and_then(|n| n.element.id.strip_prefix("header-"))
        {
            self.drag = Some(Drag::Tab {
                panel: panel.into(),
                start: self.pointer,
                surface: self.current.clone(),
            });
            return;
        }
        let Some(index) = self.ui.hit_test(x, y) else {
            return;
        };
        let id = &self.ui.nodes[index].element.id;
        if let Some(panel) = id.strip_prefix("tab-") {
            self.drag = Some(Drag::Tab {
                panel: panel.into(),
                start: self.pointer,
                surface: self.current.clone(),
            });
            return;
        }
        let payload = match self.bindings.get(&stable_id(id)) {
            Some(Binding::Select(entity)) => Some(content::Payload::Entity(entity.clone())),
            Some(Binding::Asset(asset)) => Some(content::Payload::Asset(asset.clone())),
            Some(Binding::ComponentCollapse(component)) => {
                Some(content::Payload::Component(component.clone()))
            }
            Some(Binding::Collection(action))
                if matches!(action.intent, collection_adapter::Intent::Select(_)) =>
            {
                Some(content::Payload::Collection(action.clone()))
            }
            _ => None,
        };
        self.drag = payload
            .and_then(|payload| content::ContentDrag::begin(payload, self))
            .map(Box::new)
            .map(Drag::Content);
    }
    /// Resolve a release against the current model, then commit one atomic authoring batch.
    pub fn drop_at(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.content_pending() {
            return self.finish_content_drag(editor);
        }
        match self.drag.take() {
            Some(Drag::Column) => Ok(true),
            Some(Drag::Tab { panel, .. }) => {
                let Some(preview) = self.drop_preview.take() else {
                    return Ok(false);
                };
                self.drop_tab(editor, &panel, &preview.target, None)?;
                self.rebuild(editor)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
