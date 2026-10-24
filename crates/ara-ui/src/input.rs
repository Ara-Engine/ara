//! CommonUI-style scopes, stable focus, pointer capture and native input routing.
use crate::focus::directional_where;
/// Keyboard/spatial navigation and inspection graph helpers.
pub use crate::focus::{directional, focus_graph, next_tab, tab_order};
use crate::{Result, Ui, runtime::is_focusable, source::stable_id};
use ara_ui_text::{Clipboard, EditEvent};
/// Gamepad/keyboard spatial direction.
#[derive(Clone, Copy, Debug)]
pub enum Direction {
    /// Logical left.
    Left,
    /// Logical right.
    Right,
    /// Logical up.
    Up,
    /// Logical down.
    Down,
}
impl Direction {
    pub(crate) fn attribute(self) -> &'static str {
        match self {
            Self::Left => "nav-left",
            Self::Right => "nav-right",
            Self::Up => "nav-up",
            Self::Down => "nav-down",
        }
    }
}
/// Platform-neutral UI events. SDL3/OS IME translation belongs to the host.
pub enum InputEvent {
    /// Pointer hover or captured drag position, in logical viewport pixels.
    PointerMove([f32; 2]),
    /// Press primary pointer; captures the hit control until release/cancel.
    PointerDown([f32; 2]),
    /// Release primary pointer and activate if released over the captured control.
    PointerUp([f32; 2]),
    /// Cycle focus within the active screen/modal scope.
    Tab {
        /// Reverse document order.
        reverse: bool,
    },
    /// Gamepad directional focus movement.
    Navigate(Direction),
    /// Activate the focused control.
    Activate,
    /// Cancel capture, close the top layer or pop the active screen.
    Cancel,
    /// Deliver a keyboard/IME/clipboard edit to the focused field.
    Edit(EditEvent),
    /// Scroll the nearest scroll ancestor under the pointer.
    Wheel {
        /// Pointer viewport position.
        position: [f32; 2],
        /// Logical-pixel delta.
        delta: [f32; 2],
    },
}
struct Scope {
    root: u64,
    focus: Option<u64>,
    modal: bool,
}
/// Active screen/modal stack, pointer capture and drag state, owned per player/window.
#[derive(Default)]
pub struct InputState {
    scopes: Vec<Scope>,
    focus: Option<u64>,
    hover: Option<u64>,
    capture: Option<u64>,
    /// Where the captured control was when pressed: a release there still activates
    /// it after a rebuild or slow frame moved it (press/release match by stable ID).
    capture_rect: Option<crate::paint::Box2>,
    drag: Option<(u64, serde_json::Value)>,
}
impl InputState {
    /// Stable identity of the focused control.
    pub fn focused(&self) -> Option<u64> {
        self.focus
    }
    /// Release keyboard focus when a native host gives input to a scene/world surface.
    pub fn blur(&mut self, ui: &mut Ui) {
        self.focus_to(ui, None);
    }
    /// Current pointer-capture identity.
    pub fn captured(&self) -> Option<u64> {
        self.capture
    }
    /// Inspect navigation within the active screen/modal, using the same scope rules as input.
    pub fn focus_graph(&self, ui: &Ui) -> serde_json::Value {
        let order: Vec<_> = tab_order(ui)
            .into_iter()
            .filter(|i| self.in_scope(ui, *i))
            .collect();
        serde_json::json!(ui.paint_order.iter().filter(|i| is_focusable(&ui.nodes[**i]) && self.in_scope(ui, **i)).map(|i| {
            let id = |j: Option<usize>| j.map(|j| ui.nodes[j].element.id.as_str());
            let neighbor = |direction| id(directional_where(ui, *i, direction, |j| self.in_scope(ui, j)));
            serde_json::json!({
                "id": ui.nodes[*i].element.id,
                "tab": id(order.iter().position(|j| j == i).and_then(|position| order.get((position + 1) % order.len()).copied())),
                "left": neighbor(Direction::Left), "right": neighbor(Direction::Right),
                "up": neighbor(Direction::Up), "down": neighbor(Direction::Down),
            })
        }).collect::<Vec<_>>())
    }
    fn in_scope(&self, ui: &Ui, index: usize) -> bool {
        self.scopes.last().is_none_or(|scope| {
            let mut i = Some(index);
            while let Some(j) = i {
                if stable_id(&ui.nodes[j].element.id) == scope.root {
                    return true;
                }
                i = ui.nodes[j].element.parent;
            }
            false
        })
    }
    /// Push a screen or modal, remembering the previous scope's focus by stable identity.
    pub fn push(
        &mut self,
        ui: &mut Ui,
        root: u64,
        initial: Option<u64>,
        modal: bool,
    ) -> Result<()> {
        if ui.by_id(root).is_none() {
            return Err("ARA4045: input scope root not found".into());
        }
        if let Some(s) = self.scopes.last_mut() {
            s.focus = self.focus;
        }
        self.capture = None;
        self.scopes.push(Scope {
            root,
            focus: initial,
            modal,
        });
        let initial = initial
            .filter(|id| {
                ui.by_id(*id).is_some_and(|i| {
                    self.in_scope(ui, i)
                        && !ui.nodes[i].state.disabled
                        && matches!(
                            ui.nodes[i].element.tag.as_str(),
                            "button"
                                | "field"
                                | "input"
                                | "toggle"
                                | "slider"
                                | "tab"
                                | "tabs"
                                | "dropdown"
                                | "menu"
                        )
                })
            })
            .or_else(|| {
                ui.paint_order
                    .iter()
                    .copied()
                    .find(|i| self.in_scope(ui, *i) && is_focusable(&ui.nodes[*i]))
                    .map(|i| stable_id(&ui.nodes[i].element.id))
            });
        self.focus_to(ui, initial);
        Ok(())
    }
    /// Pop a screen/modal and restore surviving focus, falling back within its parent scope.
    pub fn pop(&mut self, ui: &mut Ui) -> Option<u64> {
        self.scopes.pop();
        self.capture = None;
        let restored = self
            .scopes
            .last()
            .and_then(|s| s.focus)
            .filter(|id| {
                ui.by_id(*id)
                    .is_some_and(|i| is_focusable(&ui.nodes[i]) && self.in_scope(ui, i))
            })
            .or_else(|| {
                ui.paint_order
                    .iter()
                    .copied()
                    .find(|i| self.in_scope(ui, *i) && is_focusable(&ui.nodes[*i]))
                    .map(|i| stable_id(&ui.nodes[i].element.id))
            });
        self.focus_to(ui, restored);
        restored
    }
    /// Explicitly focus a control in the active scope.
    pub fn focus(&mut self, ui: &mut Ui, id: u64) -> Result<()> {
        if ui
            .by_id(id)
            .is_none_or(|i| !is_focusable(&ui.nodes[i]) || !self.in_scope(ui, i))
        {
            return Err(
                "ARA4045: focus target is disabled, clipped or outside the active scope".into(),
            );
        }
        self.focus_to(ui, Some(id));
        Ok(())
    }
    fn focus_to(&mut self, ui: &mut Ui, id: Option<u64>) {
        if let Some(i) = self.focus.and_then(|id| ui.by_id(id)) {
            let mut s = ui.nodes[i].state;
            s.focus = false;
            ui.set_state(i, s);
        }
        self.focus = id;
        if let Some(i) = id.and_then(|id| ui.by_id(id)) {
            let mut s = ui.nodes[i].state;
            s.focus = true;
            ui.set_state(i, s);
        }
        if let Some(s) = self.scopes.last_mut() {
            s.focus = id;
        }
    }
    /// Begin typed drag/drop from the captured control; no global mutable drag state.
    pub fn begin_drag(&mut self, payload: serde_json::Value) -> Result<()> {
        let source = self
            .capture
            .ok_or("ARA4045: drag requires pointer capture")?;
        self.drag = Some((source, payload));
        Ok(())
    }
    /// Route one input message, preserving hover and focus as independent states.
    pub fn event(
        &mut self,
        ui: &mut Ui,
        event: InputEvent,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        if let Some(id) = self.focus
            && ui
                .by_id(id)
                .is_none_or(|i| !is_focusable(&ui.nodes[i]) || !self.in_scope(ui, i))
        {
            self.focus_to(ui, None);
        }
        let hit = |this: &Self, ui: &Ui, p: [f32; 2]| {
            ui.hit_test(p[0], p[1]).filter(|i| this.in_scope(ui, *i))
        };
        match event {
            InputEvent::PointerMove(p) => {
                let hover = hit(self, ui, p).map(|i| stable_id(&ui.nodes[i].element.id));
                if hover != self.hover {
                    if let Some(i) = self.hover.and_then(|id| ui.by_id(id)) {
                        let mut s = ui.nodes[i].state;
                        s.hover = false;
                        ui.set_state(i, s);
                    }
                    if let Some(i) = hover.and_then(|id| ui.by_id(id)) {
                        let mut s = ui.nodes[i].state;
                        s.hover = true;
                        ui.set_state(i, s);
                    }
                    self.hover = hover;
                }
                if let Some(i) = self.capture.and_then(|id| ui.by_id(id))
                    && matches!(
                        ui.widgets.get(&stable_id(&ui.nodes[i].element.id)),
                        Some(
                            crate::widgets::Widget::Slider { .. }
                                | crate::widgets::Widget::Splitter(_)
                        )
                    )
                {
                    self.drag_slider(ui, i, p)?;
                }
                if let Some(i) = self.capture.and_then(|id| ui.by_id(id)) {
                    ui.select_text_at(i, p, true, clipboard)?;
                }
            }
            InputEvent::PointerDown(p) => {
                if let Some(i) = hit(self, ui, p) {
                    let id = stable_id(&ui.nodes[i].element.id);
                    // An unfocused input is drawn from its start (no caret scroll):
                    // put the caret there so the click lands where the text shows.
                    if !ui.nodes[i].state.focus
                        && matches!(ui.widgets.get(&id), Some(crate::widgets::Widget::Input(_)))
                    {
                        ui.edit(
                            i,
                            EditEvent::Select {
                                anchor: 0,
                                caret: 0,
                            },
                            clipboard,
                        )?;
                    }
                    self.focus_to(ui, Some(id));
                    self.capture = Some(id);
                    self.capture_rect = Some(ui.nodes[i].rect);
                    ui.select_text_at(i, p, false, clipboard)?;
                    let mut state = ui.nodes[i].state;
                    state.active = true;
                    ui.set_state(i, state);
                    self.drag_slider(ui, i, p)?;
                }
            }
            InputEvent::PointerUp(p) => {
                let captured = self.capture.take();
                let pressed_at = self.capture_rect.take();
                if let Some(i) = captured.and_then(|id| ui.by_id(id)) {
                    let mut state = ui.nodes[i].state;
                    state.active = false;
                    ui.set_state(i, state);
                    if let Some((source, payload)) = self.drag.take() {
                        if let Some(target) = hit(self, ui, p) {
                            ui.emit(
                                target,
                                serde_json::json!({"source":source,"payload":payload}),
                            );
                        }
                    } else if (hit(self, ui, p) == Some(i)
                        || pressed_at.is_some_and(|r| r.contains(p[0], p[1]))
                            && is_focusable(&ui.nodes[i])
                            && self.in_scope(ui, i))
                        && !matches!(
                            ui.widgets.get(&stable_id(&ui.nodes[i].element.id)),
                            Some(
                                crate::widgets::Widget::Input(_)
                                    | crate::widgets::Widget::Slider { .. }
                            )
                        )
                    {
                        ui.activate(i)?;
                    }
                }
            }
            InputEvent::Tab { reverse } => {
                let order: Vec<_> = tab_order(ui)
                    .into_iter()
                    .filter(|i| self.in_scope(ui, *i))
                    .collect();
                if !order.is_empty() {
                    let current = self.focus.and_then(|id| {
                        order
                            .iter()
                            .position(|i| stable_id(&ui.nodes[*i].element.id) == id)
                    });
                    let next = current.map_or(if reverse { order.len() - 1 } else { 0 }, |i| {
                        if reverse {
                            (i + order.len() - 1) % order.len()
                        } else {
                            (i + 1) % order.len()
                        }
                    });
                    self.focus_to(ui, Some(stable_id(&ui.nodes[order[next]].element.id)));
                }
            }
            InputEvent::Navigate(direction) => {
                if let Some(i) = self.focus.and_then(|id| ui.by_id(id))
                    && let Some(next) =
                        directional_where(ui, i, direction, |j| self.in_scope(ui, j))
                {
                    self.focus_to(ui, Some(stable_id(&ui.nodes[next].element.id)));
                }
            }
            InputEvent::Activate => {
                if let Some(i) = self.focus.and_then(|id| ui.by_id(id)) {
                    ui.activate(i)?;
                }
            }
            InputEvent::Cancel => {
                if let Some(i) = self.capture.take().and_then(|id| ui.by_id(id)) {
                    let mut s = ui.nodes[i].state;
                    s.active = false;
                    ui.set_state(i, s);
                }
                self.drag = None;
                if self.scopes.last().is_some_and(|s| s.modal) || self.scopes.len() > 1 {
                    self.pop(ui);
                }
            }
            InputEvent::Edit(event) => {
                if let Some(i) = self.focus.and_then(|id| ui.by_id(id)) {
                    if let EditEvent::Move { forward, select } = event {
                        ui.move_caret(i, forward, select, clipboard)?;
                    } else {
                        ui.edit(i, event, clipboard)?;
                    }
                }
            }
            InputEvent::Wheel { position, delta } => {
                let mut i = ui.paint_order.iter().copied().rev().find(|i| {
                    self.in_scope(ui, *i)
                        && ui.nodes[*i].rect.contains(position[0], position[1])
                        && ui.nodes[*i].clip.contains(position[0], position[1])
                });
                while let Some(j) = i {
                    let id = stable_id(&ui.nodes[j].element.id);
                    if let Some(crate::widgets::Widget::Scroll(p)) = ui.widgets.get(&id) {
                        let target = [p[0] + delta[0], p[1] + delta[1]];
                        ui.scroll_to(j, target)?;
                        break;
                    }
                    i = ui.nodes[j].element.parent;
                }
            }
        }
        Ok(())
    }
    fn drag_slider(&self, ui: &mut Ui, i: usize, pointer: [f32; 2]) -> Result<()> {
        let id = stable_id(&ui.nodes[i].element.id);
        if let Some(crate::widgets::Widget::Slider { min, max, .. }) = ui.widgets.get(&id) {
            let r = ui.nodes[i].rect;
            let value =
                min + (max - min) * f64::from(((pointer[0] - r.x) / r.w.max(1.0)).clamp(0.0, 1.0));
            ui.set_value(i, value)?;
            let actual = match ui.widgets.get(&id) {
                Some(crate::widgets::Widget::Slider { value, .. }) => *value,
                _ => value,
            };
            ui.emit(i, serde_json::json!(actual));
        } else if matches!(
            ui.widgets.get(&id),
            Some(crate::widgets::Widget::Splitter(_))
        ) && let Some(parent) = ui.nodes[i].element.parent
        {
            let r = ui.nodes[parent].rect;
            let horizontal = ui.tree.style(ui.nodes[parent].layout_id)?.flex_direction
                == taffy::FlexDirection::Row;
            let ratio = if horizontal {
                (pointer[0] - r.x) / r.w.max(1.0)
            } else {
                (pointer[1] - r.y) / r.h.max(1.0)
            };
            ui.set_value(i, f64::from(ratio))?;
            ui.emit(i, serde_json::json!(ratio.clamp(0.1, 0.9)));
        }
        Ok(())
    }
}
/// World-surface ray mapping shared by screen and diegetic controls.
pub use crate::world::ray_to_local;
