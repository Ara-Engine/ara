use super::*;
use ara_ui::input::InputEvent;
use ara_ui_text::Clipboard;
impl Shell {
    pub(crate) fn hover_tooltip(&mut self) -> ara_ui::Result<()> {
        let description = self.ui.paint_order.iter().rev().find_map(|i| {
            let node = &self.ui.nodes[*i];
            (node.rect.contains(self.pointer[0], self.pointer[1])
                && node.clip.contains(self.pointer[0], self.pointer[1]))
            .then(|| node.element.attrs.get("description").map(String::as_str))
            .flatten()
        });
        if description == self.tooltip_text.as_deref() {
            return Ok(());
        }
        let description = description.map(str::to_owned);
        if let Some(i) = self.ui.find("tooltip") {
            self.ui.set_visible(i, description.is_some())?;
            self.ui.nodes[i].paint_offset = [
                self.pointer[0].clamp(0., (self.ui.size[0] - 325.).max(0.)),
                (self.pointer[1] + 18.).clamp(0., (self.ui.size[1] - 100.).max(0.)),
            ];
        }
        if let Some(text) = &description
            && let Some(i) = self.ui.find("tooltip-text")
        {
            self.ui.set_text(i, text)?;
        }
        self.tooltip_text = description;
        Ok(())
    }
    /// Route input through retained hit testing, focus and native widgets.
    pub fn event(
        &mut self,
        event: InputEvent,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<()> {
        self.input.event(&mut self.ui, event, clipboard)
    }
    /// Dispatch queued widget actions through the typed engine endpoint.
    pub fn dispatch(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let actions: Vec<_> = self.ui.drain_actions().collect();
        let bindings = std::mem::take(&mut self.bindings);
        let mut rebuild = false;
        let result = (|| -> ara_ui::Result<()> {
            for action in actions {
                if action.name == "ResizeDock" {
                    if let Some(index) = self.ui.by_id(action.node) {
                        let id = &self.ui.nodes[index].element.id;
                        if let Some(path) = id.strip_prefix("splitter-root") {
                            let path: Vec<_> = path
                                .split('-')
                                .filter(|s| !s.is_empty())
                                .map(|part| part == "b")
                                .collect();
                            if let Some(ratio) = action.value.as_f64() {
                                editor
                                    .action("editor.layout", json!({"path":path,"ratio":ratio}))?;
                                rebuild = true;
                            }
                        }
                    }
                    continue;
                }
                let Some(binding) = bindings.get(&action.node) else {
                    continue;
                };
                match binding {
                    Binding::ComponentCollapse(name) => {
                        if !self.collapsed_components.remove(name) {
                            self.collapsed_components.insert(name.clone());
                        }
                        rebuild = true;
                    }
                    Binding::ComponentMenu(name) => {
                        self.component_menu = if self.component_menu.as_ref() == Some(name) {
                            None
                        } else {
                            Some(name.clone())
                        };
                        rebuild = true;
                    }
                    Binding::Scrub(_, _) => {}
                    Binding::ColorPicker(path) => {
                        self.color_picker = if self.color_picker.as_ref() == Some(path) {
                            None
                        } else {
                            Some(path.clone())
                        };
                        rebuild = true;
                    }
                    Binding::Diagnostic(diagnostic) => {
                        if let Some(record) = &diagnostic.location.record {
                            let mut args = json!({"entities":[record]});
                            if let Some(doc) = &diagnostic.location.doc {
                                args["doc"] = doc.clone().into();
                            }
                            editor.action("editor.select", args)?;
                        }
                        editor.action(
                            "editor.focus",
                            json!({"panel":"inspector","field":diagnostic.location.path}),
                        )?;
                        rebuild = true;
                    }
                    Binding::Protocol(method, params) => {
                        editor.action(method, params.clone())?;
                        rebuild = true;
                    }
                    Binding::Content(commands, why) => {
                        editor.apply(commands.clone(), why, None)?;
                        self.context = false;
                        if editor.state.palette {
                            editor.action("editor.palette", json!({"open":false}))?;
                        }
                        rebuild = true;
                    }
                    Binding::Select(entity) => {
                        let key = crate::world::render_id(entity);
                        let chevron = self.hierarchy_style.get(&key).is_some_and(|s| {
                            s.children
                                && self.ui.by_id(action.node).is_some_and(|i| {
                                    self.pointer[0]
                                        < self.ui.nodes[i].rect.x
                                            + 24.
                                            + s.depth.min(32) as f32 * 14.
                                })
                        });
                        if chevron {
                            editor.action("editor.collapse",json!({"entity":entity,"enabled":!editor.state.collapsed.contains(entity)}))?;
                            rebuild = true;
                            continue;
                        }

                        if let Some(path) = self.pick_field.take() {
                            editor.apply(
                                crate::inspector::edit_commands(
                                    &editor.state.selection,
                                    &path,
                                    &json!(entity),
                                ),
                                "Pick entity reference",
                                None,
                            )?;
                        } else {
                            editor.action("editor.select",json!({"entities":[entity],"toggle":self.modifiers[1],"additive":self.modifiers[0]}))?;
                        }
                        rebuild = true;
                    }
                    Binding::Field(path, axis) => {
                        if let Some(value) = action.value.as_bool() {
                            editor.apply(
                                crate::inspector::edit_commands(
                                    &editor.state.selection,
                                    path,
                                    &json!(value),
                                ),
                                "Edit inspector value",
                                None,
                            )?;
                            rebuild = true;
                        } else {
                            self.drafts.insert(
                                format!(
                                    "{path}#{}",
                                    axis.map(|a| a.to_string()).unwrap_or_default()
                                ),
                                action
                                    .value
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| action.value.to_string()),
                            );
                        }
                    }
                    Binding::Search(panel) => {
                        editor.action(
                            "editor.search",
                            json!({"panel":panel,"query":action.value.as_str().unwrap_or("")}),
                        )?;
                        rebuild = true;
                    }
                    Binding::Palette(command) => {
                        self.palette_command = Some(command.clone());
                        self.palette_params.clear();
                        if let Some(entity) = editor.state.selection.first() {
                            self.palette_params.insert("entity".into(), json!(entity));
                        }
                        rebuild = true;
                    }
                    Binding::PaletteParam(key) => {
                        let text = action.value.as_str().unwrap_or("");
                        let value = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
                        self.palette_params.insert(key.clone(), value);
                    }
                    Binding::PaletteRun => {
                        if let Some(command) = self.palette_command.as_deref() {
                            let params = serde_json::to_value(&self.palette_params)?;
                            if command.starts_with("editor.")
                                && editor.host.catalog().iter().any(|m| m.name == command)
                            {
                                editor.action(command, params)?;
                            } else {
                                editor.apply(
                                    vec![json!({"cmd":command,"args":params})],
                                    "Run command palette action",
                                    None,
                                )?;
                            }
                            editor.action("editor.palette", json!({"open":false}))?;
                            self.palette_command = None;
                            rebuild = true;
                        }
                    }
                    Binding::Asset(asset) => {
                        if let Some(path) = self.pick_field.take()
                            && let Some(reference) = &asset.reference
                        {
                            editor.apply(
                                crate::inspector::edit_commands(
                                    &editor.state.selection,
                                    &path,
                                    &json!(reference),
                                ),
                                "Pick asset reference",
                                None,
                            )?;
                            rebuild = true;
                        }
                    }
                    Binding::Fix(diagnostic, index) => {
                        apply_fix(editor, diagnostic, *index)?;
                        rebuild = true;
                    }
                    Binding::Approve(ticket) => {
                        editor.approve(ticket)?;
                        rebuild = true;
                    }
                    Binding::Reject(ticket) => {
                        editor.reject(ticket)?;
                        rebuild = true;
                    }
                    Binding::PickField(path) => {
                        self.pick_field = Some(path.clone());
                        if self.fields.get(path).is_some_and(|f| {
                            matches!(f.widget, crate::inspector::Widget::Asset { .. })
                        }) {
                            editor.action("editor.focus", json!({"panel":"assets"}))?;
                        }
                        rebuild = true;
                    }
                    Binding::Rename => {
                        self.rename = true;
                        self.context = false;
                        rebuild = true;
                    }
                    Binding::RenameValue => {
                        self.drafts
                            .insert("@name".into(), action.value.as_str().unwrap_or("").into());
                    }
                    Binding::AddComponent => {
                        self.palette_command = Some("@components".into());
                        editor.action("editor.palette", json!({"open":true}))?;
                        rebuild = true;
                    }
                    Binding::Context => {
                        self.context = !self.context;
                        rebuild = true;
                    }
                }
            }
            Ok(())
        })();
        self.bindings = bindings;
        result?;
        if rebuild {
            self.rebuild(editor)?;
        }
        Ok(rebuild)
    }
    /// Commit pending inspector text and slider values as one transaction.
    pub fn commit_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.drafts.is_empty() {
            return Ok(false);
        }
        let mut commands = Vec::new();
        for (key, text) in &self.drafts {
            if key == "@name" {
                commands.extend(crate::inspector::edit_commands(
                    &editor.state.selection,
                    "name",
                    &json!(text),
                ));
                continue;
            }
            let (path, axis) = key.rsplit_once('#').ok_or("invalid inspector edit key")?;
            let field = self
                .fields
                .get(path)
                .ok_or("inspector schema field disappeared")?;
            let value = if matches!(
                field.widget,
                crate::inspector::Widget::Number { .. } | crate::inspector::Widget::Vector { .. }
            ) {
                let number = crate::inspector::parse_number(text, field.unit.as_deref())?;
                serde_json::from_str::<Value>(text)
                    .ok()
                    .filter(Value::is_number)
                    .unwrap_or_else(|| json!(number))
            } else if matches!(field.widget, crate::inspector::Widget::Text)
                && !field.value.is_string()
            {
                serde_json::from_str(text)?
            } else {
                json!(text)
            };
            let path = if axis.is_empty() {
                path.to_owned()
            } else {
                let axis: usize = axis.parse()?;
                format!(
                    "{path}.{}",
                    if matches!(
                        field.widget,
                        crate::inspector::Widget::Vector { color: true, .. }
                    ) {
                        ["r", "g", "b", "a"][axis]
                    } else {
                        ["x", "y", "z", "w"][axis]
                    }
                )
            };
            commands.extend(crate::inspector::edit_commands(
                &editor.state.selection,
                &path,
                &value,
            ));
        }
        editor.apply(commands, "Edit inspector values", None)?;
        self.drafts.clear();
        self.rename = false;
        self.rebuild(editor)?;
        Ok(true)
    }
}
fn apply_fix(
    editor: &mut Editor,
    diagnostic: &ara_diag::Diagnostic,
    index: usize,
) -> ara_ui::Result<()> {
    use ara_diag::FixAction;
    let fix = diagnostic.fixes.get(index).ok_or("fix disappeared")?;
    let entity = diagnostic
        .location
        .record
        .as_deref()
        .or_else(|| editor.state.selection.first().map(String::as_str));
    let workspace = editor.host.store().workspace();
    let doc = diagnostic
        .location
        .doc
        .clone()
        .or_else(|| {
            diagnostic
                .location
                .file
                .as_deref()
                .and_then(|file| workspace.find(file).map(ToString::to_string))
        })
        .unwrap_or_else(|| editor.state.doc.clone());
    match &fix.action {
        FixAction::Remove { record, path } => {
            let entity = record
                .as_deref()
                .or(entity)
                .ok_or("fix needs a target entity")?;
            editor.apply_to(
                &doc,
                vec![json!({"cmd":"editor.property.reset","args":{"entity":entity,"path":path}})],
                &fix.title,
            )?;
        }
        FixAction::Rename { record, path, to } => {
            let entity = record
                .as_deref()
                .or(entity)
                .ok_or("fix needs a target entity")?;
            editor.apply_to(&doc, vec![json!({"cmd":"editor.property.rename","args":{"entity":entity,"path":path,"to":to}})],&fix.title)?;
        }
        FixAction::Command { command, params } => {
            editor.apply_to(&doc, vec![json!({"cmd":command,"args":params})], &fix.title)?;
        }
        FixAction::Call { method, params } => {
            editor.action(method, params.clone())?;
        }
        FixAction::Set {
            record,
            path,
            value,
        } => {
            let entity = record
                .as_deref()
                .or(entity)
                .ok_or("fix needs a target record")?;
            editor.apply_to(&doc, vec![json!({"cmd":"property.set","args":{"entity":entity,"path":path,"value":value}})],&fix.title)?;
        }
        _ => {
            return Err(
                "This fix requires the file/schema tool; its diagnostic includes the exact action"
                    .into(),
            );
        }
    }
    Ok(())
}
