//! Numeric fields scrub when dragged anywhere and edit as text when clicked.
//! A drag streams each preview value as text (the host's draft protocol), then
//! emits one finite number on release; a click focuses the field with its text
//! selected. Slider fields (`class='slider'` with finite `min`/`max`) also draw
//! their value as a fill and map the field's width to their whole range.
use super::*;
use crate::{paint::Box2, source::Element, widgets::Widget};
use taffy::NodeId;

const THRESHOLD: f64 = 4.;

/// A field opts into scrubbing with `class='number'` or numeric bounds/step.
pub(crate) fn declared(element: &Element) -> bool {
    matches!(element.tag.as_str(), "field" | "input")
        && (has_class(element, "number")
            || ["min", "max", "step"]
                .iter()
                .any(|key| element.attrs.contains_key(*key)))
}

fn has_class(element: &Element, name: &str) -> bool {
    element
        .attrs
        .get("class")
        .is_some_and(|classes| classes.split_ascii_whitespace().any(|class| class == name))
}

/// Pressing anywhere on a numeric field may start a scrub. Releasing without
/// moving edits instead: the caret goes to the click, or the whole value is
/// selected when the field was not being edited yet.
pub(crate) fn scrubs(element: &Element, rect: Box2, point: [f32; 2]) -> bool {
    declared(element) && rect.contains(point[0], point[1])
}

/// How full a slider field is (0–1) for its shown `text`, if it is one.
pub(crate) fn slider_ratio(element: &Element, text: &str) -> Option<f32> {
    if !has_class(element, "slider") {
        return None;
    }
    let config = Config::read(element).ok()?;
    let span = config.max - config.min;
    let value = text.trim().parse::<f64>().ok()?;
    (span.is_finite() && span > 0. && value.is_finite())
        .then(|| ((value - config.min) / span).clamp(0., 1.) as f32)
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
    /// Value units per pixel: one step, or a slider's range over its width.
    rate: f64,
    value: f64,
    active: bool,
    /// The last preview text sent to the host, which may echo it back.
    previewed: Option<String>,
    /// The field was already being edited when pressed.
    editing: bool,
}

impl InputState {
    /// A numeric field being scrubbed owns capture, including its drag threshold.
    /// Fields opt in with `class='number'` or `min`/`max`/`step` attributes.
    /// Release emits one numeric action; Escape, blur or removal restores the
    /// original value (re-sent to the host if a preview was).
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
            if drag.previewed.is_some() {
                ui.emit(i, serde_json::json!(drag.original));
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
                self.can_focus(ui, i) && self.capture == Some(drag.node)
                    && ui.nodes[i].text.is_some()
                    && ui.nodes[i].layout_id == drag.layout
                    && declared(&ui.nodes[i].element)
                    && Config::read(&ui.nodes[i].element).is_ok_and(|config| config.min == drag.config.min && config.max == drag.config.max && config.step == drag.config.step)
                    // The host may echo a preview back as the field's text.
                    && matches!(ui.widgets.get(&drag.node), Some(Widget::Input(input)) if input.value() == drag.original || drag.previewed.as_deref() == Some(input.value()))
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
                let node = &ui.nodes[i];
                if !scrubs(&node.element, node.rect, *point) {
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
                let rate = match slider_ratio(&ui.nodes[i].element, &original) {
                    Some(_) => (config.max - config.min) / f64::from(ui.nodes[i].rect.w.max(1.)),
                    None => config.step,
                };
                // Another field being edited lets go; this one focuses only on a click.
                let editing = self.focus == Some(id);
                if !editing {
                    self.focus_to(ui, None);
                }
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
                    rate,
                    value: initial,
                    active: false,
                    previewed: None,
                    editing,
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
                    let steps = (distance * drag.rate / drag.config.step).round();
                    let value = (drag.initial + steps * drag.config.step)
                        .clamp(drag.config.min, drag.config.max);
                    let value = if value == 0. { 0. } else { value };
                    let text = format!("{value:.precision$}", precision = drag.config.precision);
                    drag.value = text
                        .parse::<f64>()
                        .unwrap_or(value)
                        .clamp(drag.config.min, drag.config.max);
                    if let Some(i) = ui.by_id(drag.node) {
                        ui.set_display_text(i, &text)?;
                        // Hosts preview text drafts live; release commits a number.
                        if drag.previewed.as_deref() != Some(text.as_str()) {
                            ui.emit(i, serde_json::json!(text));
                            drag.previewed = Some(text);
                        }
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
                let Some(i) = ui.by_id(drag.node) else {
                    return Ok(true);
                };
                let mut state = ui.nodes[i].state;
                state.active = false;
                ui.set_state(i, state);
                if !drag.active {
                    // A click edits: the caret goes to the click in a field being
                    // edited; otherwise the field focuses with its value selected.
                    self.focus_to(ui, Some(drag.node));
                    if drag.editing {
                        ui.select_text_at(i, drag.start, false, clipboard)?;
                    } else {
                        ui.edit(i, EditEvent::SelectAll, clipboard)?;
                    }
                } else if drag.value != drag.initial {
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
                    if drag.previewed.is_some() {
                        ui.emit(i, serde_json::json!(drag.original));
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
