use super::panels::display;
use super::*;
use crate::inspector::{Provenance, Widget};
impl Shell {
    pub(super) fn inspector_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        if editor.state.asset_path.is_some() {
            return self.import_panel(editor, out);
        }
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
        out.push_str(&format!("<panel id='entity-heading' class='row'>{}<text id='entity-title' style='font-size:15px' text='{}' /><panel id='entity-title-fill' style='flex-grow:1' /><text id='selection-count' class='badge' text='{} selected' /></panel><panel id='inspector-divider' class='divider' />",icons::icon("entity-heading","box"),escape(name),editor.state.selection.len()));
        let changes = editor.agent_fields();
        let doc = editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("document not found")?;
        let mut fields = crate::inspector::fields(
            editor.host.store().schema(),
            doc,
            &resolved,
            &editor.state.selection,
            &changes,
        );
        fields.sort_by(|a, b| {
            (!a.path.starts_with("Transform."))
                .cmp(&(!b.path.starts_with("Transform.")))
                .then(a.path.cmp(&b.path))
        });
        let mut component = String::new();
        for field in fields {
            let head = field.path.split('.').next().unwrap_or("");
            if component != head {
                component = head.into();
                out.push_str(&format!("<panel id='divider-{head}' class='divider' /><panel id='component-{head}' class='component'>"));
                out.push_str(&self.bind_button(
                    &format!("collapse-component-{head}"),
                    if self.collapsed_components.contains(head) {
                        "Expand"
                    } else {
                        "Collapse"
                    },
                    Binding::ComponentCollapse(head.into()),
                    "small quiet",
                ));
                out.push_str(&icons::icon(
                    &format!("component-{head}"),
                    if head == "Transform" {
                        "move-3d"
                    } else if head == "Light" {
                        "lightbulb"
                    } else {
                        "box"
                    },
                ));
                out.push_str(&format!("<text id='heading-{head}' class='heading' text='{head}' /><panel id='component-fill-{head}' style='flex-grow:1' />"));
                out.push_str(&self.bind_button(
                    &format!("remove-{head}"),
                    "Component options",
                    Binding::ComponentMenu(head.into()),
                    "small quiet",
                ));
                out.push_str("</panel>");
                if self.component_menu.as_deref() == Some(head) {
                    let calls=editor.state.selection.iter().map(|entity|json!({"cmd":"component.remove","args":{"entity":entity,"component":head}})).collect();
                    out.push_str(&self.bind_button(
                        &format!("component-remove-{head}"),
                        "Remove component",
                        Binding::Content(calls, format!("Remove {head}")),
                        "small destructive",
                    ));
                }
            }
            if self.collapsed_components.contains(head) {
                continue;
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
                        let axis_id = format!("scrub-{id}-{axis}");
                        self.bindings.insert(
                            ara_ui::source::stable_id(&axis_id),
                            Binding::Scrub(field.path.clone(), axis),
                        );
                        out.push_str(&format!("<panel id='axis-cell-{id}-{axis}' class='vector-cell'><button id='{axis_id}' class='axis axis-{}' text='{}' description='Drag to scrub {}. Shift: fine, Ctrl: coarse. Escape cancels.' action='Dispatch' />",["x","y","z","w"][axis],if *color {["R","G","B","A"][axis]}else{["X","Y","Z","W"][axis]},escape(&field.path)));
                        out.push_str(&format!("<field id='{widget}' class='number vector-input' style='flex-grow:1; flex-basis:0px' text='{}' action='Dispatch' label='{} {}' description='{}' />",escape(&value),escape(&field.path),["X","Y","Z","W"][axis],escape(&field.doc)));
                        out.push_str("</panel>");
                    }
                    out.push_str("</panel>");
                    if *color && !field.mixed {
                        let hex = (0..3)
                            .map(|n| {
                                format!(
                                    "{:02X}",
                                    (field.value[n]
                                        .as_f64()
                                        .unwrap_or(1.)
                                        .clamp(0., 1.)
                                        .powf(1. / 2.2)
                                        * 255.)
                                        .round() as u8
                                )
                            })
                            .collect::<String>();
                        out.push_str(&format!(
                            "<text id='color-hex-{id}' class='badge number' text='#{hex}' />"
                        ));
                    }
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
                Widget::Asset { kind } => {
                    let widget = format!("edit-{id}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&widget),
                        Binding::Field(field.path.clone(), None),
                    );
                    let reference = field.value.as_str().unwrap_or("");
                    let name = reference
                        .trim_start_matches('@')
                        .split('#')
                        .next()
                        .and_then(|s| s.rsplit('/').next())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("No asset assigned");
                    out.push_str(&format!("<panel id='asset-field-{id}' class='row'><panel id='asset-thumb-{id}' style='width:36px;height:36px;padding:10px;background:var(--background);border-width:1px;border-color:var(--input);border-radius:var(--radius-sm)'>{}</panel><panel id='asset-field-copy-{id}' style='flex-grow:1;flex-basis:0px;gap:4px;overflow:hidden'><text id='asset-name-{id}' text='{}' style='font-size:12px' /><field id='{widget}' text='{}' action='Dispatch' label='{}' style='height:24px;font-size:10px' /></panel>",icons::icon(&format!("asset-thumb-{id}"),"package"),escape(name),escape(reference),escape(&format!("{kind:?} asset reference"))));
                    out.push_str(&self.bind_button(
                        &format!("pick-{id}"),
                        "Pick asset",
                        Binding::PickField(field.path.clone()),
                        "small",
                    ));
                    out.push_str("</panel>");
                }
                _ => {
                    let widget = format!("edit-{id}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&widget),
                        Binding::Field(field.path.clone(), None),
                    );
                    out.push_str(&format!("<field id='{widget}' class='number' text='{}' action='Dispatch' description='{}' label='{}' {} />",escape(&if field.mixed{"—".into()}else{display(&field.value)}),escape(&field.doc),escape(&field.path),if field.read_only{"disabled='true'"}else{""}));
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
            if let Widget::Entity = &field.widget {
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
            "",
        ));
        out.push_str("</scroll>");
        Ok(())
    }
}
