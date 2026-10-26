//! Scrollbar capture is independent of row focus and ordinary control activation.
use super::*;
pub(super) struct ScrollDrag {
    pub(super) node: u64,
    pub(super) axis: usize,
    pub(super) grab: Option<f32>,
}
impl InputState {
    pub(super) fn scroll_event(&mut self, ui: &mut Ui, event: &InputEvent) -> Result<bool> {
        match event {
            InputEvent::PointerDown(p) => {
                if let Some((i, bar)) = ui.scrollbar_at(*p).filter(|(i, _)| self.in_scope(ui, *i)) {
                    let id = stable_id(&ui.nodes[i].element.id);
                    let axis = bar.axis;
                    if bar.thumb.contains(p[0], p[1]) {
                        let start = if axis == 0 { bar.thumb.x } else { bar.thumb.y };
                        self.scroll_drag = Some(ScrollDrag {
                            node: id,
                            axis,
                            grab: Some(p[axis] - start),
                        });
                    } else {
                        let mut target = ui
                            .scroll_position(i)
                            .expect("scrollbar has a scroll widget");
                        let start = if axis == 0 { bar.thumb.x } else { bar.thumb.y };
                        let page = if axis == 0 {
                            ui.nodes[i].rect.w
                        } else {
                            ui.nodes[i].rect.h
                        };
                        target[axis] += if p[axis] < start { -page } else { page };
                        ui.scroll_to(i, target)?;
                        // Capture track clicks as well: releasing cannot activate a row beneath it.
                        self.scroll_drag = Some(ScrollDrag {
                            node: id,
                            axis,
                            grab: None,
                        });
                    }
                    self.capture = None;
                    self.capture_rect = None;
                    if let Some(hover) = self.hover.take().and_then(|id| ui.by_id(id)) {
                        let mut state = ui.nodes[hover].state;
                        state.hover = false;
                        ui.set_state(hover, state);
                    }
                    let mut state = ui.nodes[i].state;
                    state.active = true;
                    ui.set_state(i, state);
                    return Ok(true);
                }
            }
            InputEvent::PointerMove(p) => {
                if let Some(drag) = &self.scroll_drag {
                    if let Some(i) = ui.by_id(drag.node)
                        && let Some(grab) = drag.grab
                        && let Some(bar) =
                            ui.scrollbars(i)?.into_iter().find(|b| b.axis == drag.axis)
                    {
                        let (start, length, thumb) = if drag.axis == 0 {
                            (bar.track.x, bar.track.w, bar.thumb.w)
                        } else {
                            (bar.track.y, bar.track.h, bar.thumb.h)
                        };
                        let mut target = ui
                            .scroll_position(i)
                            .expect("scrollbar has a scroll widget");
                        target[drag.axis] =
                            (p[drag.axis] - start - grab) / (length - thumb).max(1.) * bar.maximum;
                        ui.scroll_to(i, target)?;
                    }
                    return Ok(true);
                }
            }
            InputEvent::PointerUp(_) | InputEvent::Cancel => {
                if let Some(drag) = self.scroll_drag.take() {
                    if let Some(i) = ui.by_id(drag.node) {
                        let mut state = ui.nodes[i].state;
                        state.active = false;
                        ui.set_state(i, state);
                    }
                    return Ok(true);
                }
            }
            _ => {}
        }
        Ok(false)
    }
}
