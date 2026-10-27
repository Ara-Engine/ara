//! Numeric field grips preview native text and emit one finite value on release.
use super::*;
use crate::{paint::Box2, source::Element, widgets::Widget};
use taffy::NodeId;

pub(crate) const GRIP: f32 = 16.;
const THRESHOLD: f64 = 6.;

/// A field opts into scrubbing with `class='number'` or numeric bounds/step.
pub(crate) fn declared(element: &Element) -> bool {
    matches!(element.tag.as_str(), "field" | "input")
        && (element.attrs.get("class").is_some_and(|classes| {
            classes
                .split_ascii_whitespace()
                .any(|class| class == "number")
        }) || ["min", "max", "step"]
            .iter()
            .any(|key| element.attrs.contains_key(*key)))
}

pub(crate) fn grip(rect: Box2, point: [f32; 2]) -> bool {
    rect.contains(point[0], point[1]) && point[0] >= rect.x + (rect.w - GRIP).max(0.)
}

#[derive(Clone, Copy, PartialEq)]
struct Config {
    min: f64,
    max: f64,
    step: f64,
    precision: usize,
}

impl Config {
    fn read(element: &Element) -> Result<Self> {
        let number = |key: &str, default: f64| -> Result<f64> {
            let value = element
                .attrs
                .get(key)
                .map_or(Ok(default), |s| s.parse::<f64>())?;
            if !value.is_finite() {
                return Err("ARA4044: numeric field bounds and step must be finite".into());
            }
            Ok(value)
        };
        let config = Self {
            min: number("min", -f64::MAX)?,
            max: number("max", f64::MAX)?,
            step: number("step", 1.)?,
            precision: ["text", "min", "max", "step"]
                .into_iter()
                .filter_map(|key| element.attrs.get(key))
                .map(|text| decimal_places(text))
                .max()
                .unwrap_or(0),
        };
        if config.min > config.max || config.step <= 0. {
            return Err("ARA4044: numeric field requires min <= max and a positive step".into());
        }
        Ok(config)
    }
}

pub(super) struct NumericDrag {
    node: u64,
    layout: NodeId,
    start: [f32; 2],
    initial: f64,
    original: String,
    config: Config,
    value: f64,
    active: bool,
}

impl InputState {
    /// A numeric field's right-side grip owns capture, including its drag threshold.
    /// Fields opt in with `class='number'` or `min`/`max`/`step` attributes. Their
    /// ordinary text area retains caret editing. Release emits one numeric action;
    /// Escape, focus loss or removal restores the uncommitted preview.
    pub fn scrubbing(&self) -> bool {
        self.numeric_drag.is_some()
    }

    pub(super) fn cancel_numeric(&mut self, ui: &mut Ui) {
        let Some(drag) = self.numeric_drag.take() else {
            return;
        };
        if self.capture == Some(drag.node) {
            self.capture = None;
            self.capture_rect = None;
        }
        if let Some(i) = ui.by_id(drag.node) {
            let text = match ui.widgets.get(&drag.node) {
                Some(Widget::Input(input)) => Some(input.display()),
                _ => None,
            };
            if let Some(text) = text
                && ui.nodes[i].text.is_some()
            {
                // The ID resolves to a retained text-input node with a text layout.
                ui.set_display_text(i, &text)
                    .expect("retained text input has its text layout");
            }
            let mut state = ui.nodes[i].state;
            state.active = false;
            ui.set_state(i, state);
        }
    }

