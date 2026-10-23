use super::panels::display;
use super::*;
use crate::inspector::{Provenance, Widget};
impl Shell {
    pub(super) fn inspector_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?;
        out.push_str("<scroll id='inspector-scroll' class='body'>");
        if editor.state.selection.is_empty() {
            out.push_str("<text id='inspector-empty' class='muted' text='Select an entity to inspect it.' /></scroll>");
            return Ok(());
        }
        let name = editor
            .state
            .selection
            .first()
            .and_then(|id| resolved.get(id))
            .and_then(|e| e.record.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Selection");
        if self.rename {
            let id = "entity-rename-value";
            self.bindings
                .insert(ara_ui::source::stable_id(id), Binding::RenameValue);
            out.push_str(&format!(
                "<field id='{id}' text='{}' label='Entity name' action='Dispatch' />",
                escape(name)
            ));
        } else {
            out.push_str(&self.bind_button(
                "entity-rename",
                &format!("{}  ({})", name, editor.state.selection.len()),
                Binding::Rename,
                "quiet",
            ));
        }
        let changes = editor.agent_fields();
        let doc = editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("document not found")?;
        let fields = crate::inspector::fields(
            editor.host.store().schema(),
            doc,
            &resolved,
            &editor.state.selection,
            &changes,
        );
        let mut component = String::new();
        for field in fields {
            let head = field.path.split('.').next().unwrap_or("");
            if component != head {
                component = head.into();
                out.push_str(&format!("<panel id='component-{head}' class='component'><text id='heading-{head}' class='heading' text='{head}' /><panel id='component-fill-{head}' style='flex-grow:1' />"));
                let calls=editor.state.selection.iter().map(|entity|json!({"cmd":"component.remove","args":{"entity":entity,"component":head}})).collect();
                out.push_str(&self.bind_button(
                    &format!("remove-{head}"),
                    "Remove",
                    Binding::Content(calls, format!("Remove {head}")),
                    "small quiet",
                ));
                out.push_str("</panel>");
            }
            let id = field.path.replace('.', "-");
            let provenance = match field.provenance {
                Provenance::Default => "default",
                Provenance::Prefab => "prefab",
                Provenance::Override => "override",
                Provenance::Authored => "instance",
                Provenance::AgentChanged => "agent change",
            };
            out.push_str(&format!("<panel id='field-{id}' class='field-row'><panel id='label-row-{id}' class='row'><text id='label-{id}' class='field-label' text='{}{}' description='{}' /><panel id='label-fill-{id}' style='flex-grow:1' /><text id='origin-{id}' class='badge' text='{provenance}' /></panel>",escape(&field.label),field.unit.as_ref().map(|u|format!(" ({u})")).unwrap_or_default(),escape(&field.doc)));
            match &field.widget {
                Widget::Vector { count, color } => {
                    out.push_str(&format!("<panel id='vector-{id}' class='row'>"));
                    if *color {
                        out.push_str(&self.bind_button(
                            &format!("swatch-{id}"),
                            "   ",
                            Binding::ColorPicker(field.path.clone()),
                            "small",
                        ));
                    }
                    for axis in 0..*count {
                        let widget = format!("edit-{id}-{axis}");
                        self.bindings.insert(
                            ara_ui::source::stable_id(&widget),
                            Binding::Field(field.path.clone(), Some(axis)),
                        );
                        let value = if field.mixed {
                            "—".into()
                        } else {
                            display(&field.value[axis])
                        };
                        out.push_str(&format!("<field id='{widget}' style='flex-grow:1; flex-basis:0px' text='{}' action='Dispatch' label='{} {}' description='{}' />",escape(&value),escape(&field.path),["X","Y","Z","W"][axis],escape(&field.doc)));
                    }
                    out.push_str("</panel>");
                    if *color && self.color_picker.as_deref() == Some(field.path.as_str()) {
                        for axis in 0..*count {
                            let widget = format!("color-slider-{id}-{axis}");
                            self.bindings.insert(
                                ara_ui::source::stable_id(&widget),
                                Binding::Field(field.path.clone(), Some(axis)),
                            );
                            out.push_str(&format!("<slider id='{widget}' min='0' max='1' step='0.01' value='{}' label='{}' action='Dispatch' />",field.value[axis].as_f64().unwrap_or(1.).clamp(0.,1.),["Red","Green","Blue","Alpha"][axis]));
                        }
                    }
                }
                Widget::Bool => {
                    let widget = format!("edit-{id}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&widget),
                        Binding::Field(field.path.clone(), None),
                    );
                    out.push_str(&format!(
                        "<toggle id='{widget}' checked='{}' text='{}' action='Dispatch' />",
                        field.value.as_bool().unwrap_or(false),
                        if field.mixed { "Mixed" } else { "Enabled" }
                    ));
                }
                Widget::Enum { options } => {
                    out.push_str(&format!(
                        "<dropdown id='enum-{id}' text='{}' description='{}' style='height:auto' >",
                        escape(if field.mixed {
                            "Mixed"
                        } else {
                            field.value.as_str().unwrap_or("")
                        }),
                        escape(&field.doc)
                    ));
                    for (option_index, option) in options.iter().enumerate() {
                        let calls = crate::inspector::edit_commands(
                            &editor.state.selection,
                            &field.path,
                            &json!(option),
                        );
                        out.push_str(&self.bind_button(
                            &format!("enum-{id}-{option_index}"),
                            option,
                            Binding::Content(calls, format!("Set {}", field.path)),
                            if field.value == *option {
                                "small selected"
                            } else {
                                "small"
                            },
                        ));
                    }
                    out.push_str("</dropdown>");
                }
                _ => {
                    let widget = format!("edit-{id}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&widget),
                        Binding::Field(field.path.clone(), None),
                    );
                    out.push_str(&format!("<field id='{widget}' text='{}' action='Dispatch' description='{}' label='{}' {} />",escape(&if field.mixed{"—".into()}else{display(&field.value)}),escape(&field.doc),escape(&field.path),if field.read_only{"disabled='true'"}else{""}));
                }
            }
            if let Widget::Number {
                min: Some(min),
                max: Some(max),
                slider: true,
                step,
            } = &field.widget
            {
                let widget = format!("slider-{id}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&widget),
                    Binding::Field(field.path.clone(), None),
                );
                out.push_str(&format!("<slider id='{widget}' min='{min}' max='{max}' step='{step}' value='{}' action='Dispatch' />",field.value.as_f64().unwrap_or(*min).clamp(*min,*max)));
            }
            if let Widget::Asset { .. } | Widget::Entity = &field.widget {
                out.push_str(&self.bind_button(
                    &format!("pick-{id}"),
                    if matches!(field.widget, Widget::Entity) {
                        "Pick from scene"
                    } else {
                        "Pick asset"
                    },
                    Binding::PickField(field.path.clone()),
                    "small quiet",
                ));
            }
            if field.provenance == Provenance::Override {
                let calls=editor.state.selection.iter().map(|entity|json!({"cmd":"prefab.override.revert","args":{"entity":entity,"path":field.path}})).collect();
                out.push_str(&self.bind_button(
                    &format!("revert-{id}"),
                    "Revert override",
                    Binding::Content(calls, "Revert instance override".into()),
                    "small quiet",
                ));
            }
            if let Some(was) = &field.was {
                out.push_str(&format!(
                    "<text id='was-{id}' class='warning' text='{} -> {}' />",
                    escape(&display(was)),
                    escape(&display(&field.value))
                ));
            }
            out.push_str("</panel>");
            self.fields.insert(field.path.clone(), field);
        }
        out.push_str(&self.bind_button(
            "add-component",
            "+ Add component",
            Binding::AddComponent,
            "quiet",
        ));
        out.push_str("</scroll>");
        Ok(())
    }
}
