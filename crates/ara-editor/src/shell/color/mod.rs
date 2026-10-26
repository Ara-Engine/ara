//! The colour picker: a hue ring around a saturation/value square, RGB, HSV and linear
//! channel sliders with typed fields, sRGB and linear hex, alpha over a checkerboard,
//! HDR intensity in stops, a Kelvin slider and presets for light colours, Old/New
//! swatches, an eyedropper, project swatches and recent colours.
//!
//! It is a popover beside the Inspector swatch that opened it, or pinned where the user
//! puts it (a pinned picker ignores clicks outside and keeps editing the entities it
//! opened on). Drags preview through a `@picker|target` draft marker, which the Scene
//! previews like a scrub, and apply one transaction on release; typed values, swatches,
//! presets and exposure steps apply one each; Escape restores the colour it opened with.
mod eyedropper;
mod markup;
pub(crate) mod math;
mod paint;

use super::*;
use crate::inspector::Widget;

/// The units the channel sliders and fields edit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    /// sRGB bytes, 0–255.
    #[default]
    Rgb,
    /// Hue in degrees, saturation and value in percent.
    Hsv,
    /// Linear-light channels, 0–1 (what documents store).
    Linear,
}

/// Schema hints that add picker sections ([`ara_schema::EditorHints`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Hints {
    /// Intensity in stops, for colours that may exceed 1.
    pub(crate) hdr: bool,
    /// A Kelvin slider and presets, for light colours.
    pub(crate) temperature: bool,
}

/// A typed text box in the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Input {
    /// A channel's field in the mode's units; 3 is alpha.
    Channel(usize),
    /// sRGB (`linear: false`) or linear hex.
    Hex {
        /// Linear-light hex instead of sRGB.
        linear: bool,
    },
    /// HDR intensity in stops.
    Stops,
    /// Colour temperature in Kelvin.
    Kelvin,
}

/// A picker button or field: what its [`Binding::Picker`] does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Control {
    /// Switch the slider units.
    Mode(Mode),
    /// Text typed into a field (applied on Enter, Tab or a click elsewhere).
    Text(Input),
    /// An exposure button: this many stops brighter.
    Exposure(i8),
    /// A Kelvin preset ([`math::KELVIN_PRESETS`]).
    Preset(usize),
    /// A project swatch: apply it; Shift+click removes it.
    Swatch(usize),
    /// A recent colour.
    Recent(usize),
    /// Save the current colour as a project swatch.
    AddSwatch,
    /// Restore the colour the picker opened with.
    Old,
    /// Arm the eyedropper from the keyboard (a click then picks).
    Eyedropper,
    /// Pin the picker where it is, or unpin it.
    Pin,
    /// Close, keeping the colour.
    Close,
}

/// What a pointer drag inside the picker edits.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Ring,
    Square,
    Channel(usize),
    Stops,
    Kelvin,
    /// A pinned picker dragged by its title; the grab offset from its origin.
    Move([f32; 2]),
}

/// The lowest and highest HDR intensity the picker offers, in stops.
const STOPS: (f64, f64) = (-10., 10.);
/// The hue ring's thickness and the gap between it and the square, in logical pixels.
const RING: f32 = 16.;
const RING_GAP: f32 = 6.;

/// An open colour picker.
pub(crate) struct ColorPicker {
    /// `Component.field`, or `Component.field#/pointer` inside a compound value.
    pub(crate) target: String,
    hints: Hints,
    /// The document and entities it edits, captured when it opened.
    doc: String,
    entities: Vec<String>,
    /// The field it opened on (a pinned picker outlives the Inspector's selection).
    field: crate::inspector::Field,
    pointer: Option<String>,
    /// Channels the value has: 3 (RGB) or 4 (RGBA).
    count: usize,
    /// Linear RGBA when it opened (Old, Escape) and as last applied.
    old: [f64; 4],
    applied: [f64; 4],
    /// The edited colour: display-space HSV of the base colour, alpha and HDR stops.
    hsv: [f64; 3],
    alpha: f64,
    stops: f64,
    /// The temperature the colour was set from, or the one it matches.
    kelvin: Option<f64>,
    mode: Mode,
    drag: Option<Part>,
    /// Top-left of a pinned picker.
    pinned: Option<[f32; 2]>,
    /// Text typed into a field and not applied yet.
    text: Option<(Input, String)>,
    eyedropper: Option<eyedropper::Eyedropper>,
    /// The project swatches and recent colours shown, as of the last rebuild.
    swatches: Vec<[f64; 4]>,
    recent: Vec<[f64; 4]>,
}

