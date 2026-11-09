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
        let optional =
            field.optional && !matches!(field.widget, Widget::Compound { .. } | Widget::Entity);
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
        // Narrow inspectors put a vector's axis fields on their own line.
        let stacked = self.stacked_rows
            && matches!(field.widget, Widget::Vector { color: false, .. })
            && !(field.value.is_null() && !field.mixed);
        out.push_str(&format!(
            "<panel id='prop-{id}' class='prop-row{}'>",
            if stacked { " prop-row-stacked" } else { "" }
        ));
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
        // An optional property's set/unset toggle leads its value column, so labels
        // stay aligned with every other row.
        let toggle = optional.then(|| {
            let id = format!("override-{id}");
            let binding = if original.value.is_null()
                && field.fallback.is_null()
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
            format!("<toggle id='{id}' class='override-toggle' checked='{}' label='Override {}' description='{} · Uncheck to leave this property unset' action='Dispatch' />", !original.value.is_null(), escape(&sentence(&field.label)), escape(&field.doc))
        });
        let probe_resolution = field.path == "ReflectionProbe.captureResolution";
        let scrub = matches!(field.widget, Widget::Number { .. })
            && !probe_resolution
            && !field.read_only
            && !inactive;
        let tag = if scrub { "button" } else { "panel" };
        if scrub {
            self.bindings.insert(
                ara_ui::source::stable_id(&format!("field-{id}")),
                Binding::Scrub(field.path.clone(), None),
            );
        }
        // Long names end in "…" inside the label column (its 12 + 6 px padding, 12 px
        // text); the tooltip then leads with the full name, as Unity and Unreal do.
        let name = sentence(&field.label);
        let shown = self
            .name_fit
            .lines(&mut self.text, &name, (label - 18.).max(24.), 12., 1)[0]
            .clone();
        let full = if shown == name {
            String::new()
        } else {
            format!("{name} · ")
        };
        out.push_str(&format!("<{tag} id='field-{id}' class='prop-label{}' style='{}' description='{}{}{}' {}><text id='label-{id}' text='{}' /></{tag}><panel id='value-{id}' class='prop-value{}'>",if scrub {" scrubbable"} else {""},if stacked {"width:100%".to_owned()} else {format!("width:{label}px")},escape(&full),escape(&field.doc),if scrub {" · Drag to scrub; Shift: fine, Ctrl: coarse; Esc cancels."} else {""},if scrub {"cursor='ew-resize' action='Dispatch'"} else {""},escape(&shown),if stacked {" prop-value-stacked"} else {""}));
        if let Some(toggle) = toggle {
            out.push_str(&toggle);
        }
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
                count,
                color: true,
                hdr,
                temperature,
                ..
            } => {
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("swatch-{id}")),
                    Binding::ColorPicker(
                        field.path.clone(),
                        super::color::Hints {
                            hdr: *hdr,
                            temperature: *temperature,
                        },
                    ),
                );
                // One long swatch: hex, alpha and HDR intensity live in the picker,
                // and the tooltip reads them out.
                let channel = |n: usize| field.value[n].as_f64().unwrap_or(1.);
                let (base, stops) =
                    super::color::math::split_hdr([channel(0), channel(1), channel(2)]);
                let mut about = if field.mixed {
                    "Mixed colours".to_owned()
                } else {
                    format!(
                        "#{}",
                        super::color::math::format_hex(
                            base.map(|c| super::color::math::linear_to_srgb(c.clamp(0., 1.))),
                            None
                        )
                    )
                };
                if !field.mixed && *count > 3 && channel(3) < 1. {
                    about.push_str(&format!(" · alpha {}", display(&json!(channel(3)))));
                }
                if !field.mixed && stops > 0.005 {
                    about.push_str(&format!(" · {stops:+.1} EV"));
                }
                out.push_str(&format!("<button id='swatch-{id}' class='swatch inspector-swatch' label='Pick colour' description='{} · Click to edit in the colour picker' action='Dispatch' />",escape(&about)));
                out.push_str(&format!(
                    "<text id='unit-{id}' class='inspector-unit unit-slot' text='' />"
                ));
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
                    out.push_str(&format!("<field id='{widget}' class='number vector-input' step='0.01' text='{}' action='Dispatch' label='{} {}' description='{}' />",escape(&value),escape(&field.path),["X","Y","Z","W"][axis],escape(&field.doc)));
                    out.push_str("</panel>");
                }
                // Always present, fixed width: rows with and without units align.
                out.push_str(&format!(
                    "<text id='unit-{id}' class='inspector-unit unit-slot' text='{}' />",
                    escape(unit)
                ));
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
            Widget::Number { .. } if probe_resolution => self.probe_resolution_select(field, out),
            Widget::Enum { options } => {
                let start = out.len();
                self.choice_button(
                    &format!("enum-{id}"),
                    &field.label,
                    choices::Select {
                        destination: choices::Destination::Field(field.path.clone()),
                        options: options
                            .iter()
                            .map(|option| (json!(option), sentence(option)))
                            .collect(),
                        current: if field.mixed {
                            Value::Null
                        } else {
                            field.value.clone()
                        },
                    },
                    out,
                );
                // Inspector selects fill the value column, like fields.
                let markup = out[start..].replacen(
                    "kit-select-compact'",
                    "kit-select-compact kit-select-fill'",
                    1,
                );
                out.truncate(start);
                out.push_str(&markup);
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
                        && crate::asset_tools::thumbnails::has_thumbnail(source) {
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
                out.push_str(&format!("<field id='{widget}' style='flex-grow:1;flex-basis:0px;min-width:0px' text='{}' action='Dispatch' label='{}' description='{}' />",escape(if field.mixed {"—"} else {field.value.as_str().unwrap_or("")}),escape(&field.path),escape(&field.doc)));
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("pick-{id}")),
                    Binding::PickField(field.path.clone()),
                );
                out.push_str(&format!("<button id='pick-{id}' class='ghost icon-button' label='Pick from scene' description='Click an entity in the Scene' action='Dispatch'>{}</button>",icons::icon(&format!("pick-{id}"),"crosshair")));
                if !field.value.is_null() || field.mixed {
                    let button = format!("clear-{id}");
                    self.bindings.insert(
                        ara_ui::source::stable_id(&button),
                        Binding::Content(
                            crate::inspector::edit_commands(
                                &editor.state.selection,
                                &field.path,
                                &Value::Null,
                            ),
                            format!("Clear {}", field.label),
                        ),
                    );
                    out.push_str(&format!("<button id='{button}' class='ghost icon-button' label='Clear reference' action='Dispatch'>{}</button>",icons::icon_in(&button,"x","icon-sm")));
                }
            }
            _ => {
                let widget = format!("edit-{id}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&widget),
                    Binding::Field(field.path.clone(), None),
                );
                // A bounded number is one slider field: it fills to its value, drags
                // across its range and types like any number.
                let slider = matches!(
                    field.widget,
                    Widget::Number {
                        min: Some(_),
                        max: Some(_),
                        slider: true,
                        ..
                    }
                );
                let numeric = matches!(field.widget, Widget::Number { .. });
                let bounds = if let Widget::Number { min, max, step, .. } = field.widget {
                    format!(
                        "step='{step}' {}{}",
                        min.map(|min| format!("min='{min}' ")).unwrap_or_default(),
                        max.map(|max| format!("max='{max}' ")).unwrap_or_default()
                    )
                } else {
                    String::new()
                };
                out.push_str(&format!("<field id='{widget}' class='{}' {bounds} style='{}' text='{}' action='Dispatch' description='{}' label='{}' />",if slider{"number slider"}else if numeric{"number"}else{""},"flex-grow:1;flex-basis:0px;min-width:0px",escape(&if field.mixed{"—".into()}else{display(&field.value)}),escape(&field.doc),escape(&field.path)));
                // Always present, fixed width: rows with and without units align.
                out.push_str(&format!(
                    "<text id='unit-{id}' class='inspector-unit unit-slot' text='{}' />",
                    escape(unit)
                ));
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