    pub(super) fn numeric_event(
        &mut self,
        ui: &mut Ui,
        event: &InputEvent,
        clipboard: &mut dyn Clipboard,
    ) -> Result<bool> {
        if let Some(drag) = &self.numeric_drag {
            let valid = ui.by_id(drag.node).is_some_and(|i| {
                self.can_focus(ui, i) && self.focus == Some(drag.node)
                    && ui.nodes[i].text.is_some()
                    && ui.nodes[i].layout_id == drag.layout
                    && declared(&ui.nodes[i].element)
                    && Config::read(&ui.nodes[i].element).is_ok_and(|config| config.min == drag.config.min && config.max == drag.config.max && config.step == drag.config.step)
                    && matches!(ui.widgets.get(&drag.node), Some(Widget::Input(input)) if input.value() == drag.original)
            });
            if !valid {
                self.cancel_numeric(ui);
            }
        }
        match event {
            InputEvent::PointerDown(point) => {
                self.cancel_numeric(ui);
                if ui.scrollbar_at(*point).is_some() {
                    return Ok(false);
                }
                let Some(i) = ui
                    .hit_test(point[0], point[1])
                    .filter(|i| self.can_focus(ui, *i))
                else {
                    return Ok(false);
                };
                if !declared(&ui.nodes[i].element) || !grip(ui.nodes[i].rect, *point) {
                    return Ok(false);
                }
                let id = stable_id(&ui.nodes[i].element.id);
                let Some(Widget::Input(input)) = ui.widgets.get(&id) else {
                    return Ok(false);
                };
                let Ok(initial) = input.value().parse::<f64>() else {
                    return Ok(false);
                };
                if !initial.is_finite() || input.preedit_range().is_some() {
                    return Ok(false);
                }
                let original = input.value().to_owned();
                let mut config = Config::read(&ui.nodes[i].element)?;
                config.precision = config.precision.max(decimal_places(&original));
                self.focus_to(ui, Some(id));
                self.capture = Some(id);
                self.capture_rect = Some(ui.nodes[i].rect);
                let mut state = ui.nodes[i].state;
                state.active = true;
                ui.set_state(i, state);
                self.numeric_drag = Some(NumericDrag {
                    node: id,
                    layout: ui.nodes[i].layout_id,
                    start: *point,
                    initial,
                    original,
                    config,
                    value: initial,
                    active: false,
                });
                Ok(true)
            }
            InputEvent::PointerMove(point) => {
                let Some(drag) = &mut self.numeric_drag else {
                    return Ok(false);
                };
                let dx = f64::from(point[0]) - f64::from(drag.start[0]);
                if dx.abs() >= THRESHOLD {
                    drag.active = true;
                }
                if drag.active {
                    let distance = (dx.abs() - THRESHOLD).max(0.) * dx.signum();
                    let value = (drag.initial + distance.round() * drag.config.step)
                        .clamp(drag.config.min, drag.config.max);
                    let value = if value == 0. { 0. } else { value };
                    let text = format!("{value:.precision$}", precision = drag.config.precision);
                    drag.value = text
                        .parse::<f64>()
                        .unwrap_or(value)
                        .clamp(drag.config.min, drag.config.max);
                    if let Some(i) = ui.by_id(drag.node) {
                        ui.set_display_text(i, &text)?;
                    }
                }
                Ok(true)
            }
            InputEvent::PointerUp(point) => {
                if self.numeric_drag.is_some() {
                    self.numeric_event(ui, &InputEvent::PointerMove(*point), clipboard)?;
                }
                let Some(drag) = self.numeric_drag.take() else {
                    return Ok(false);
                };
                self.capture = None;
                self.capture_rect = None;
                if let Some(i) = ui.by_id(drag.node) {
                    let mut state = ui.nodes[i].state;
                    state.active = false;
                    ui.set_state(i, state);
                    if drag.active && drag.value != drag.initial {
                        let text = format!(
                            "{:.precision$}",
                            drag.value,
                            precision = drag.config.precision
                        );
                        if let Some(Widget::Input(input)) = ui.widgets.get_mut(&drag.node) {
                            input.event(EditEvent::SelectAll, clipboard)?;
                            input.event(EditEvent::Insert(text), clipboard)?;
                            let text = input.display();
                            ui.set_display_text(i, &text)?;
                            ui.emit(i, serde_json::json!(drag.value));
                        }
                    } else {
                        ui.set_display_text(i, &drag.original)?;
                    }
                }
                Ok(true)
            }
            InputEvent::Cancel if self.numeric_drag.is_some() => {
                self.cancel_numeric(ui);
                Ok(true)
            }
            InputEvent::Tab { .. }
            | InputEvent::Navigate(_)
            | InputEvent::Edit(_)
            | InputEvent::Activate => {
                self.cancel_numeric(ui);
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}

fn decimal_places(text: &str) -> usize {
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or((text, 0), |(m, e)| (m, e.parse::<i32>().unwrap_or(0)));
    let fraction = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len() as i64);
    (fraction - i64::from(exponent)).clamp(0, 15) as usize
}

#[cfg(test)]
mod tests;
