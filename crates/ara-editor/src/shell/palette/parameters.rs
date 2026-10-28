//! Reflected command parameters are native controls; only Run submits authoring commands.
use super::*;

pub(crate) const FORM_CSS: &str = "
.palette-form-title { white-space:normal;flex-shrink:0; }
.palette-form-scroll { min-width:0px;min-height:0px;flex-basis:auto;flex-shrink:1;gap:8px;padding-right:12px; }
.palette-parameter { min-width:0px;flex-shrink:0;gap:3px; }
.palette-parameter-label { font-size:12px;white-space:normal; }
.palette-parameter-control { width:100%;max-width:100%;min-width:0px; }
.palette-vector { gap:4px;min-width:0px; }
.palette-axis { flex-direction:row;gap:4px;min-width:0px; }
.palette-axis-label { width:16px;flex-shrink:0; }
.palette-axis-input { min-width:0px;flex-grow:1;flex-basis:0px; }
.palette-parameter-help { white-space:normal;font-size:11px;color:var(--muted-foreground); }
.palette-parameter-error { white-space:normal;font-size:11px;color:var(--destructive); }
.palette-form-actions { flex-direction:row;flex-wrap:wrap;min-width:0px;flex-shrink:0;gap:4px; }
.palette-form-button { height:auto;min-height:28px;max-width:100%;white-space:normal; }
";

#[derive(Clone)]
enum Kind {
    Choice(Vec<(Value, String)>),
    Boolean,
    Number {
        integer: bool,
        min: Option<f64>,
        max: Option<f64>,
        step: f64,
    },
    Vector {
        count: usize,
        integer: bool,
        min: Option<f64>,
        max: Option<f64>,
        step: f64,
    },
    String,
    Json(Option<String>),
}
#[derive(Clone)]
struct Parameter {
    key: String,
    label: String,
    description: String,
    kind: Kind,
    required: bool,
    default: Option<Value>,
}
impl Parameter {
    fn control_id(&self) -> String {
        if matches!(self.kind, Kind::Vector { .. }) {
            format!("param-{}-0", self.key)
        } else {
            format!("param-{}", self.key)
        }
    }
}
/// One visible form's drafts and one-shot focus request; rebuilding is not re-entry.
#[derive(Default)]
pub(crate) struct State {
    command: Option<String>,
    fields: Vec<Parameter>,
    drafts: BTreeMap<String, String>,
    errors: BTreeMap<String, String>,
    error: Option<String>,
    focus: Option<String>,
    generation: Option<String>,
}

