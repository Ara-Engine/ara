//! Anchored colour controls, using the shared popover placement and dismissal API.
use super::*;

impl Shell {
    pub(super) fn inspector_colour_popup(&mut self, out: &mut String) {
        let Some(path) = self.color_picker.clone() else {
            return;
        };
        let value = self.field_value(&path);
        let (root, pointer) = path.split_once('#').unwrap_or((&path, ""));
        let Some(field) = self.fields.get(root) else {
            return;
        };
        let count = match &field.widget {
            crate::inspector::Widget::Vector { count, .. } if pointer.is_empty() => *count,
            _ => value.as_array().map_or(4, Vec::len),
        };
        let anchor = if pointer.is_empty() {
            format!("swatch-{}", path.replace('.', "-"))
        } else {
            format!(
                "nest-swatch-{}",
                format!("{root}{pointer}")
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect::<String>()
            )
        };
        if self.shown_node(&anchor).is_none() {
            return;
        }
        out.push_str(&format!("<popover id='inspector-colour-popup' anchor='{}' placement='left-start' offset='6' class='inspector-colour-popup'><panel id='inspector-colour-title' class='row'><text id='inspector-colour-heading' class='fill' text='Colour' /><button id='inspector-colour-close' class='ghost icon-button' label='Close colour picker' action='Dispatch'>{}</button></panel>",escape(&anchor),icons::icon_in("inspector-colour-close","x","icon-sm")));
        self.bindings.insert(
            ara_ui::source::stable_id("inspector-colour-close"),
            Binding::ColorPicker(path.clone()),
        );
        self.bindings.insert(
            ara_ui::source::stable_id("inspector-colour-hex"),
            if pointer.is_empty() {
                Binding::Field(path.clone(), None)
            } else {
                Binding::Nested(path.clone(), compound::Leaf::Hex, false, None)
            },
        );
        out.push_str(&format!("<field id='inspector-colour-hex' class='mono' text='{}' label='sRGB hex colour' description='Enter #RRGGBB; alpha keeps its value' action='Dispatch' />",escape(&if value.is_null(){"—".into()}else{compound::linear_hex(&value)})));
        for axis in 0..count.min(4) {
            let id = format!("inspector-colour-channel-{axis}");
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                if pointer.is_empty() {
                    Binding::Field(path.clone(), Some(axis))
                } else {
                    Binding::Nested(
                        format!("{path}/{axis}"),
                        compound::Leaf::Number,
                        false,
                        None,
                    )
                },
            );
            out.push_str(&format!("<panel id='{id}-row' class='row' style='height:26px;gap:8px'><text id='{id}-label' class='caption' style='width:36px' text='{}' /><slider id='{id}' style='flex-grow:1;flex-basis:0px' min='0' max='1' step='0.001' value='{}' label='{}' action='Dispatch' /></panel>",["Red","Green","Blue","Alpha"][axis],value[axis].as_f64().unwrap_or(1.).clamp(0.,1.),["Red","Green","Blue","Alpha"][axis]));
        }
        out.push_str("</popover>");
    }
    pub(crate) fn inspector_popup_contains(&self, point: [f32; 2]) -> bool {
        self.inspector_popovers.contains(&self.ui, point)
    }
    pub(crate) fn dismiss_inspector_colour(&mut self, point: [f32; 2]) -> ara_ui::Result<bool> {
        let dismissal = self.inspector_popovers.pointer_down(&self.ui, point);
        if dismissal
            .close
            .iter()
            .any(|id| id == "inspector-colour-popup")
        {
            self.color_picker = None;
            if let Some(index) = self.ui.find("inspector-colour-popup") {
                self.ui.set_visible(index, false)?;
            }
            self.inspector_popovers
                .sync(&mut self.ui, &mut self.input)?;
            return Ok(dismissal.on_anchor);
        }
        Ok(false)
    }
}