impl ColorPicker {
    /// The linear base colour (at most 1), before the HDR intensity.
    fn base(&self) -> [f64; 3] {
        math::hsv_to_rgb(self.hsv).map(math::srgb_to_linear)
    }
    /// The edited colour as stored: linear RGBA, intensity applied.
    fn value(&self) -> [f64; 4] {
        let scale = self.stops.exp2();
        let [r, g, b] = self.base().map(|c| c * scale);
        [r, g, b, self.alpha]
    }
    /// Take a stored linear RGBA colour, keeping the hue (and saturation) where the
    /// colour no longer defines them (greys, black).
    fn set_linear(&mut self, linear: [f64; 4]) {
        let rgb = [linear[0], linear[1], linear[2]].map(|c| c.max(0.));
        let (base, stops) = if self.hints.hdr {
            math::split_hdr(rgb)
        } else {
            (rgb.map(|c| c.min(1.)), 0.)
        };
        self.set_display(base.map(math::linear_to_srgb));
        self.stops = stops.clamp(STOPS.0, STOPS.1);
        self.alpha = linear[3].clamp(0., 1.);
    }
    /// Set the base colour from display-space RGB.
    fn set_display(&mut self, rgb: [f64; 3]) {
        let hsv = math::rgb_to_hsv(rgb.map(|c| c.clamp(0., 1.)));
        let keep_hue = hsv[1] <= 1e-9 || hsv[2] <= 1e-9;
        self.hsv = [
            if keep_hue { self.hsv[0] } else { hsv[0] },
            if hsv[2] <= 1e-9 { self.hsv[1] } else { hsv[1] },
            hsv[2],
        ];
    }
    fn set_kelvin(&mut self, kelvin: f64) {
        let kelvin = kelvin.clamp(math::KELVIN.0, math::KELVIN.1).round();
        self.set_display(math::kelvin_to_linear(kelvin).map(math::linear_to_srgb));
        self.kelvin = Some(kelvin);
    }
    /// A channel along its slider, 0–1, in the current mode; 3 is alpha.
    fn channel(&self, i: usize) -> f64 {
        match (self.mode, i) {
            (_, 3) => self.alpha,
            (Mode::Hsv, i) => self.hsv[i],
            (Mode::Rgb, i) => math::hsv_to_rgb(self.hsv)[i],
            (Mode::Linear, i) => self.base()[i],
        }
    }
    fn set_channel(&mut self, i: usize, t: f64) {
        let t = t.clamp(0., 1.);
        match (self.mode, i) {
            (_, 3) => self.alpha = t,
            (Mode::Hsv, i) => self.hsv[i] = t,
            (Mode::Rgb, i) => {
                let mut rgb = math::hsv_to_rgb(self.hsv);
                rgb[i] = t;
                self.set_display(rgb);
            }
            (Mode::Linear, i) => {
                let mut linear = self.base();
                linear[i] = t;
                self.set_display(linear.map(math::linear_to_srgb));
            }
        }
        if i < 3 {
            self.kelvin = None;
        }
    }
    /// A channel's text in the mode's units: bytes, degrees and percent, or 0–1.
    fn channel_text(&self, i: usize) -> String {
        let t = self.channel(i);
        match (self.mode, i) {
            (Mode::Rgb, _) => format!("{}", (t * 255.).round()),
            (Mode::Hsv, 0) => format!("{}", (t * 360.).round()),
            (Mode::Hsv, _) => format!("{}", (t * 100.).round()),
            (Mode::Linear, _) => format!("{:.3}", t),
        }
    }
    /// Parse text typed into `input`; Err explains what it takes.
    fn set_text(&mut self, input: Input, text: &str) -> Result<(), String> {
        let number = |text: &str| {
            text.trim()
                .trim_end_matches(['%', '°', 'K', 'k'])
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
        };
        match input {
            Input::Channel(i) => {
                let n = number(text).ok_or("Enter a number")?;
                let t = match (self.mode, i) {
                    (Mode::Rgb, _) => n / 255.,
                    (Mode::Hsv, 0) => n.rem_euclid(360.) / 360.,
                    (Mode::Hsv, _) => n / 100.,
                    (Mode::Linear, _) => n,
                };
                self.set_channel(i, t);
            }
            Input::Hex { linear } => {
                let (rgb, alpha) = math::parse_hex(text)
                    .ok_or("Enter a colour as #RRGGBB, #RGB or #RRGGBBAA")?;
                self.set_display(if linear {
                    rgb.map(math::linear_to_srgb)
                } else {
                    rgb
                });
                if let Some(alpha) = alpha.filter(|_| self.count > 3) {
                    self.alpha = alpha;
                }
                self.kelvin = None;
            }
            Input::Stops => {
                let n = number(text).ok_or("Enter the intensity in stops")?;
                self.stops = n.clamp(STOPS.0, STOPS.1);
            }
            Input::Kelvin => {
                let n = number(text).ok_or("Enter a temperature in Kelvin")?;
                self.set_kelvin(n);
            }
        }
        Ok(())
    }
    /// The stored value (channels rounded for stable documents).
    fn json(&self) -> Value {
        json!(
            self.value()[..self.count]
                .iter()
                .map(|c| math::stable(*c))
                .collect::<Vec<_>>()
        )
    }
    /// `property.set` commands writing the colour to every captured entity.
    fn commands(&self) -> Vec<Value> {
        match &self.pointer {
            None => crate::inspector::edit_commands(&self.entities, &self.field.path, &self.json()),
            Some(pointer) => compound::preview_value(&self.field.value, pointer, self.json())
                .map(|whole| {
                    crate::inspector::edit_commands(&self.entities, &self.field.path, &whole)
                })
                .unwrap_or_default(),
        }
    }
    /// Whether it is pinned (it stays open through clicks outside it).
    pub(crate) fn pinned(&self) -> bool {
        self.pinned.is_some()
    }
    /// The draft marker that makes the Scene preview a gesture.
    fn marker(&self) -> String {
        format!("@picker|{}", self.target)
    }
    /// The field and its whole previewed value, while a gesture previews.
    pub(super) fn preview(
        &self,
        drafts: &BTreeMap<String, String>,
    ) -> Option<(&crate::inspector::Field, Value, &[String])> {
        drafts.get(&self.marker())?;
        let value = match &self.pointer {
            None => self.json(),
            Some(pointer) => compound::preview_value(&self.field.value, pointer, self.json())?,
        };
        Some((&self.field, value, &self.entities))
    }
    /// The Inspector swatch it opened from.
    fn anchor(&self) -> String {
        match &self.pointer {
            None => format!("swatch-{}", self.target.replace('.', "-")),
            Some(_) => format!(
                "nest-swatch-{}",
                self.target
                    .replace('#', "")
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect::<String>()
            ),
        }
    }
}

