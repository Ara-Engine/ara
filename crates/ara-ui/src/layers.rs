//! Popup/menu/tooltip/modal composition over the same retained tree and input scopes.
use crate::{Result, Ui, input::InputState, source::stable_id};
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
        if matches!(event, crate::input::InputEvent::Cancel) && !self.layers.is_empty() {
            self.close(ui, input)?;
            return Ok(());
        }
        if let crate::input::InputEvent::PointerDown(p) = &event
            && let Some(layer) = self.layers.last()
            && layer.kind == LayerKind::Popup
            && ui
                .by_id(layer.node)
                .is_some_and(|i| !ui.nodes[i].rect.contains(p[0], p[1]))
        {
            self.close(ui, input)?;
            return Ok(());
        }
        input.event(ui, event, clipboard)
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
            return Err("ARA7046: layer already open or invalid position".into());
        }
        let i = ui.by_id(node).ok_or("ARA7046: layer node missing")?;
        if ui.nodes[i].element.parent != Some(0) {
            return Err("ARA7046: overlays must be mounted under the UI root".into());
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
            input.pop(ui);
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
    pub fn set_visible(&mut self, index: usize, visible: bool) -> Result<()> {
        let n = self
            .nodes
            .get_mut(index)
            .ok_or("ARA7046: invalid node handle")?;
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
            return Err("ARA7040: invalid viewport".into());
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
        self.nodes
            .iter()
            .find(|n| stable_id(&n.element.id) == id)
            .map(|n| n.element.id.as_str())
    }
}