fn shape<'a>(root: &'a Value, mut value: &'a Value) -> &'a Value {
    for _ in 0..32 {
        if let Some(reference) = value
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|path| path.strip_prefix('#'))
            && let Some(target) = root.pointer(reference)
        {
            value = target;
            continue;
        }
        if let Some(target) = value
            .get("anyOf")
            .and_then(Value::as_array)
            .and_then(|options| {
                options
                    .iter()
                    .find(|value| value.get("type").and_then(Value::as_str) != Some("null"))
            })
        {
            value = target;
            continue;
        }
        break;
    }
    value
}
fn parameters(schema: &Value) -> Vec<Parameter> {
    let required = schema.get("required").and_then(Value::as_array);
    schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(key, _)| *key != "session")
        .map(|(key, property)| {
            let resolved = shape(schema, property);
            let choices = resolved
                .get("oneOf")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(|option| option.get("const"))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .or_else(|| resolved.get("enum").and_then(Value::as_array).cloned())
                .unwrap_or_default();
            let ty = resolved.get("type").and_then(|value| {
                value.as_str().or_else(|| {
                    value.as_array().and_then(|types| {
                        types
                            .iter()
                            .filter_map(Value::as_str)
                            .find(|ty| *ty != "null")
                    })
                })
            });
            let number = |key: &str| {
                property
                    .get(key)
                    .or_else(|| resolved.get(key))
                    .and_then(Value::as_f64)
                    .filter(|value| value.is_finite())
            };
            let vector = resolved
                .get("minItems")
                .and_then(Value::as_u64)
                .zip(resolved.get("maxItems").and_then(Value::as_u64))
                .filter(|(min, max)| min == max && (2..=4).contains(min));
            let item = resolved.get("items").map(|item| shape(schema, item));
            let kind = if !choices.is_empty() {
                Kind::Choice(
                    choices
                        .into_iter()
                        .map(|value| {
                            let label = value
                                .as_str()
                                .map(inspector_panel::sentence)
                                .unwrap_or_else(|| value.to_string());
                            (value, label)
                        })
                        .collect(),
                )
            } else {
                match ty {
                    Some("boolean") => Kind::Boolean,
                    Some("number" | "integer") => Kind::Number {
                        integer: ty == Some("integer"),
                        min: number("minimum"),
                        max: number("maximum"),
                        step: number("multipleOf")
                            .filter(|step| *step > 0.)
                            .unwrap_or(if ty == Some("integer") { 1. } else { 0.1 }),
                    },
                    Some("string") => Kind::String,
                    Some("array")
                        if vector.is_some()
                            && item.is_some_and(|item| {
                                matches!(
                                    item.get("type").and_then(Value::as_str),
                                    Some("number" | "integer")
                                )
                            }) =>
                    {
                        let item = item.expect("numeric vector item checked");
                        let integer = item.get("type").and_then(Value::as_str) == Some("integer");
                        Kind::Vector {
                            count: vector.map_or(0, |(count, _)| count as usize),
                            integer,
                            min: item.get("minimum").and_then(Value::as_f64),
                            max: item.get("maximum").and_then(Value::as_f64),
                            step: if integer { 1. } else { 0.1 },
                        }
                    }
                    other => Kind::Json(other.map(str::to_owned)),
                }
            };
            Parameter {
                key: key.clone(),
                label: inspector_panel::sentence(key),
                description: property
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                kind,
                required: required.is_some_and(|required| {
                    required
                        .iter()
                        .any(|value| value.as_str() == Some(key.as_str()))
                }),
                default: property
                    .get("default")
                    .filter(|value| !value.is_null())
                    .cloned(),
            }
        })
        .collect()
}
fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => ara_json::to_canonical_string(other),
    }
}
fn number_value(
    text: &str,
    integer: bool,
    min: Option<f64>,
    max: Option<f64>,
) -> Result<Value, String> {
    let value =
        serde_json::from_str::<Value>(text).map_err(|_| "Enter a finite number".to_owned())?;
    let number = value
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or("Enter a finite number")?;
    if min.is_some_and(|min| number < min) || max.is_some_and(|max| number > max) {
        return Err("The number is outside the allowed range".into());
    }
    if !integer || value.as_i64().is_some() || value.as_u64().is_some() {
        return Ok(value);
    }
    if number.fract() != 0. {
        return Err("Enter a whole number".into());
    }
    serde_json::from_str::<Value>(&format!("{number:.0}"))
        .map_err(|_| "The whole number is too large".to_owned())
        .and_then(|value| {
            if value.as_i64().is_some() || value.as_u64().is_some() {
                Ok(value)
            } else {
                Err("The whole number is too large".into())
            }
        })
}

impl Shell {
    pub(in crate::shell) fn begin_palette_form(
        &mut self,
        editor: &Editor,
        command: &str,
    ) -> ara_ui::Result<()> {
        self.input.cancel_interaction(&mut self.ui);
        if self.palette_form.command.as_deref() != Some(command) {
            let schema = editor
                .host
                .catalog()
                .iter()
                .find(|method| method.name == command)
                .map(|method| &method.params)
                .or_else(|| editor.command_schemas.get(command))
                .ok_or("This command no longer has a parameter schema")?;
            self.palette_form.command = Some(command.into());
            self.palette_form.fields = parameters(schema);
            self.palette_form.drafts.clear();
            self.palette_form.errors.clear();
            self.palette_form.error = None;
            self.palette_params.clear();
            for field in &self.palette_form.fields {
                let initial = field.default.clone().or_else(|| match field.key.as_str() {
                    "entity" => editor.state.selection.first().cloned().map(Value::String),
                    "entities" if !editor.state.selection.is_empty() => {
                        Some(json!(editor.state.selection))
                    }
                    _ if field.required && matches!(field.kind, Kind::Boolean) => {
                        Some(json!(false))
                    }
                    _ => None,
                });
                if let Some(value) = initial {
                    self.palette_params.insert(field.key.clone(), value);
                }
            }
        }
        self.palette_command = Some(command.into());
        self.palette_form.focus = Some(
            self.palette_form
                .fields
                .first()
                .map_or_else(|| "palette-run".into(), Parameter::control_id),
        );
        Ok(())
    }