impl Shell {
    /// Open the picker on an Inspector colour (`target` is `path` or `path#pointer`), or
    /// close it when it is already open there.
    pub(super) fn toggle_color_picker(
        &mut self,
        editor: &mut Editor,
        target: &str,
        hints: Hints,
    ) -> ara_ui::Result<()> {
        let same = self
            .color_picker
            .as_ref()
            .is_some_and(|p| p.target == target);
        self.close_color_picker(editor, false)?;
        if same {
            return Ok(());
        }
        let (path, pointer) = target
            .split_once('#')
            .map_or((target, None), |(path, pointer)| (path, Some(pointer)));
        let Some(field) = self.fields.get(path).filter(|f| !f.read_only).cloned() else {
            return Ok(());
        };
        let value = match pointer {
            Some(pointer) => field.value.pointer(pointer).cloned().unwrap_or(Value::Null),
            None if field.optional && field.value.is_null() => field.fallback.clone(),
            None if field.mixed => Value::Array(field.channels.clone()),
            None => field.value.clone(),
        };
        let count = match &field.widget {
            Widget::Vector { count, .. } if pointer.is_none() => *count,
            _ => value.as_array().map_or(4, Vec::len),
        }
        .clamp(3, 4);
        let channel = |n: usize| value.get(n).and_then(Value::as_f64).unwrap_or(1.);
        let old = [
            channel(0),
            channel(1),
            channel(2),
            if count > 3 { channel(3) } else { 1. },
        ];
        let hints = Hints {
            hdr: hints.hdr || old[..3].iter().any(|c| *c > 1.),
            ..hints
        };
        let mut picker = ColorPicker {
            target: target.to_owned(),
            hints,
            doc: editor.state.doc.clone(),
            entities: editor.state.selection.clone(),
            field,
            pointer: pointer.map(str::to_owned),
            count,
            old,
            applied: old,
            hsv: [0., 0., 0.],
            alpha: 1.,
            stops: 0.,
            kelvin: None,
            mode: self.color_mode,
            drag: None,
            pinned: None,
            text: None,
            eyedropper: None,
            swatches: Vec::new(),
            recent: Vec::new(),
        };
        picker.set_linear(old);
        if hints.temperature {
            picker.kelvin = math::linear_to_kelvin(picker.base()).map(f64::round);
        }
        self.color_picker = Some(picker);
        Ok(())
    }
    /// Close the picker. `revert` (Escape) restores the colour it opened with as one
    /// transaction; otherwise the colour stays, and joins the recent colours.
    pub fn close_color_picker(
        &mut self,
        editor: &mut Editor,
        revert: bool,
    ) -> ara_ui::Result<()> {
        if !revert {
            self.commit_picker_text(editor)?;
        }
        let Some(mut picker) = self.color_picker.take() else {
            return Ok(());
        };
        self.drafts.remove(&picker.marker());
        editor.end_field_gestures()?;
        if editor.state.doc != picker.doc {
            return Ok(());
        }
        if revert {
            if picker.applied != picker.old {
                picker.set_linear(picker.old);
                editor.apply(picker.commands(), "Restore colour", None)?;
            }
        } else if picker.applied != picker.old {
            let applied = picker.applied;
            self.remember_color(editor, applied)?;
        }
        Ok(())
    }
    /// Whether the picker is open on `target` (`path` or `path#pointer`).
    pub fn color_picker_open(&self, target: &str) -> bool {
        self.color_picker.as_ref().is_some_and(|p| p.target == target)
    }
    /// Verification routes: open the picker on the Inspector field `path` as its swatch
    /// does, in `mode` (`rgb`, `hsv` or `linear`), `stops` brighter (applied), pinned
    /// at `pin` or with the eyedropper armed.
    pub fn show_color_picker(
        &mut self,
        editor: &mut Editor,
        path: &str,
        mode: &str,
        stops: i8,
        pin: Option<[f32; 2]>,
        eyedropper: bool,
    ) -> ara_ui::Result<()> {
        let hints = match self.fields.get(path).map(|f| &f.widget) {
            Some(Widget::Vector {
                hdr, temperature, ..
            }) => Hints {
                hdr: *hdr,
                temperature: *temperature,
            },
            _ => Hints::default(),
        };
        if !self.color_picker_open(path) {
            self.toggle_color_picker(editor, path, hints)?;
        }
        let mode = match mode {
            "hsv" => Mode::Hsv,
            "linear" => Mode::Linear,
            _ => Mode::Rgb,
        };
        self.picker_control(editor, Control::Mode(mode), &Value::Null)?;
        if stops != 0 {
            self.picker_control(editor, Control::Exposure(stops), &Value::Null)?;
        }
        if let Some(picker) = &mut self.color_picker {
            picker.pinned = pin;
        }
        if eyedropper {
            self.arm_eyedropper(false);
        }
        self.rebuild(editor)
    }
    /// Apply the picker's colour as one undoable transaction, when it changed.
    fn apply_color(&mut self, editor: &mut Editor, why: &str) -> ara_ui::Result<()> {
        let Some(picker) = &mut self.color_picker else {
            return Ok(());
        };
        self.drafts.remove(&picker.marker());
        let value = picker.value();
        if value == picker.applied || editor.state.doc != picker.doc {
            return Ok(());
        }
        let commands = picker.commands();
        picker.applied = value;
        if !commands.is_empty() {
            editor.apply(commands, why, None)?;
        }
        Ok(())
    }
    /// Put `color` first in the user's recent colours.
    fn remember_color(&mut self, editor: &mut Editor, color: [f64; 4]) -> ara_ui::Result<()> {
        let mut recent = color_list(editor, "color.recent");
        recent.retain(|c| (0..4).any(|i| (c[i] - color[i]).abs() > 1e-4));
        recent.insert(0, color);
        recent.truncate(crate::settings::RECENT_COLORS);
        set_color_list(editor, "color.recent", &recent)
    }
    /// A picker button or field changed; true when the shell should rebuild.
    pub(super) fn picker_control(
        &mut self,
        editor: &mut Editor,
        control: Control,
        value: &Value,
    ) -> ara_ui::Result<bool> {
        let shift = self.modifiers[0];
        let Some(picker) = &mut self.color_picker else {
            return Ok(false);
        };
        match control {
            Control::Text(input) => {
                let text = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                picker.text = Some((input, text));
                return Ok(false);
            }
            Control::Mode(mode) => {
                picker.mode = mode;
                self.color_mode = mode;
            }
            Control::Exposure(step) => {
                picker.stops = (picker.stops + f64::from(step)).clamp(STOPS.0, STOPS.1);
                self.apply_color(editor, "Change colour intensity")?;
            }
            Control::Preset(index) => {
                if let Some((_, kelvin)) = math::KELVIN_PRESETS.get(index) {
                    picker.set_kelvin(*kelvin);
                    self.apply_color(editor, "Set colour temperature")?;
                }
            }
            Control::Swatch(index) => {
                let mut swatches = color_list(editor, "color.swatches");
                if shift {
                    if index < swatches.len() {
                        swatches.remove(index);
                        set_color_list(editor, "color.swatches", &swatches)?;
                    }
                } else if let Some(color) = swatches.get(index) {
                    picker.set_linear(*color);
                    picker.kelvin = None;
                    self.apply_color(editor, "Apply colour swatch")?;
                }
            }
            Control::Recent(index) => {
                if let Some(color) = color_list(editor, "color.recent").get(index) {
                    picker.set_linear(*color);
                    picker.kelvin = None;
                    self.apply_color(editor, "Apply recent colour")?;
                }
            }
            Control::AddSwatch => {
                let color = picker.value();
                let mut swatches = color_list(editor, "color.swatches");
                if swatches.len() < crate::settings::COLOR_SWATCHES
                    && !swatches
                        .iter()
                        .any(|c| (0..4).all(|i| (c[i] - color[i]).abs() <= 1e-4))
                {
                    swatches.push(color);
                    set_color_list(editor, "color.swatches", &swatches)?;
                }
            }
            Control::Old => {
                let old = picker.old;
                picker.set_linear(old);
                self.apply_color(editor, "Restore colour")?;
            }
            Control::Eyedropper => self.arm_eyedropper(false),
            Control::Pin => {
                picker.pinned = match picker.pinned {
                    Some(_) => None,
                    None => self
                        .ui
                        .find("color-picker")
                        .map(|i| [self.ui.nodes[i].rect.x, self.ui.nodes[i].rect.y]),
                };
            }
            Control::Close => self.close_color_picker(editor, false)?,
        }
        Ok(true)
    }
    /// Apply text typed into a picker field; false when there is none.
    pub fn commit_picker_text(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(picker) = &mut self.color_picker else {
            return Ok(false);
        };
        let Some((input, text)) = picker.text.take() else {
            return Ok(false);
        };
        match picker.set_text(input, &text) {
            Ok(()) => self.apply_color(editor, "Edit colour")?,
            Err(why) => self.status_note = Some(why),
        }
        Ok(true)
    }
    /// Escape: cancel the eyedropper or a drag, else restore the old colour and close.
    pub fn picker_escape(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(picker) = &mut self.color_picker else {
            return Ok(false);
        };
        if picker.eyedropper.is_some() {
            self.cancel_eyedropper();
            return Ok(true);
        }
        if picker.drag.take().is_some() {
            let applied = picker.applied;
            picker.set_linear(applied);
            self.drafts.remove(&picker.marker());
            editor.end_field_gestures()?;
            return Ok(true);
        }
        self.close_color_picker(editor, true)?;
        Ok(true)
    }
    /// Start a drag on the ring, the square, a slider track, the eyedropper or a pinned
    /// picker's title; false when the press is elsewhere.
    pub fn begin_picker_drag(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(pinned) = self.color_picker.as_ref().map(|p| p.pinned) else {
            return Ok(false);
        };
        let Some(hit) = self.ui.hit_test(self.pointer[0], self.pointer[1]) else {
            return Ok(false);
        };
        let (id, rect) = (
            self.ui.nodes[hit].element.id.clone(),
            self.ui.nodes[hit].rect,
        );
        let part = match id.as_str() {
            "cp-wheel" => match wheel_part(rect, self.pointer) {
                Some(part) => part,
                None => return Ok(false),
            },
            "cp-eyedropper" | "cp-eyedropper-icon" => {
                self.arm_eyedropper(true);
                return Ok(true);
            }
            "cp-title" | "cp-title-text" => match pinned {
                Some(origin) => Part::Move([self.pointer[0] - origin[0], self.pointer[1] - origin[1]]),
                None => return Ok(false),
            },
            "cp-stops-track" => Part::Stops,
            "cp-kelvin-track" => Part::Kelvin,
            id => match id.strip_prefix("cp-track-").and_then(|n| n.parse().ok()) {
                Some(channel) => Part::Channel(channel),
                None => return Ok(false),
            },
        };
        let Some(picker) = &mut self.color_picker else {
            return Ok(false);
        };
        if !matches!(part, Part::Move(_)) {
            for entity in &picker.entities {
                editor.field_gesture(entity, &picker.field.path, true)?;
            }
        }
        picker.drag = Some(part);
        self.move_picker()?;
        Ok(true)
    }
    /// Pointer motion during a picker drag or the eyedropper: preview, never author.
    pub fn move_picker(&mut self) -> ara_ui::Result<bool> {
        let rect = |ui: &Ui, id: &str| ui.find(id).map(|i| ui.nodes[i].rect);
        let pointer = self.pointer;
        let Some(picker) = &mut self.color_picker else {
            return Ok(false);
        };
        if picker.eyedropper.is_some() {
            return Ok(false);
        }
        let Some(part) = picker.drag else {
            return Ok(false);
        };
        let along = |r: Option<Box2>| r.map(|r| f64::from((pointer[0] - r.x) / r.w.max(1.)));
        match part {
            Part::Move(grab) => {
                let origin = [pointer[0] - grab[0], pointer[1] - grab[1]];
                picker.pinned = Some(origin);
                if let Some(i) = self.ui.find("color-picker") {
                    self.ui.nodes[i]
                        .element
                        .attrs
                        .insert("at".into(), format!("{},{}", origin[0], origin[1]));
                    self.ui.nodes[i].dirty.layout = true;
                }
                return Ok(true);
            }
            Part::Ring => {
                let Some(r) = rect(&self.ui, "cp-wheel") else {
                    return Ok(false);
                };
                let (dx, dy) = (pointer[0] - (r.x + r.w / 2.), pointer[1] - (r.y + r.h / 2.));
                picker.hsv[0] = (f64::from((-dy).atan2(dx)) / std::f64::consts::TAU).rem_euclid(1.);
                picker.kelvin = None;
            }
            Part::Square => {
                let Some(r) = rect(&self.ui, "cp-wheel").map(square) else {
                    return Ok(false);
                };
                picker.hsv[1] = f64::from((pointer[0] - r.x) / r.w).clamp(0., 1.);
                picker.hsv[2] = f64::from(1. - (pointer[1] - r.y) / r.h).clamp(0., 1.);
                picker.kelvin = None;
            }
            Part::Channel(i) => {
                if let Some(t) = along(rect(&self.ui, &format!("cp-track-{i}"))) {
                    picker.set_channel(i, t);
                }
            }
            Part::Stops => {
                if let Some(t) = along(rect(&self.ui, "cp-stops-track")) {
                    let stops = STOPS.0 + t.clamp(0., 1.) * (STOPS.1 - STOPS.0);
                    picker.stops = (stops * 100.).round() / 100.;
                }
            }
            Part::Kelvin => {
                if let Some(t) = along(rect(&self.ui, "cp-kelvin-track")) {
                    let (low, high) = math::KELVIN;
                    picker.set_kelvin(((low + t.clamp(0., 1.) * (high - low)) / 50.).round() * 50.);
                }
            }
        }
        self.preview_picker()?;
        Ok(true)
    }
    /// Release: a drag applies its colour as one transaction; the eyedropper held from
    /// its button stays armed when released on it, else picks where it was released.
    pub fn finish_picker(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.eyedropper_held() {
            let on_button = self
                .ui
                .find("cp-eyedropper")
                .is_some_and(|i| self.ui.nodes[i].rect.contains(self.pointer[0], self.pointer[1]));
            if on_button {
                self.arm_eyedropper(false);
            } else {
                self.pick_eyedropper(editor)?;
            }
            return Ok(true);
        }
        let Some(part) = self.color_picker.as_mut().and_then(|p| p.drag.take()) else {
            return Ok(false);
        };
        if !matches!(part, Part::Move(_)) {
            self.apply_color(editor, "Pick colour")?;
            editor.end_field_gestures()?;
        }
        self.rebuild(editor)?;
        Ok(true)
    }
    /// Show the edited colour live: the Scene previews the draft marker; the picker's
    /// fields and painted parts update without a rebuild.
    fn preview_picker(&mut self) -> ara_ui::Result<()> {
        let Some(picker) = &self.color_picker else {
            return Ok(());
        };
        self.drafts.insert(picker.marker(), picker.json().to_string());
        let texts = markup::field_texts(picker);
        for (id, text) in texts {
            if let Some(i) = self.ui.find(&id)
                && self.input.focused() != Some(ara_ui::source::stable_id(&id))
            {
                self.ui.set_text(i, &text)?;
            }
        }
        if let Some(i) = self.ui.find("color-picker") {
            self.ui.nodes[i].dirty.paint = true;
        }
        Ok(())
    }
}

