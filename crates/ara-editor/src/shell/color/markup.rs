//! The picker popover's markup, and the text its fields show (which a drag updates in
//! place). Painted parts (`cp-wheel`, `cp-track-*`, swatches) are empty boxes here;
//! `paint.rs` fills them.
use super::*;

/// Swatches per row: the popover's width over a swatch and its gap.
const SWATCH_ROW: usize = 11;

/// Every picker field's ID and text.
pub(super) fn field_texts(picker: &ColorPicker) -> Vec<(String, String)> {
    let mut out: Vec<_> = (0..picker.channels())
        .map(|i| (format!("cp-field-{i}"), picker.channel_text(i)))
        .collect();
    let base = picker.base();
    let alpha = (picker.channels() > 3 && picker.alpha < 1.).then_some(picker.alpha);
    out.push((
        "cp-hex".into(),
        math::format_hex(base.map(math::linear_to_srgb), alpha),
    ));
    out.push(("cp-hex-linear".into(), math::format_hex(base, alpha)));
    if picker.hints.hdr {
        out.push(("cp-stops".into(), format!("{:+.2}", picker.stops)));
    }
    if picker.hints.temperature {
        out.push((
            "cp-kelvin".into(),
            picker
                .kelvin
                .map_or_else(|| "—".into(), |k| format!("{k:.0} K")),
        ));
    }
    out
}