    pub(in crate::shell) fn back_palette_form(&mut self) {
        self.input.cancel_interaction(&mut self.ui);
        self.palette_command = None;
        self.palette_form.focus = Some("palette-search".into());
    }

    pub(super) fn palette_parameters(
        &mut self,
        editor: &Editor,
        name: &str,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        if self.palette_form.command.as_deref() != Some(name) {
            self.begin_palette_form(editor, name)?;
        }
        out.push_str(&format!("<text id='palette-command' class='accent palette-form-title' text='{}' /><scroll id='palette-form-scroll' class='palette-form-scroll'>",escape(name)));
        let fields = std::mem::take(&mut self.palette_form.fields);
        for field in &fields {
            let key = &field.key;
            let id = format!("param-{key}");
            out.push_str(&format!("<panel id='param-row-{key}' class='palette-parameter'><text id='param-label-{key}' class='palette-parameter-label' text='{}{}' /><panel id='param-value-{key}' style='min-width:0px'>",escape(&field.label),if field.required{" *"}else{""}));
            let value = self
                .palette_params
                .get(key)
                .cloned()
                .or_else(|| field.default.clone())
                .unwrap_or(Value::Null);
            match &field.kind {
                Kind::Choice(options) => {
                    let mut options = options.clone();
                    if value.is_null() {
                        options.insert(
                            0,
                            (
                                Value::Null,
                                if field.required {
                                    "Choose…"
                                } else {
                                    "Use default"
                                }
                                .into(),
                            ),
                        );
                    }
                    self.choice_button(
                        &id,
                        &field.label,
                        choices::Select {
                            destination: choices::Destination::PaletteParameter {
                                command: name.into(),
                                key: key.clone(),
                            },
                            options,
                            current: value,
                        },
                        out,
                    );
                }
                Kind::Boolean => {
                    self.bindings.insert(
                        ara_ui::source::stable_id(&id),
                        Binding::PaletteParam(key.clone()),
                    );
                    let inherited = !field.required && value.is_null();
                    out.push_str(&format!("<toggle id='{id}' label='{}{}' checked='{}' description='{}{}' action='Dispatch' style='width:28px;min-height:28px' />",escape(&field.label),if inherited {" (using command default)"}else{""},value.as_bool().unwrap_or(false),escape(&field.description),if inherited {" · Using the command default; omitted until edited"}else{""}));
                }
                Kind::Vector {
                    count,
                    min,
                    max,
                    step,
                    ..
                } => {
                    out.push_str(&format!("<panel id='{id}-axes' class='palette-vector'>"));
                    for axis in 0..*count {
                        let axis_id = format!("{id}-{axis}");
                        let key = format!("{key}#{axis}");
                        self.bindings.insert(
                            ara_ui::source::stable_id(&axis_id),
                            Binding::PaletteParam(key.clone()),
                        );
                        let text =
                            self.palette_form
                                .drafts
                                .get(&key)
                                .cloned()
                                .unwrap_or_else(|| {
                                    value.get(axis).map(display_value).unwrap_or_default()
                                });
                        let label = ["X", "Y", "Z", "W"][axis];
                        out.push_str(&format!("<panel id='{axis_id}-row' class='palette-axis'><text id='{axis_id}-label' class='palette-axis-label' text='{label}' /><field id='{axis_id}' class='number palette-axis-input' step='{step}' {} {} text='{}' label='{} {label}' description='{}' action='Dispatch' /></panel>",min.map(|value|format!("min='{value}'")).unwrap_or_default(),max.map(|value|format!("max='{value}'")).unwrap_or_default(),escape(&text),escape(&field.label),escape(&field.description)));
                    }
                    out.push_str("</panel>");
                }
                kind => {
                    self.bindings.insert(
                        ara_ui::source::stable_id(&id),
                        Binding::PaletteParam(key.clone()),
                    );
                    let text = self
                        .palette_form
                        .drafts
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| {
                            if matches!(kind, Kind::Json(_)) && !value.is_null() {
                                ara_json::to_canonical_string(&value)
                            } else {
                                display_value(&value)
                            }
                        });
                    let numeric = if let Kind::Number { min, max, step, .. } = kind {
                        format!(
                            "class='palette-parameter-control number' step='{step}' {} {}",
                            min.map(|value| format!("min='{value}'"))
                                .unwrap_or_default(),
                            max.map(|value| format!("max='{value}'"))
                                .unwrap_or_default()
                        )
                    } else {
                        "class='palette-parameter-control'".into()
                    };
                    out.push_str(&format!("<field id='{id}' {numeric} text='{}' label='{}' description='{}' action='Dispatch' />",escape(&text),escape(&field.label),escape(&field.description)));
                }
            }
            out.push_str("</panel>");
            if !field.required && !self.palette_params.contains_key(key) {
                out.push_str(&format!("<text id='param-default-{key}' class='palette-parameter-help' text='Using the command default; omitted until edited' />"));
            }
            if let Kind::Json(ty) = &field.kind {
                out.push_str(&format!(
                    "<text id='param-format-{key}' class='palette-parameter-help' text='{}' />",
                    escape(&format!(
                        "Structured value: enter JSON{}",
                        ty.as_deref().map(|ty| format!(" {ty}")).unwrap_or_default()
                    ))
                ));
            }
            if !field.required {
                out.push_str(&self.bind_button(
                    &format!("param-unset-{key}"),
                    "Use default",
                    Binding::PaletteUnset(key.clone()),
                    "ghost",
                ));
            }
            if let Some(error) = self.palette_form.errors.get(key) {
                out.push_str(&format!(
                    "<text id='param-error-{key}' class='palette-parameter-error' text='{}' />",
                    escape(error)
                ));
            }
            out.push_str("</panel>");
        }
        self.palette_form.fields = fields;
        out.push_str("</scroll>");
        if let Some(error) = &self.palette_form.error {
            out.push_str(&format!(
                "<text id='palette-form-error' class='palette-parameter-error' text='{}' />",
                escape(error)
            ));
        }
        out.push_str("<panel id='palette-form-actions' class='palette-form-actions'>");
        out.push_str(&self.bind_button(
            "palette-back",
            "Back",
            Binding::PaletteBack,
            "ghost palette-form-button",
        ));
        out.push_str(&self.bind_button(
            "palette-run",
            "Run command",
            Binding::PaletteRun,
            "primary palette-form-button",
        ));
        out.push_str("</panel>");
        Ok(())
    }

    pub(in crate::shell) fn edit_palette_parameter(&mut self, key: &str, value: Value) {
        if let Some((field, axis)) = key
            .rsplit_once('#')
            .and_then(|(field, axis)| axis.parse::<usize>().ok().map(|axis| (field, axis)))
        {
            self.edit_palette_axis(field, axis, value);
            return;
        }
        let Some(field) = self
            .palette_form
            .fields
            .iter()
            .find(|field| field.key == key)
        else {
            return;
        };
        let text = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        let parsed = match &field.kind {
            Kind::Boolean => value
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| "Choose on or off".into()),
            Kind::Choice(options) => options
                .iter()
                .any(|(option, _)| option == &value)
                .then_some(value)
                .ok_or_else(|| "Choose an available option".into()),
            Kind::String => Ok(Value::String(text.clone())),
            Kind::Number {
                integer, min, max, ..
            } => number_value(&text, *integer, *min, *max),
            Kind::Vector { .. } => Err("Edit the numeric axes".into()),
            Kind::Json(ty) => serde_json::from_str::<Value>(&text)
                .map_err(|_| "Enter valid JSON".to_owned())
                .and_then(|value| match ty.as_deref() {
                    Some("array") if !value.is_array() => Err("Enter a JSON array".into()),
                    Some("object") if !value.is_object() => Err("Enter a JSON object".into()),
                    _ => Ok(value),
                }),
        };
        self.palette_form.drafts.insert(key.into(), text.clone());
        self.palette_form.error = None;
        if text.is_empty()
            && !field.required
            && !matches!(field.kind, Kind::String | Kind::Boolean | Kind::Choice(_))
        {
            self.palette_params.remove(key);
            self.palette_form.errors.remove(key);
            return;
        }
        match parsed {
            Ok(value) => {
                self.palette_params.insert(key.into(), value);
                self.palette_form.errors.remove(key);
            }
            Err(error) => {
                self.palette_params.remove(key);
                self.palette_form.errors.insert(key.into(), error);
            }
        }
    }
    pub(in crate::shell) fn unset_palette_parameter(&mut self, key: &str) {
        self.palette_params.remove(key);
        self.palette_form.drafts.remove(key);
        self.palette_form.errors.remove(key);
        self.palette_form.error = None;
        self.palette_form.drafts.retain(|draft, _| {
            !draft
                .strip_prefix(key)
                .is_some_and(|suffix| suffix.starts_with('#'))
        });
        self.palette_form.focus = self
            .palette_form
            .fields
            .iter()
            .find(|field| field.key == key)
            .map(Parameter::control_id);
    }
    pub(in crate::shell) fn choose_palette_parameter(
        &mut self,
        command: &str,
        key: &str,
        value: Value,
    ) {
        if self.palette_command.as_deref() != Some(command) {
            return;
        }
        if value.is_null() {
            self.unset_palette_parameter(key);
        } else {
            self.edit_palette_parameter(key, value);
        }
    }
    pub(in crate::shell) fn run_palette_form(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        for field in &self.palette_form.fields {
            if field.required && !self.palette_params.contains_key(&field.key) {
                self.palette_form
                    .errors
                    .entry(field.key.clone())
                    .or_insert_with(|| "Enter a value before running".into());
            }
        }
        if !self.palette_form.errors.is_empty() {
            self.palette_form.focus = self
                .palette_form
                .fields
                .iter()
                .find(|field| self.palette_form.errors.contains_key(&field.key))
                .map(Parameter::control_id);
            return Ok(());
        }
        let Some(command) = self.palette_command.as_deref() else {
            return Ok(());
        };
        let params = serde_json::to_value(&self.palette_params)?;
        let result = if command.starts_with("editor.")
            && editor
                .host
                .catalog()
                .iter()
                .any(|method| method.name == command)
        {
            editor.action(command, params)
        } else {
            editor.apply(
                vec![json!({"cmd":command,"args":params})],
                "Run command palette action",
                None,
            )
        };
        if let Err(error) = result {
            self.palette_form.error = Some(error.to_string());
            editor.report_failure(&error);
            return Ok(());
        }
        editor.action("editor.palette", json!({"open":false}))?;
        self.palette_command = None;
        Ok(())
    }

    fn edit_palette_axis(&mut self, key: &str, axis: usize, value: Value) {
        let Some(Parameter {
            kind:
                Kind::Vector {
                    count,
                    integer,
                    min,
                    max,
                    ..
                },
            ..
        }) = self
            .palette_form
            .fields
            .iter()
            .find(|field| field.key == key)
        else {
            return;
        };
        let (count, integer, min, max) = (*count, *integer, *min, *max);
        if axis >= count {
            return;
        }
        if let Some(values) = self.palette_params.get(key).and_then(Value::as_array) {
            for (axis, value) in values.iter().enumerate() {
                self.palette_form
                    .drafts
                    .entry(format!("{key}#{axis}"))
                    .or_insert_with(|| display_value(value));
            }
        }
        self.palette_form.drafts.insert(
            format!("{key}#{axis}"),
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        );
        let values: Result<Vec<_>, _> = (0..count)
            .map(|axis| {
                number_value(
                    self.palette_form
                        .drafts
                        .get(&format!("{key}#{axis}"))
                        .map(String::as_str)
                        .unwrap_or(""),
                    integer,
                    min,
                    max,
                )
            })
            .collect();
        self.palette_form.error = None;
        match values {
            Ok(values) => {
                self.palette_params.insert(key.into(), Value::Array(values));
                self.palette_form.errors.remove(key);
            }
            Err(error) => {
                self.palette_params.remove(key);
                self.palette_form.errors.insert(
                    key.into(),
                    format!("Enter all {count} numeric axes: {error}"),
                );
            }
        }
    }

    /// Mount hook: a retained form keeps focus; only an explicit transition moves it.
    pub(crate) fn sync_palette_form(&mut self) -> ara_ui::Result<()> {
        if self.current.is_some() {
            return Ok(());
        }
        if self.ui.find("palette").is_none() {
            self.palette_form.command = None;
            self.palette_form.drafts.clear();
            self.palette_form.focus = None;
            return Ok(());
        }
        self.sync_choice_controls()?;
        if self.confirmations.is_open() || self.dialog.is_some() || self.choice_open() {
            return Ok(());
        }
        if let Some(id) = self.palette_form.focus.take()
            && let Some(index) = self.shown_node(&id)
        {
            self.ui.scroll_into_view(index)?;
            self.ui.frame(&mut self.text)?;
            self.input
                .focus(&mut self.ui, ara_ui::source::stable_id(&id))?;
        }
        Ok(())
    }
    /// Form navigation never commits Inspector drafts behind the palette modal.
    pub(crate) fn palette_form_key(
        &mut self,
        editor: &mut Editor,
        key: i32,
        pressed: bool,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<bool> {
        if !pressed
            || self.modal_root != Some("palette")
            || self.ui.find("palette-form-scroll").is_none()
        {
            return Ok(false);
        }
        match key {
            40 | 88 => {
                self.dispatch(editor)?;
                let button = self
                    .input
                    .focused()
                    .and_then(|id| self.ui.by_id(id))
                    .is_some_and(|index| self.ui.nodes[index].element.tag == "button");
                if button {
                    self.event(ara_ui::input::InputEvent::Activate, clipboard)?;
                    self.dispatch(editor)?;
                } else {
                    self.run_palette_form(editor)?;
                    self.rebuild(editor)?;
                }
            }
            43 => {
                self.dispatch(editor)?;
                self.event(
                    ara_ui::input::InputEvent::Tab {
                        reverse: self.modifiers[0],
                    },
                    clipboard,
                )?;
            }
            41 => {
                self.input.cancel_interaction(&mut self.ui);
                editor.action("editor.palette", json!({"open":false}))?;
                self.palette_command = None;
                self.rebuild(editor)?;
            }
            44 if !self.input.focused().is_some_and(|id| {
                matches!(
                    self.ui.widgets.get(&id),
                    Some(ara_ui::widgets::Widget::Input(_))
                )
            }) =>
            {
                self.event(ara_ui::input::InputEvent::Activate, clipboard)?;
                self.dispatch(editor)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub(crate) fn palette_row_id(&self, index: usize) -> Option<String> {
        self.ui
            .paint_order
            .iter()
            .filter_map(|index| {
                let node = &self.ui.nodes[*index];
                (node.element.tag == "button" && node.element.id.starts_with("command-"))
                    .then_some(&node.element.id)
            })
            .nth(index)
            .cloned()
    }
    pub(super) fn guard_palette_generation(&mut self, editor: &Editor) {
        if self.palette_form.generation != editor.extensions.generation {
            if self
                .input
                .captured()
                .and_then(|id| self.ui.by_id(id))
                .is_some_and(|index| self.ui.nodes[index].element.id.starts_with("command-"))
            {
                self.input.cancel_interaction(&mut self.ui);
            }
            self.palette_form.generation = editor.extensions.generation.clone();
        }
    }
}