/// Which part of the wheel a press at `point` lands on: the ring band or the square.
fn wheel_part(wheel: Box2, point: [f32; 2]) -> Option<Part> {
    let (dx, dy) = (point[0] - (wheel.x + wheel.w / 2.), point[1] - (wheel.y + wheel.h / 2.));
    let distance = (dx * dx + dy * dy).sqrt();
    let outer = wheel.w.min(wheel.h) / 2.;
    if (outer - RING - 2.0..=outer + 1.).contains(&distance) {
        return Some(Part::Ring);
    }
    let s = square(wheel);
    (point[0] >= s.x - 4. && point[0] <= s.x + s.w + 4. && point[1] >= s.y - 4. && point[1] <= s.y + s.h + 4.)
        .then_some(Part::Square)
}
/// The saturation/value square inscribed in the ring.
fn square(wheel: Box2) -> Box2 {
    let inner = wheel.w.min(wheel.h) / 2. - RING - RING_GAP;
    let side = (inner * std::f32::consts::SQRT_2).floor();
    Box2 {
        x: (wheel.x + (wheel.w - side) / 2.).round(),
        y: (wheel.y + (wheel.h - side) / 2.).round(),
        w: side,
        h: side,
    }
}
/// A colour-list setting (`color.swatches`, `color.recent`) as linear RGBA.
fn color_list(editor: &Editor, key: &str) -> Vec<[f64; 4]> {
    editor
        .state
        .settings
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_array)
                .map(|c| {
                    let channel = |n: usize| c.get(n).and_then(Value::as_f64).unwrap_or(1.);
                    [channel(0), channel(1), channel(2), channel(3)]
                })
                .collect()
        })
        .unwrap_or_default()
}
fn set_color_list(editor: &mut Editor, key: &str, list: &[[f64; 4]]) -> ara_ui::Result<()> {
    let value: Vec<Vec<f64>> = list
        .iter()
        .map(|c| c.iter().map(|v| math::stable(*v)).collect())
        .collect();
    editor.action("editor.settings.set", json!({"key": key, "value": value}))?;
    Ok(())
}