impl Shell {
    /// The picker popover, beside its Inspector swatch or where it is pinned. An unpinned
    /// picker closes when its field leaves the Inspector (another selection).
    pub(in crate::shell) fn color_picker_markup(&mut self, editor: &Editor, out: &mut String) {
        let Some(picker) = &mut self.color_picker else {
            return;
        };
        let current = picker.entities == editor.state.selection && picker.doc == editor.state.doc;
        match self.fields.get(&picker.field.path) {
            // Keep a nested value's siblings current for the commands it writes.
            Some(field) if current => picker.field = field.clone(),
            _ if picker.pinned.is_some() => {}
            _ => {
                let marker = picker.marker();
                self.drafts.remove(&marker);
                self.color_picker = None;
                return;
            }
        }
        let Some(picker) = &mut self.color_picker else {
            return;
        };
        picker.swatches = color_list(editor, "color.swatches");
        picker.recent = color_list(editor, "color.recent");
        let anchor = picker.anchor();
        let anchored = self.shown_node(&anchor).is_some();
        let Some(picker) = &self.color_picker else {
            return;
        };
        // A pinned picker lives in the main window.
        if picker.pinned.is_some() && self.current.is_some() {
            return;
        }
        let place = match picker.pinned {
            Some([x, y]) => format!("at='{x},{y}' placement='bottom-start' offset='0'"),
            None if anchored => format!("anchor='{anchor}' placement='left-start' offset='8'"),
            None => return,
        };
        let component = picker.field.path.split('.').next().unwrap_or_default();
        let mut title = format!(
            "{} · {}",
            super::super::inspector_panel::sentence(component),
            super::super::inspector_panel::sentence(&picker.field.label)
        );
        if let Some(pointer) = &picker.pointer {
            title.push_str(&format!(
                " {}",
                pointer.trim_start_matches('/').replace('/', " ")
            ));
        }
        let mut bindings: Vec<(String, Control)> = Vec::new();
        let mut bind = |id: &str, control: Control| bindings.push((id.to_owned(), control));
        let pinned = picker.pinned.is_some();
        out.push_str(&format!(
            "<popover id='color-picker' {place} class='color-picker{}'>",
            if pinned { " color-picker-pinned" } else { "" }
        ));
        // Title: what is being edited, pin and close.
        // Pinned, the title strip is a control: dragging it moves the picker.
        let title_tag = if pinned { "button" } else { "panel" };
        bind("cp-pin", Control::Pin);
        bind("cp-close", Control::Close);
        out.push_str(&format!(
            "<{title} id='cp-title' class='cp-title cp-paint'{}><text id='cp-title-text' class='heading fill' text='{}' description='{}' /><button id='cp-pin' class='ghost icon-button{}' label='{}' description='{}' action='Dispatch'>{}</button><button id='cp-close' class='ghost icon-button' label='Close' description='Close, keeping the colour. Escape restores the old one' action='Dispatch'>{}</button></{title}>",
            if pinned { " cursor='move' description='Drag to move the picker'" } else { "" },
            escape(&title),
            escape(&picker.field.doc),
            if pinned { " segment-on" } else { "" },
            if pinned { "Unpin" } else { "Pin" },
            if pinned {
                "Return the picker to its swatch"
            } else {
                "Keep the picker open here; drag its title to move it"
            },
            icons::icon_in("cp-pin", "pin", "icon-sm"),
            icons::icon_in("cp-close", "x", "icon-sm"),
            title = title_tag,
        ));
        // Wheel, then New/Old and the eyedropper beside it.
        bind("cp-old", Control::Old);
        bind("cp-eyedropper", Control::Eyedropper);
        out.push_str(&format!(
            "<panel id='cp-top' class='cp-top'><button id='cp-wheel' class='cp-wheel cp-paint' label='Hue ring and saturation and value square' description='Drag the ring for hue, the square for saturation (across) and value (up)' /><panel id='cp-side' class='cp-side'><text id='cp-new-label' class='caption' text='New' /><panel id='cp-new' class='cp-preview' description='The edited colour' /><button id='cp-old' class='cp-preview cp-paint' label='Old colour' description='The colour the picker opened with; click to restore it' action='Dispatch' /><text id='cp-old-label' class='caption' text='Old' /><panel id='cp-side-gap' class='fill' /><button id='cp-eyedropper' class='cp-tool' label='Eyedropper' description='Pick a colour from anywhere on screen: click here then click a pixel, or drag from here and release. Escape cancels' action='Dispatch'>{}</button></panel></panel>",
            icons::icon_in("cp-eyedropper", "pipette", "icon-sm"),
        ));
        let texts: BTreeMap<String, String> = field_texts(picker).into_iter().collect();
        let text = |id: &str| texts.get(id).map(String::as_str).unwrap_or_default();
        // Hex first: an opened popover focuses its first field, ready for a paste.
        bind("cp-hex", Control::Text(Input::Hex { linear: false }));
        bind("cp-hex-linear", Control::Text(Input::Hex { linear: true }));
        out.push_str(&format!(
            "<panel id='cp-hexes' class='cp-row'><text id='cp-hex-label' class='caption' text='Hex' /><field id='cp-hex' class='mono cp-hex' text='{}' label='Hex sRGB' description='sRGB #RRGGBB, #RGB or #RRGGBBAA' action='Dispatch' /><text id='cp-hex-linear-label' class='caption' text='Linear' /><field id='cp-hex-linear' class='mono cp-hex' text='{}' label='Hex linear' description='Linear-light #RRGGBB, as stored' action='Dispatch' /></panel>",
            escape(text("cp-hex")),
            escape(text("cp-hex-linear")),
        ));
        // Units.
        out.push_str("<panel id='cp-modes' class='segment'>");
        for (mode, label, description) in [
            (Mode::Rgb, "RGB", "sRGB channels, 0–255"),
            (
                Mode::Hsv,
                "HSV",
                "Hue in degrees, saturation and value in percent",
            ),
            (
                Mode::Linear,
                "Linear",
                "Linear-light channels, 0–1, as stored",
            ),
        ] {
            let id = format!("cp-mode-{}", label.to_lowercase());
            bind(&id, Control::Mode(mode));
            out.push_str(&format!(
                "<button id='{id}' class='segment-button cp-mode{}' label='{label}' description='{description}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",
                if picker.mode == mode { " segment-on" } else { "" }
            ));
        }
        out.push_str("</panel>");
        // Channel sliders and their fields.
        let names = match picker.mode {
            Mode::Hsv => [
                ("H", "Hue, 0–360°"),
                ("S", "Saturation, 0–100%"),
                ("V", "Value, 0–100%"),
            ],
            Mode::Rgb => [
                ("R", "Red, 0–255"),
                ("G", "Green, 0–255"),
                ("B", "Blue, 0–255"),
            ],
            Mode::Linear => [
                ("R", "Linear red, 0–1"),
                ("G", "Linear green, 0–1"),
                ("B", "Linear blue, 0–1"),
            ],
        };
        let alpha_name = match picker.mode {
            Mode::Rgb => "Alpha, 0–255",
            Mode::Hsv => "Alpha, 0–100%",
            Mode::Linear => "Alpha, 0–1",
        };
        for i in 0..picker.channels() {
            let (name, description) = names.get(i).copied().unwrap_or(("A", alpha_name));
            let field = format!("cp-field-{i}");
            bind(&field, Control::Text(Input::Channel(i)));
            out.push_str(&format!(
                "<panel id='cp-row-{i}' class='cp-row'><text id='cp-label-{i}' class='cp-label' text='{name}' /><button id='cp-track-{i}' class='cp-track cp-paint' label='{name} slider' description='{description}' /><field id='{field}' class='number cp-number' text='{}' label='{}' description='{description}' action='Dispatch' /></panel>",
                escape(text(&field)),
                escape(description.split(',').next().unwrap_or(name)),
            ));
        }
        // HDR intensity and exposure steps.
        if picker.hints.hdr {
            bind("cp-stops", Control::Text(Input::Stops));
            out.push_str(&format!(
                "<panel id='cp-hdr' class='cp-row'><text id='cp-stops-label' class='cp-label-wide' text='Intensity' /><button id='cp-stops-track' class='cp-track cp-paint' label='Intensity slider' description='Intensity in stops: each stop doubles the brightness' /><field id='cp-stops' class='number cp-number' text='{}' label='Intensity' description='Stops, −10 to +10; the stored colour is the base times 2^stops' action='Dispatch' /></panel><panel id='cp-exposure' class='cp-row'>",
                escape(text("cp-stops")),
            ));
            for (i, step) in (-2i8..=2).enumerate() {
                let id = format!("cp-exposure-{i}");
                bind(&id, Control::Exposure(step));
                let scale = (picker.stops + f64::from(step)).exp2();
                let ink = math::contrast_text(picker.base().map(|c| c * scale));
                out.push_str(&format!(
                    "<button id='{id}' class='cp-exposure cp-paint' label='{step:+} stops' description='{}' action='Dispatch'><text id='{id}-label' class='cp-exposure-label' style='color:{ink}' text='{}' /></button>",
                    if step == 0 { "The current intensity".to_owned() } else { format!("{step:+} stops: {} as bright", if step > 0 { format!("{}×", 1 << step) } else { format!("1/{}", 1 << -step) }) },
                    if step == 0 { "0".to_owned() } else { format!("{step:+}") },
                ));
            }
            out.push_str("</panel>");
        }
        // Colour temperature.
        if picker.hints.temperature {
            bind("cp-kelvin", Control::Text(Input::Kelvin));
            out.push_str(&format!(
                "<panel id='cp-temperature' class='cp-row'><text id='cp-kelvin-label' class='cp-label-wide' text='Temperature' /><button id='cp-kelvin-track' class='cp-track cp-paint' label='Temperature slider' description='Blackbody colour, 1000–15000 K' /><field id='cp-kelvin' class='number cp-number' text='{}' label='Temperature' description='Kelvin, 1000–15000' action='Dispatch' /></panel>",
                escape(text("cp-kelvin")),
            ));
            for (row, presets) in math::KELVIN_PRESETS.chunks(4).enumerate() {
                out.push_str(&format!("<panel id='cp-presets-{row}' class='cp-row'>"));
                for (n, (name, kelvin)) in presets.iter().enumerate() {
                    let index = row * 4 + n;
                    let id = format!("cp-preset-{index}");
                    bind(&id, Control::Preset(index));
                    let ink = math::contrast_text(math::kelvin_to_linear(*kelvin));
                    out.push_str(&format!(
                        "<button id='{id}' class='cp-preset cp-paint' label='{name}' description='{name}, {kelvin:.0} K' action='Dispatch'><text id='{id}-label' class='cp-preset-label' style='color:{ink}' text='{name}' /></button>",
                    ));
                }
                out.push_str("</panel>");
            }
        }
        // Project swatches and recent colours.
        bind("cp-add-swatch", Control::AddSwatch);
        let (swatches, recent) = (&picker.swatches, &picker.recent);
        out.push_str(&format!(
            "<panel id='cp-swatches-head' class='cp-row'><text id='cp-swatches-title' class='caption fill' text='Swatches · {}' /><button id='cp-add-swatch' class='ghost icon-button' label='Save swatch' description='Save this colour to the project, for everyone' action='Dispatch'>{}</button></panel>",
            swatches.len(),
            icons::icon_in("cp-add-swatch", "plus", "icon-sm"),
        ));
        if swatches.is_empty() {
            out.push_str("<text id='cp-swatches-empty' class='caption wrap-text' text='Save colours with + to share them with the project.' />");
        }
        for (prefix, list, description) in [
            (
                "swatch",
                swatches,
                "Click to apply; Shift+click removes it from the project",
            ),
            (
                "recent",
                recent,
                "A colour you picked lately; click to apply",
            ),
        ] {
            if prefix == "recent" && !list.is_empty() {
                out.push_str("<text id='cp-recent-title' class='caption' text='Recent' />");
            }
            for (row, chunk) in list.chunks(SWATCH_ROW).enumerate() {
                out.push_str(&format!(
                    "<panel id='cp-{prefix}-row-{row}' class='cp-swatches'>"
                ));
                for (n, color) in chunk.iter().enumerate() {
                    let index = row * SWATCH_ROW + n;
                    let id = format!("cp-{prefix}-{index}");
                    bind(
                        &id,
                        if prefix == "swatch" {
                            Control::Swatch(index)
                        } else {
                            Control::Recent(index)
                        },
                    );
                    let hex = math::format_hex(
                        [color[0], color[1], color[2]].map(|c| math::linear_to_srgb(c.min(1.))),
                        None,
                    );
                    out.push_str(&format!(
                        "<button id='{id}' class='cp-swatch cp-paint' label='#{hex}' description='{description}' action='Dispatch' />"
                    ));
                }
                out.push_str("</panel>");
            }
        }
        out.push_str("</popover>");
        for (id, control) in bindings {
            self.bindings
                .insert(ara_ui::source::stable_id(&id), Binding::Picker(control));
        }
    }
}
