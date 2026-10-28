//! Popup/menu/tooltip/modal composition over the same retained tree and input scopes.
use crate::{Result, Ui, input::InputState};
/// Overlay routing policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    /// Does not steal focus or intercept pointer input.
    Tooltip,
    /// Focused dropdown or context menu, dismissed through back/cancel.
    Popup,
    /// Input-trapping dialog with focus restoration on close.
    Modal,
}
struct Layer {
    node: u64,
    kind: LayerKind,
}
/// Ordered layers, mounted as root children so ancestor clips cannot hide them.
#[derive(Default)]
pub struct LayerStack {
    layers: Vec<Layer>,
}
impl LayerStack {
    /// Route overlay input, consuming dismissals and restoring focus when the top layer closes.
    pub fn event(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        event: crate::input::InputEvent,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> Result<()> {
        self.sync(ui, input)?;
        if matches!(event, crate::input::InputEvent::Cancel) && input.scrubbing() {
            return input.event(ui, event, clipboard);
        }
        let interactive = self
            .layers
            .iter()
            .rposition(|l| l.kind != LayerKind::Tooltip);
        if matches!(event, crate::input::InputEvent::Cancel)
            && let Some(index) = interactive
        {
            while self.layers.len() > index {
                self.close(ui, input)?;
            }
            return Ok(());
        }
        if let crate::input::InputEvent::PointerDown(p) = &event
            && let Some(layer) = interactive.and_then(|index| self.layers.get(index))
            && layer.kind == LayerKind::Popup
            && ui
                .by_id(layer.node)
                .is_some_and(|i| !ui.nodes[i].rect.contains(p[0], p[1]))
        {
            if let Some(index) = interactive {
                while self.layers.len() > index {
                    self.close(ui, input)?;
                }
            }
            return Ok(());
        }
        // Enter in a prompt invokes its declared primary action. A focused button
        // keeps its ordinary activation, including the explicit Cancel button.
        if matches!(event, crate::input::InputEvent::Activate)
            && let Some(layer) = interactive.and_then(|index| self.layers.get(index))
            && layer.kind == LayerKind::Modal
            && input
                .focused()
                .and_then(|id| ui.by_id(id))
                .is_none_or(|i| matches!(ui.nodes[i].element.tag.as_str(), "field" | "input"))
            && let Some(action) = ui
                .by_id(layer.node)
                .and_then(|i| ui.nodes[i].element.attrs.get("default-action"))
                .and_then(|id| ui.find(id))
            && ui.focus_candidate(action)
            && input.in_scope(ui, action)
        {
            return ui.activate(action);
        }
        input.event(ui, event, clipboard)
    }
    /// Forget removed/hidden top overlays after host reconciliation and restore the
    /// preceding scope. Stable IDs preserve open scopes when the tree is rebuilt.
    pub fn sync(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<()> {
        while self
            .layers
            .last()
            .is_some_and(|layer| ui.by_id(layer.node).is_none_or(|i| !ui.shown(i)))
        {
            self.close(ui, input)?;
        }
        Ok(())
    }
    /// Open a root-level overlay at logical coordinates and push its input scope when interactive.
    pub fn open(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        node: u64,
        kind: LayerKind,
        position: [f32; 2],
        initial: Option<u64>,
    ) -> Result<()> {
        if self.layers.iter().any(|l| l.node == node) || position.iter().any(|v| !v.is_finite()) {
            return Err("ARA4046: layer already open or invalid position".into());
        }
        let i = ui.by_id(node).ok_or("ARA4046: layer node missing")?;
        if ui.nodes[i].element.parent != Some(0) {
            return Err("ARA4046: overlays must be mounted under the UI root".into());
        }
        ui.set_visible(i, true)?;
        let mut style = ui.tree.style(ui.nodes[i].layout_id)?.clone();
        style.position = taffy::Position::Absolute;
        style.inset.left = taffy::LengthPercentageAuto::length(position[0]);
        style.inset.top = taffy::LengthPercentageAuto::length(position[1]);
        ui.tree.set_style(ui.nodes[i].layout_id, style)?;
        ui.nodes[0].element.children.retain(|c| *c != i);
        ui.nodes[0].element.children.push(i);
        let ids: Vec<_> = ui.nodes[0]
            .element
            .children
            .iter()
            .map(|j| ui.nodes[*j].layout_id)
            .collect();
        ui.tree.set_children(ui.nodes[0].layout_id, &ids)?;
        ui.nodes[0].dirty.layout = true;
        if kind != LayerKind::Tooltip {
            input.push(ui, node, initial, kind == LayerKind::Modal)?;
        }
        self.layers.push(Layer { node, kind });
        Ok(())
    }
    /// Close the top layer and restore the previous focus scope.
    pub fn close(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<Option<u64>> {
        let Some(layer) = self.layers.pop() else {
            return Ok(None);
        };
        if let Some(i) = ui.by_id(layer.node) {
            ui.set_visible(i, false)?;
        }
        if layer.kind != LayerKind::Tooltip {
            input.remove_scope(ui, layer.node);
        }
        Ok(Some(layer.node))
    }
    /// Inspect painter/input order by stable node ID.
    pub fn nodes(&self) -> impl Iterator<Item = u64> + '_ {
        self.layers.iter().map(|l| l.node)
    }
}
impl Ui {
    /// Runtime visibility mutation; authoring changes still go through source commands.
    /// The current visibility again is a no-op.
    pub fn set_visible(&mut self, index: usize, visible: bool) -> Result<()> {
        let n = self
            .nodes
            .get_mut(index)
            .ok_or("ARA4046: invalid node handle")?;
        if n.visible == visible {
            return Ok(());
        }
        n.visible = visible;
        let mut style = self.tree.style(n.layout_id)?.clone();
        style.display = if visible {
            n.computed.layout.display
        } else {
            taffy::Display::None
        };
        self.tree.set_style(n.layout_id, style)?;
        n.dirty.layout = true;
        n.dirty.paint = true;
        Ok(())
    }
    /// Resize/DPI logical viewport update; text is rebroken only when its available width changes.
    pub fn resize(&mut self, size: [f32; 2]) -> Result<()> {
        if size.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err("ARA4040: invalid viewport".into());
        }
        if size != self.size {
            self.size = size;
            self.nodes[0].dirty.layout = true;
            self.tree.mark_dirty(self.nodes[0].layout_id)?;
        }
        Ok(())
    }
    /// Authored ID for host action routing from a stable identity.
    pub fn authored_id(&self, id: u64) -> Option<&str> {
        self.by_id(id).map(|i| self.nodes[i].element.id.as_str())
    }
}

#[cfg(test)]
mod tests;
