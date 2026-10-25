//! Inspector v1.5: a two-column property grid (resizable label column, 24 px
//! rows), sentence-case labels with doc tooltips, a 2 px modified marker with ↺,
//! and the right widget per type: checkboxes, enum menus, colour swatches with
//! hex, single-row asset references.
use super::commands::{Target, menus::Context};
use super::panels::display;
use super::*;
use crate::inspector::{Provenance, Widget};

/// `innerAngle` → `Inner angle`.
pub(super) fn sentence(label: &str) -> String {
    let mut out = String::new();
    for (i, c) in label.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else if c == '_' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}
impl Shell {
    /// Label column width in logical pixels for an inspector of `width`.
    pub(super) fn label_px(&self, width: f32) -> f32 {
        (width * self.label_width).clamp(96., (width - 120.).max(96.))
    }
    pub(super) fn inspector_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        if editor.state.focus == "material" {
            return self.material_inspector(editor, out);
        }
        if editor.state.focus == "animation" {
            return self.animation_inspector(editor, out);
        }
        if editor.state.focus == "vfx" {
            return self.vfx_inspector(editor, out);
        }
        if editor.state.asset_path.is_some() {
            return self.import_panel(editor, out);
        }
        let resolved = self.resolved(editor)?;
        self.nested_swatches.clear();
        if editor.state.selection.is_empty() {
            self.empty(
                out,
                "inspector-empty",
                "sliders-horizontal",
                "Nothing selected",
                "Select an entity in the Scene or Hierarchy to edit its components.",
                &[("inspector-empty-select", "Select all", "edit.select_all")],
            );
            return Ok(());
        }
        let width = self
            .ui
            .find("inspector-scroll")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(320.);
        let label = self.label_px(width);
        let first = editor
            .state
            .selection
            .first()
            .and_then(|id| resolved.get(id));
        let name = first
            .and_then(|e| e.record.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Selection");
        let prefab = first.is_some_and(|e| e.source.0.as_str() != editor.state.doc);
        self.bindings.insert(
            ara_ui::source::stable_id("entity-name"),
            Binding::RenameValue,
        );
        out.push_str(&format!("<panel id='inspector-header' class='inspector-header'><panel id='entity-heading' class='row'>{}<field id='entity-name' style='flex-grow:1;flex-basis:0px' text='{}' label='Entity name' description='Rename; Enter applies' action='Dispatch' />",icons::icon("entity-heading",if prefab{"package"}else{"box"}),escape(name)));
        if editor.state.selection.len() > 1 {
            out.push_str(&format!(
                "<text id='selection-count' class='caption' text='{} selected' />",
                editor.state.selection.len()
            ));
        }
        out.push_str("</panel>");
        // Prefab instances (Unity's Overrides bar): which prefab, how many overrides,
        // and Open / Apply all / Revert all.
        if prefab
            && let Some(entity) = editor.state.selection.first()
            && let Some(found) =
                super::prefab_ops::instance(editor.host.store(), &editor.state.doc, entity)
        {
            let file = found
                .prefab_path
                .rsplit('/')
                .next()
                .unwrap_or(&found.prefab_path)
                .to_owned();
            let detail = match (found.inner.is_some(), found.overrides.len()) {
                (true, _) if found.overrides.is_empty() => "inside an instance".to_owned(),
                (_, 0) => "no overrides".to_owned(),
                (_, 1) => "1 override".to_owned(),
                (_, n) => format!("{n} overrides"),
            };
            // The name and count clip in narrow docks; the actions get their own row.
            out.push_str(&format!("<panel id='prefab-bar' class='prefab-bar'><panel id='prefab-head' class='row' style='gap:6px;min-width:0px;overflow:hidden'>{}<text id='prefab-file' class='fg' style='white-space:nowrap;overflow:hidden;min-width:0px;flex-shrink:1' text='{}' /><text id='prefab-overrides' class='caption' style='white-space:nowrap;flex-shrink:0' text='{}' /></panel><panel id='prefab-actions' class='row' style='gap:4px;justify-content:end'>",icons::icon_in("prefab-bar","package","accent"),escape(&file),escape(&detail)));
            for (id, command, label, show) in [
                ("prefab-open", "prefab.open", "Open", true),
                (
                    "prefab-apply",
                    "prefab.apply_all",
                    "Apply all",
                    !found.overrides.is_empty(),
                ),
                (
                    "prefab-revert",
                    "prefab.revert_all",
                    "Revert all",
                    !found.overrides.is_empty(),
                ),
            ] {
                if show {
                    self.bind_command(id, command, super::commands::Target::None);
                    out.push_str(&format!("<button id='{id}' class='notice-action' label='{label}' description='{label}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>"));
                }
            }
            out.push_str("</panel></panel>");
        }
        out.push_str("<panel id='inspector-filter-row' class='row'>");
        self.search(out, "inspector", editor);
        out.push_str("</panel>");
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
        let mut components: Vec<String> = Vec::new();
        for field in &fields {
            let head = field.path.split('.').next().unwrap_or("").to_owned();
            if !components.contains(&head) {
                components.push(head);
            }
        }
        if components.len() > 1 {
            out.push_str("<panel id='jump-chips' class='row' style='gap:2px;overflow:hidden'>");
            for head in &components {
                let id = format!("jump-{head}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Protocol("editor.focus", json!({"panel":"inspector","field":head})),
                );
                out.push_str(&format!("<button id='{id}' class='ghost' style='height:20px;padding-left:6px;padding-right:6px;font-size:11px' label='Jump to {head}' action='Dispatch'><text id='{id}-label' text='{head}' /></button>"));
            }
            out.push_str("</panel>");
        }
        out.push_str("</panel><scroll id='inspector-scroll' style='flex-grow:1;flex-basis:0px;min-height:0px;padding-bottom:12px'>");
        let query = editor
            .state
            .searches
            .get("inspector")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let mut component = String::new();
        for field in fields {
            let head = field.path.split('.').next().unwrap_or("").to_owned();
            if !query.is_empty()
                && !format!("{} {}", field.label, field.path)
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            if component != head {
                component = head.clone();
                let collapsed = self.collapsed_components.contains(&head);
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("collapse-component-{head}")),
                    Binding::ComponentCollapse(head.clone()),
                );
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("component-menu-{head}")),
                    Binding::ContextMenu(Context::Component, Target::Component(head.clone())),
                );
                let icon = match head.as_str() {
                    "Transform" => "move-3d",
                    "Light" => "lightbulb",
                    "Camera" => "camera",
                    "VisualEffect" => "sparkles",
                    "Environment" => "sun",
                    _ => "box",
                };
                out.push_str(&format!("<panel id='component-{head}' class='component-header'><button id='collapse-component-{head}' class='ghost icon-button' label='{} {head}' action='Dispatch'>{}</button>{}<text id='heading-{head}' class='component-title' text='{head}' /><panel id='component-fill-{head}' class='fill' /><button id='component-menu-{head}' class='ghost icon-button' label='{head} options' description='Reset, remove or add components' action='Dispatch'>{}</button></panel>",if collapsed{"Expand"}else{"Collapse"},icons::icon(&format!("collapse-component-{head}"),if collapsed{"chevron-right"}else{"chevron-down"}),icons::icon(&format!("component-{head}"),icon),icons::icon(&format!("component-menu-{head}"),"ellipsis")));
            }
            if self.collapsed_components.contains(&head) {
                continue;
            }
            self.property_row(editor, &field, label, out);
            self.fields.insert(field.path.clone(), field);
        }
        self.bind_command("add-component", "component.add", Target::None);
        out.push_str(&format!("<panel id='add-component-row' style='padding:8px;flex-shrink:0'><button id='add-component' label='Add component' description='Search components to add' action='Dispatch'>{}<text id='add-component-label' text='Add component' /></button></panel></scroll>",icons::icon("add-component","plus")));
        Ok(())
    }
    fn property_row(
        &mut self,
        editor: &Editor,
        field: &crate::inspector::Field,
        label: f32,
        out: &mut String,
    ) {
        let id = field.path.replace('.', "-");
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
        out.push_str(&format!("<panel id='field-{id}' class='prop-label' style='width:{label}px' description='{}'><text id='label-{id}' text='{}' /></panel><panel id='value-{id}' class='prop-value'>",escape(&field.doc),escape(&sentence(&field.label))));
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
                out.push_str(&format!("<button id='swatch-{id}' class='swatch' label='Pick colour' description='Open the colour picker' action='Dispatch' />"));
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
                    out.push_str(&format!("<field id='{alpha}' class='number' style='width:44px' text='{}' label='Alpha' description='Opacity 0–1' action='Dispatch' />",escape(&display(&field.value[3]))));
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
                    out.push_str(&format!("<panel id='axis-cell-{id}-{axis}' class='vector-cell'><button id='{axis_id}' class='axis axis-{}' description='Drag to scrub. Shift: fine, Ctrl: coarse. Esc cancels.' action='Dispatch'><text id='{axis_id}-label' text='{}' /></button>",["x","y","z","w"][axis],["X","Y","Z","W"][axis]));
                    out.push_str(&format!("<field id='{widget}' class='number vector-input' text='{}' action='Dispatch' label='{} {}' description='{}' />",escape(&value),escape(&field.path),["X","Y","Z","W"][axis],escape(&field.doc)));
                    out.push_str("</panel>");
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
                let name = reference
                    .trim_start_matches('@')
                    .split('#')
                    .next()
                    .and_then(|s| s.rsplit('/').next())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("None");
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("asset-field-{id}")),
                    Binding::AssetPicker(field.path.clone(), kind.clone()),
                );
                self.bindings.insert(
                    ara_ui::source::stable_id(&format!("pick-{id}")),
                    Binding::PickField(field.path.clone()),
                );
                out.push_str(&format!("<button id='asset-field-{id}' style='flex-grow:1;flex-basis:0px;min-width:0px;overflow:hidden;justify-content:start;background:var(--background);border-color:var(--input)' label='{}' description='{} · drop an asset here, or pick one' action='Dispatch'>{}<text id='asset-name-{id}' class='{}' style='white-space:nowrap;overflow:hidden;min-width:0px;flex-shrink:1' text='{}' /></button>",escape(&format!("{kind:?} asset")),escape(reference),icons::icon(&format!("asset-field-{id}"),"package"),if reference.is_empty(){"muted"}else{"fg"},escape(name)));
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
                out.push_str(&format!("<field id='{widget}' style='flex-grow:1;flex-basis:0px' text='{}' action='Dispatch' label='{}' />",escape(&display(&field.value)),escape(&field.path)));
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
                    out.push_str(&format!("<slider id='{slider}' style='flex-grow:1;flex-basis:0px' min='{min}' max='{max}' step='{step}' value='{}' action='Dispatch' />",field.value.as_f64().unwrap_or(*min).clamp(*min,*max)));
                }
                let numeric = matches!(field.widget, Widget::Number { .. });
                out.push_str(&format!("<field id='{widget}' class='{}' style='{}' text='{}' action='Dispatch' description='{}' label='{}' {} />",if numeric{"number"}else{""},if numeric && matches!(field.widget, Widget::Number{slider:true,..}){"width:56px"}else{"flex-grow:1;flex-basis:0px"},escape(&if field.mixed{"—".into()}else{display(&field.value)}),escape(&field.doc),escape(&field.path),if field.read_only{"disabled='true'"}else{""}));
                if !unit.is_empty() {
                    out.push_str(&format!(
                        "<text id='unit-{id}' class='caption' text='{}' />",
                        escape(unit)
                    ));
                }
            }
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
impl Shell {
    /// Start resizing the label column when the pointer is on its edge.
    pub(crate) fn begin_column_drag(&mut self) -> bool {
        let Some(i) = self.shown_node("inspector-scroll") else {
            return false;
        };
        let r = self.ui.nodes[i].rect;
        let edge = r.x + self.label_px(r.w);
        if r.contains(self.pointer[0], self.pointer[1]) && (self.pointer[0] - edge).abs() <= 4. {
            self.drag = Some(super::Drag::Column {
                start: self.pointer,
            });
            return true;
        }
        false
    }
    /// Follow the pointer while resizing the label column.
    pub(crate) fn drag_column(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(super::Drag::Column { .. }) = &self.drag else {
            return Ok(false);
        };
        let Some(i) = self.shown_node("inspector-scroll") else {
            return Ok(false);
        };
        let r = self.ui.nodes[i].rect;
        let ratio = ((self.pointer[0] - r.x) / r.w.max(1.)).clamp(0.2, 0.7);
        if (ratio - self.label_width).abs() < 0.004 {
            return Ok(false);
        }
        self.label_width = ratio;
        self.rebuild(editor)?;
        Ok(true)
    }
}
