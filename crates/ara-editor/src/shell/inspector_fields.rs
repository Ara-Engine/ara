//! Compact schema-driven field rows with typed bindings.
use super::commands::Target;
use super::inspector_panel::sentence;
use super::panels::display;
use super::*;
use crate::inspector::{Provenance, Widget};

impl Shell {
    pub(super) fn property_row(
        &mut self,
        editor: &Editor,
        field: &crate::inspector::Field,
        label: f32,
        out: &mut String,
    ) {
        let row_start = out.len();
        let id = field.path.replace('.', "-");
        let optional = field.optional && !matches!(field.widget, Widget::Compound { .. });
        let inactive = optional && field.value.is_null() && !field.mixed;
        let fallback = inactive.then(|| {
            let mut fallback = field.clone();
            fallback.value = field.fallback.clone();
            fallback.channels = field.fallback.as_array().cloned().unwrap_or_default();
            fallback
        });
        let original = field;
        let field = fallback.as_ref().unwrap_or(field);
        let modified = matches!(
            field.provenance,
            Provenance::Authored | Provenance::Override | Provenance::AgentChanged
        );
        out.push_str(&format!("<panel id='prop-{id}' class='prop-row'>"));
        if modified {
            out.push_str(&format!(
                "<panel id='mark-{id}' class='prop-marker{}' />",
                if field.provenance == Provenance::AgentChanged {
                    " marker-agent"
                } else {
                    ""
                }
            ));
        }
        let unit = field.unit.as_deref().unwrap_or("");
        let label = if optional {
            let id = format!("override-{id}");
            let binding = if original.value.is_null()
                && let Widget::Asset { kind } = &field.widget
            {
                Binding::AssetPicker(field.path.clone(), kind.clone())
            } else {
                Binding::Content(
                    crate::inspector::edit_commands(
                        &editor.state.selection,
                        &field.path,
                        &if original.value.is_null() {
                            field.fallback.clone()
                        } else {
                            Value::Null
                        },
                    ),
                    format!("Toggle {} override", field.label),
                )
            };
            self.bindings
                .insert(ara_ui::source::stable_id(&id), binding);
            out.push_str(&format!("<panel id='{id}-gutter' class='override-gutter'><toggle id='{id}' class='override-toggle' checked='{}' label='Override {}' description='{} · Uncheck to leave this property unset' action='Dispatch' /></panel>", !original.value.is_null(), escape(&sentence(&field.label)), escape(&field.doc)));
            (label - 22.).max(48.)
        } else {
            label
        };
        let scrub = matches!(field.widget, Widget::Number { .. }) && !field.read_only && !inactive;
        let tag = if scrub { "button" } else { "panel" };
        if scrub {
            self.bindings.insert(
                ara_ui::source::stable_id(&format!("field-{id}")),
                Binding::Scrub(field.path.clone(), None),
            );
        }
        out.push_str(&format!("<{tag} id='field-{id}' class='prop-label{}' style='width:{label}px' description='{}{}' {}><text id='label-{id}' text='{}' /></{tag}><panel id='value-{id}' class='prop-value'>",if scrub {" scrubbable"} else {""},escape(&field.doc),if scrub {" · Drag to scrub; Shift: fine, Ctrl: coarse; Esc cancels."} else {""},if scrub {"cursor='ew-resize' action='Dispatch'"} else {""},escape(&sentence(&field.label))));
        let value_start = out.len();
        match &field.widget {
            // An unset optional vector (`emissiveFactor`): None until set.
            Widget::Vector { count, color, .. } if field.value.is_null() && !field.mixed => {
                let start = json!(vec![if *color { 1. } else { 0. }; *count]);
                let button = format!("set-{id}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&button),
                    Binding::Content(
                        crate::inspector::edit_commands(
                            &editor.state.selection,
                            &field.path,
                            &start,
                        ),
                        format!("Set {}", sentence(&field.label).to_lowercase()),
                    ),
                );
                out.push_str(&format!("<text id='none-{id}' class='caption fill' text='None' /><button id='{button}' class='ghost icon-button' label='Set' description='Set a value' action='Dispatch'>{}</button>",icons::icon_in(&button,"plus","icon-sm")));
            }
            Widget::Vector {
                count, color: true, ..
            } => {
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("swatch-{id}")),
                    Binding::ColorPicker(field.path.clone()),
                );
                out.push_str(&format!("<button id='swatch-{id}' class='swatch inspector-swatch' label='Pick colour' description='Open the colour picker' action='Dispatch' />"));
                let hex = if field.mixed {
                    "—".to_owned()
                } else {
                    format!(
                        "#{}",
                        (0..3)
                            .map(|n| format!(
                                "{:02X}",
                                (field.value[n]
                                    .as_f64()
                                    .unwrap_or(1.)
                                    .clamp(0., 1.)
                                    .powf(1. / 2.2)
                                    * 255.)
                                    .round() as u8
                            ))
                            .collect::<String>()
                    )
                };
                let widget = format!("edit-{id}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&widget),
                    Binding::Field(field.path.clone(), None),
                );
                out.push_str(&format!("<field id='{widget}' class='mono' style='flex-grow:1;flex-basis:0px' text='{}' label='{} hex' description='sRGB hex; alpha keeps its value' action='Dispatch' />",escape(&hex),escape(&field.path)));
                if *count > 3 {
                    let alpha = format!("edit-{id}-3");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&alpha),
                        Binding::Field(field.path.clone(), Some(3)),
                    );
                    let alpha_value = field
                        .channels
                        .get(3)
                        .filter(|v| !v.is_null())
                        .map(display)
                        .unwrap_or_else(|| "—".into());
                    out.push_str(&format!("<field id='{alpha}' class='number' style='width:44px' text='{}' label='Alpha' description='Opacity 0–1' action='Dispatch' />",escape(&alpha_value)));
                }
            }
            Widget::Vector {
                count,
                color: false,
                ..
            } => {
                for axis in 0..*count {
                    let widget = format!("edit-{id}-{axis}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&widget),
                        Binding::Field(field.path.clone(), Some(axis)),
                    );
                    let value = field
                        .channels
                        .get(axis)
                        .filter(|v| !v.is_null())
                        .map(display)
                        .unwrap_or_else(|| "—".into());
                    let axis_id = format!("scrub-{id}-{axis}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&axis_id),
                        Binding::Scrub(field.path.clone(), Some(axis)),
                    );
                    out.push_str(&format!("<panel id='axis-cell-{id}-{axis}' class='vector-cell'><button id='{axis_id}' class='axis axis-{}' cursor='ew-resize' label='Scrub {} {}' description='Drag to scrub. Shift: fine, Ctrl: coarse. Esc cancels.' action='Dispatch'><text id='{axis_id}-label' text='{}' /></button>",["x","y","z","w"][axis],escape(&sentence(&field.label)),["X","Y","Z","W"][axis],["X","Y","Z","W"][axis]));
                    out.push_str(&format!("<field id='{widget}' class='number vector-input' text='{}' action='Dispatch' label='{} {}' description='{}' />",escape(&value),escape(&field.path),["X","Y","Z","W"][axis],escape(&field.doc)));
                    out.push_str("</panel>");
                }
                if !unit.is_empty() {
                    out.push_str(&format!(
                        "<text id='unit-{id}' class='inspector-unit' text='{}' />",
                        escape(unit)
                    ));
                }
            }
            Widget::Bool => {
                let widget = format!("edit-{id}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&widget),
                    Binding::Field(field.path.clone(), None),
                );
                out.push_str(&format!(
                    "<toggle id='{widget}' checked='{}' label='{}' action='Dispatch' />",
                    field.value.as_bool().unwrap_or(false),
                    escape(&field.label)
                ));
                if field.mixed {
                    out.push_str(&format!(
                        "<text id='mixed-{id}' class='caption' text='Mixed' />"
                    ));
                }
            }
            Widget::Enum { options } => {
                let current = if field.mixed {
                    "—".to_owned()
                } else {
                    field.value.as_str().unwrap_or("").to_owned()
                };
                if options.len() <= 3 && options.iter().map(String::len).sum::<usize>() < 26 {
                    out.push_str(&format!(
                        "<panel id='enum-{id}' class='segment' style='flex-grow:1;flex-basis:0px'>"
                    ));
                    for (n, option) in options.iter().enumerate() {
                        let calls = crate::inspector::edit_commands(
                            &editor.state.selection,
                            &field.path,
                            &json!(option),
                        );
                        let button = format!("enum-{id}-{n}");
                        self.bindings.insert(
                            ara_ui::source::stable_id(&button),
                            Binding::Content(calls, format!("Set {}", field.label)),
                        );
                        out.push_str(&format!("<button id='{button}' class='segment-button{}' style='flex-grow:1' label='{}' action='Dispatch'><text id='{button}-label' text='{}' /></button>",if current==*option{" selected"}else{""},escape(option),escape(&sentence(option))));
                    }
                    out.push_str("</panel>");
                } else {
                    let button = format!("enum-{id}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&button),
                        Binding::EnumMenu(field.path.clone(), options.clone()),
                    );
                    out.push_str(&format!("<button id='{button}' style='flex-grow:1;flex-basis:0px;justify-content:start' label='{}' description='{}' action='Dispatch'><text id='{button}-value' class='fill' text='{}' />{}</button>",escape(&field.label),escape(&field.doc),escape(&sentence(&current)),icons::icon_in(&button,"chevron-down","icon-sm")));
                }
            }
            Widget::Asset { kind } => {
                let reference = field.value.as_str().unwrap_or("");
                // Without its extension, like Assets; the tooltip has the reference.
                let source = reference.trim_start_matches('@').split('#').next();
                let name = source
                    .map(|s| names::stem(names::file_name(s)))
                    .filter(|s| !s.is_empty())
                    .unwrap_or(if field.mixed { "—" } else { "None" });
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("asset-field-{id}")),
                    Binding::AssetPicker(field.path.clone(), kind.clone()),
                );
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("pick-{id}")),
                    Binding::PickField(field.path.clone()),
                );
                let preview = source.and_then(|source| self.thumbnail_slots.get(source)).map(|slot| format!("<image id='asset-field-{id}-thumbnail' src='{}' style='width:20px;height:20px' />", crate::asset_tools::thumbnails::image_name(*slot))).unwrap_or_else(|| {
                    if let Some(source) = source.filter(|source| !source.is_empty())
                        && matches!(source.rsplit('.').next(), Some("material" | "graph" | "png" | "jpg" | "jpeg" | "hdr" | "exr" | "glb" | "gltf" | "fbx")) {
                            self.thumbnails.request(source);
                        }
                    icons::icon(&format!("asset-field-{id}"), "package")
                });
                out.push_str(&format!("<button id='asset-field-{id}' class='asset-reference' label='{} asset' description='{} · {} · drop an asset here, or pick one' action='Dispatch'>{preview}<text id='asset-name-{id}' class='reference-name {}' text='{}' />{}</button>",escape(kind.as_deref().unwrap_or("Any")),escape(&field.doc),escape(reference),if reference.is_empty(){"muted"}else{"fg"},escape(name),icons::icon_in(&format!("asset-picker-{id}"),"chevron-down","icon-sm")));
                out.push_str(&format!("<button id='pick-{id}' class='ghost icon-button' label='Pick asset' description='Pick in Assets' action='Dispatch'>{}</button>",icons::icon(&format!("pick-{id}"),"crosshair")));
                if !reference.is_empty() {
                    let calls = crate::inspector::edit_commands(
                        &editor.state.selection,
                        &field.path,
                        &Value::Null,
                    );
                    self.bindings.insert(
                        ara_ui::source::stable_id(&format!("clear-{id}")),
                        Binding::Content(calls, format!("Clear {}", field.label)),
                    );
                    out.push_str(&format!("<button id='clear-{id}' class='ghost icon-button' label='Clear' description='Clear the reference' action='Dispatch'>{}</button>",icons::icon(&format!("clear-{id}"),"x")));
                }
            }
            Widget::Compound { .. } => self.compound_head(editor, field, &id, out),
            Widget::Entity => {
                let widget = format!("edit-{id}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&widget),
                    Binding::Field(field.path.clone(), None),
                );
                out.push_str(&format!("<field id='{widget}' style='flex-grow:1;flex-basis:0px;min-width:0px' text='{}' action='Dispatch' label='{}' description='{}' />",escape(&if field.mixed {"—".into()} else {display(&field.value)}),escape(&field.path),escape(&field.doc)));
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("pick-{id}")),
                    Binding::PickField(field.path.clone()),
                );
                out.push_str(&format!("<button id='pick-{id}' class='ghost icon-button' label='Pick from scene' description='Click an entity in the Scene' action='Dispatch'>{}</button>",icons::icon(&format!("pick-{id}"),"crosshair")));
            }
            _ => {
                let widget = format!("edit-{id}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&widget),
                    Binding::Field(field.path.clone(), None),
                );
                if let Widget::Number {
                    min: Some(min),
                    max: Some(max),
                    slider: true,
                    step,
                } = &field.widget
                {
                    let slider = format!("slider-{id}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&slider),
                        Binding::Field(field.path.clone(), None),
                    );
                    out.push_str(&format!("<slider id='{slider}' style='flex-grow:1;flex-basis:0px;min-width:24px' min='{min}' max='{max}' step='{step}' value='{}' label='{}' description='{}' action='Dispatch' />",field.value.as_f64().unwrap_or((*min+*max)*0.5).clamp(*min,*max),escape(&sentence(&field.label)),escape(&field.doc)));
                }
                let numeric = matches!(field.widget, Widget::Number { .. });
                out.push_str(&format!("<field id='{widget}' class='{}' style='{}' text='{}' action='Dispatch' description='{}' label='{}' />",if numeric{"number"}else{""},if numeric && matches!(field.widget, Widget::Number{slider:true,..}){"width:56px"}else{"flex-grow:1;flex-basis:0px"},escape(&if field.mixed{"—".into()}else{display(&field.value)}),escape(&field.doc),escape(&field.path)));
                if !unit.is_empty() {
                    out.push_str(&format!(
                        "<text id='unit-{id}' class='inspector-unit' text='{}' />",
                        escape(unit)
                    ));
                }
            }
        }
        if inactive && !matches!(field.widget, Widget::Asset { .. } | Widget::Entity) {
            let disabled = out[value_start..]
                .replace("action='Dispatch'", "disabled='true' action='Dispatch'");
            out.truncate(value_start);
            out.push_str(&disabled);
        }
        out.push_str("</panel>");
        if modified {
            self.bind_command(
                &format!("reset-{id}"),
                "field.reset",
                Target::Field(field.path.clone()),
            );
            out.push_str(&format!("<button id='reset-{id}' class='ghost prop-reset' label='Reset {}' description='Reset to the default or prefab value' action='Dispatch'>{}</button>",escape(&field.label),icons::icon_in(&format!("reset-{id}"),"undo-2","icon-sm")));
        } else {
            out.push_str(&format!(
                "<panel id='reset-space-{id}' style='width:18px;flex-shrink:0' />"
            ));
        }
        out.push_str("</panel>");
        if field.read_only {
            let disabled =
                out[row_start..].replace("action='Dispatch'", "disabled='true' action='Dispatch'");
            out.truncate(row_start);
            out.push_str(&disabled.replace("disabled='true' disabled='true'", "disabled='true'"));
        }
        self.compound_rows(editor, field, label, out);
        if let Widget::Vector {
            count, color: true, ..
        } = &field.widget
            && self.color_picker.as_deref() == Some(field.path.as_str())
        {
            for axis in 0..*count {
                let widget = format!("color-slider-{id}-{axis}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&widget),
                    Binding::Field(field.path.clone(), Some(axis)),
                );
                out.push_str(&format!("<panel id='picker-{id}-{axis}' class='prop-row'><panel id='picker-label-{id}-{axis}' class='prop-label' style='width:{label}px'><text id='picker-name-{id}-{axis}' text='{}' /></panel><panel id='picker-value-{id}-{axis}' class='prop-value'><slider id='{widget}' style='flex-grow:1' min='0' max='1' step='0.01' value='{}' label='{}' action='Dispatch' /></panel></panel>",["Red","Green","Blue","Alpha"][axis],field.value[axis].as_f64().unwrap_or(1.).clamp(0.,1.),["Red","Green","Blue","Alpha"][axis]));
            }
        }
        if self.pick_field.as_deref() == Some(field.path.as_str()) {
            let what = match &field.widget {
                Widget::Asset { kind } => {
                    super::assets::field_accepts(kind.as_deref(), &field.path).1
                }
                _ => "an entity in the Scene or Hierarchy",
            };
            out.push_str(&format!("<panel id='picking-row-{id}' class='row' style='padding-left:{}px;min-height:18px'><text id='picking-{id}' class='chip-accent' text='Click {what} to use it · Esc cancels' /></panel>",label+4.));
        }
        if let Some(was) = &field.was {
            out.push_str(&format!("<panel id='was-row-{id}' class='row' style='padding-left:{}px;min-height:18px'><text id='was-{id}' class='chip-warning' text='{} → {}' /></panel>",label+4.,escape(&display(was)),escape(&display(&field.value))));
        }
    }
}
