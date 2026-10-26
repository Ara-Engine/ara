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
        if let InputEvent::PointerDown(point) = &event
            && self.dismiss_inspector_colour(*point)?
        {
            return self
                .input
                .event(&mut self.ui, InputEvent::Cancel, clipboard);
        }
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
                            // Floating-window and maximized splits resize natively but are not persisted.
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
                    Binding::GraphLabel(doc, key) => {
                        self.drafts.insert(
                            format!("@graph-label|{doc}|{key}"),
                            action.value.as_str().unwrap_or("").into(),
                        );
                    }
                    Binding::MaterialValue(doc, node) => {
                        let key = format!("@material|{doc}|{node}");
                        if let Some(on) = action.value.as_bool() {
                            self.drafts.insert(key, on.to_string());
                            self.commit_fields(editor)?;
                            rebuild = true;
                        } else {
                            self.drafts
                                .insert(key, action.value.as_str().unwrap_or("").into());
                        }
                    }
                    Binding::AnimationEdit(kind) => {
                        self.edit_animation(editor, kind)?;
                        rebuild = true;
                    }
                    Binding::AnimationField(doc, item, key) => {
                        if action.value.is_boolean() {
                            self.set_animation_field(editor, doc, item, key, action.value)?;
                            rebuild = true;
                        } else {
                            self.drafts.insert(
                                format!("@animation|{doc}|{item}|{key}"),
                                action.value.as_str().unwrap_or("").into(),
                            );
                        }
                    }
                    Binding::VfxPlayback(playing) => {
                        editor.action("editor.workspace", json!({"playing":playing}))?;
                        rebuild = true;
                    }
                    Binding::VfxField(doc, item, key) => {
                        if action.value.is_boolean() {
                            self.set_vfx_field(editor, doc, item, key, action.value)?;
                            rebuild = true;
                        } else {
                            self.drafts.insert(
                                format!("@vfx|{doc}|{item}|{key}"),
                                action.value.as_str().unwrap_or("").into(),
                            );
                        }
                    }
                    Binding::ResourceText(doc, _domain) => {
                        self.resource_text =
                            Some((doc.clone(), action.value.as_str().unwrap_or("").into()));
                    }
                    Binding::ResourceApply => {
                        self.apply_resource_text(editor)?;
                        rebuild = true;
                    }
                    Binding::PreviewShape(plane) => {
                        editor.action("editor.workspace", json!({"plane":plane}))?;
                        rebuild = true;
                    }
                    Binding::ComponentCollapse(name) | Binding::NestedCollapse(name) => {
                        self.inspector_sections.toggle(name);
                        rebuild = true;
                    }
                    Binding::Command(id, target) => {
                        let (id, target) = (*id, target.clone());
                        let close_palette = editor.state.palette;
                        self.remember(id);
                        self.menu = None;
                        self.mnemonics = false;
                        if close_palette {
                            editor.action("editor.palette", json!({"open":false}))?;
                        }
                        self.execute(editor, id, &target)?;
                        rebuild = true;
                    }
                    Binding::MenuBar(index) => {
                        if self.menu.as_ref().and_then(|m| m.bar()) == Some(*index) {
                            self.menu = None;
                        } else {
                            self.open_bar(*index, false);
                        }
                        self.mnemonics = false;
                        rebuild = true;
                    }
                    Binding::AssetPicker(path, kind) => {
                        let at = self
                            .ui
                            .by_id(action.node)
                            .map(|i| {
                                let r = self.ui.nodes[i].rect;
                                [r.x, r.y + r.h + 2.]
                            })
                            .unwrap_or(self.pointer);
                        self.open_menu_at(
                            super::menu::Source::Assets(path.clone(), kind.clone()),
                            at,
                        );
                        rebuild = true;
                    }
                    Binding::EnumMenu(path, options) => {
                        let at = self
                            .ui
                            .by_id(action.node)
                            .map(|i| {
                                let r = self.ui.nodes[i].rect;
                                [r.x, r.y + r.h + 2.]
                            })
                            .unwrap_or(self.pointer);
                        self.open_menu_at(
                            super::menu::Source::Enum(path.clone(), options.clone()),
                            at,
                        );
                        rebuild = true;
                    }
                    Binding::MenuSub(level, row) => {
                        if let Some(menu) = &mut self.menu {
                            menu.open.truncate(*level);
                            menu.open.push(*row);
                            menu.active = None;
                        }
                        rebuild = true;
                    }
                    Binding::MenuFilter => {
                        if let Some(menu) = &mut self.menu {
                            menu.query = action.value.as_str().unwrap_or("").into();
                            menu.open.clear();
                            menu.active = None;
                        }
                        rebuild = true;
                    }
                    Binding::ContextMenu(kind, target) => {
                        let at = self
                            .ui
                            .by_id(action.node)
                            .map(|i| {
                                let r = self.ui.nodes[i].rect;
                                [r.x, r.y + r.h + 2.]
                            })
                            .unwrap_or(self.pointer);
                        self.open_menu_at(super::menu::Source::Context(*kind, target.clone()), at);
                        rebuild = true;
                    }
                    Binding::CameraFov => {
                        self.drafts.insert(
                            "@camera|fov".into(),
                            action.value.as_str().unwrap_or("").into(),
                        );
                    }
                    Binding::Setting(key) => {
                        if let Some(v) = action.value.as_bool() {
                            editor.action("editor.settings.set", json!({"key":key,"value":v}))?;
                            rebuild = true;
                        } else {
                            self.drafts.insert(
                                format!("@setting|{key}"),
                                action.value.as_str().unwrap_or("").into(),
                            );
                        }
                    }
                    Binding::SettingChoice(key, value) => {
                        if key == "@view" {
                            editor.action("editor.settings.view", value.clone())?;
                        } else if value.is_null() {
                            editor
                                .action("editor.settings.set", json!({"key":key,"reset":true}))?;
                        } else {
                            editor
                                .action("editor.settings.set", json!({"key":key,"value":value}))?;
                        }
                        rebuild = true;
                    }
                    Binding::ShortcutRecord(id) => {
                        self.recording = Some(*id);
                        self.key_search = false;
                        rebuild = true;
                    }
                    Binding::ShortcutReset(id) => {
                        let mut map = editor
                            .state
                            .settings
                            .get("input.shortcuts")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        map.remove(*id);
                        editor.action(
                            "editor.settings.set",
                            json!({"key":"input.shortcuts","value":map}),
                        )?;
                        rebuild = true;
                    }
                    Binding::KeySearch => {
                        self.key_search = !self.key_search;
                        self.recording = None;
                        rebuild = true;
                    }
                    Binding::DialogText => {
                        if let Some(Dialog::SaveLayout { name }) = &mut self.dialog {
                            *name = action.value.as_str().unwrap_or("").into();
                        }
                    }
                    Binding::DialogAction(which) => {
                        self.dialog_action(editor, which)?;
                        rebuild = true;
                    }
                    Binding::ResourceContent(doc, commands, why) => {
                        editor.apply_to(doc, commands.clone(), why)?;
                        if commands.iter().any(|c| {
                            c["cmd"]
                                .as_str()
                                .is_some_and(|s| s.starts_with("editor.material"))
                        }) {
                            editor.action("editor.canvas", json!({"add":false}))?;
                            self.resource_text = None;
                        }
                        rebuild = true;
                    }
                    Binding::ImportChoice(doc, key, value) => {
                        self.drafts
                            .insert(format!("@import|{doc}|{key}"), value.to_string());
                        self.commit_import_fields(editor)?;
                        rebuild = true;
                    }
                    Binding::ImportField(doc, key) => {
                        self.drafts.insert(
                            format!("@import|{doc}|{key}"),
                            action
                                .value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| action.value.to_string()),
                        );
                        if action.value.is_boolean() {
                            self.commit_import_fields(editor)?;
                            rebuild = true;
                        }
                    }
                    Binding::Scrub(_, _) => {}
                    Binding::NestedScrub { .. } => {}
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
                        if matches!(*method, "editor.asset.reimport" | "editor.asset.import")
                            && let Some(path) = params["path"].as_str()
                        {
                            self.thumbnails.invalidate(path);
                            self.thumbnail_slots.remove(path);
                            self.thumbnail_revision += 1;
                        }
                        editor.action(method, params.clone())?;
                        rebuild = true;
                    }
                    Binding::Content(commands, why) => {
                        editor.apply(commands.clone(), why, None)?;
                        self.menu = None;
                        if editor.state.palette {
                            editor.action("editor.palette", json!({"open":false}))?;
                        }
                        rebuild = true;
                    }
                    Binding::Select(entity) => {
                        let key = crate::world::render_id(entity);
                        let row = self.ui.by_id(action.node).map(|i| self.ui.nodes[i].rect);
                        let x = self.pointer[0];
                        let chevron = self.hierarchy_style.get(&key).is_some_and(|s| {
                            s.children
                                && row.is_some_and(|r| x < r.x + 24. + s.depth.min(32) as f32 * 16.)
                        });
                        if chevron {
                            // The row's shown state: large groups start collapsed.
                            let collapsed =
                                self.hierarchy_style.get(&key).is_some_and(|s| s.collapsed);
                            editor.action(
                                "editor.collapse",
                                json!({"entity":entity,"enabled":!collapsed}),
                            )?;
                            rebuild = true;
                            continue;
                        }
                        // Hover gutter: eye toggles viewport visibility, lock blocks picking.
                        if let Some(r) = row {
                            let right = r.x + r.w;
                            if (right - super::polish::EYE..right - super::polish::EYE + 16.)
                                .contains(&x)
                            {
                                editor.action("editor.visibility",json!({"entity":entity,"enabled":!editor.state.hidden.contains(entity)}))?;
                                rebuild = true;
                                continue;
                            }
                            if (right - super::polish::LOCK..right - super::polish::LOCK + 16.)
                                .contains(&x)
                            {
                                editor.action("editor.lock",json!({"entity":entity,"enabled":!editor.state.locked.contains(entity)}))?;
                                rebuild = true;
                                continue;
                            }
                        }

                        if let Some(path) = self.pick_field.take() {
                            editor.apply(
                                self.field_commands(&editor.state.selection, &path, json!(entity))?,
                                "Pick entity reference",
                                None,
                            )?;
                        } else {
                            editor.action("editor.select",json!({"entities":[entity],"toggle":self.modifiers[1],"additive":self.modifiers[0]}))?;
                        }
                        rebuild = true;
                    }
                    Binding::Nested(target, leaf, nullable, unit) => {
                        let path = target
                            .split_once('#')
                            .map_or(target.as_str(), |(path, _)| path);
                        if self.fields.get(path).is_some_and(|field| field.read_only) {
                            continue;
                        }
                        for entity in &editor.state.selection.clone() {
                            editor.field_gesture(entity, path, true)?;
                        }
                        let text = action
                            .value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| action.value.to_string());
                        self.drafts
                            .insert(leaf.draft(target, *nullable, unit.as_deref()), text);
                    }
                    Binding::Field(path, axis) => {
                        if self.fields.get(path).is_some_and(|field| field.read_only) {
                            continue;
                        }
                        if !action.value.is_boolean() {
                            let gesture_path = axis.map_or_else(
                                || path.clone(),
                                |axis| {
                                    let color = self.fields.get(path).is_some_and(|f| {
                                        matches!(
                                            f.widget,
                                            crate::inspector::Widget::Vector { rgba: true, .. }
                                        )
                                    });
                                    format!(
                                        "{path}.{}",
                                        if color {
                                            ["r", "g", "b", "a"][axis]
                                        } else {
                                            ["x", "y", "z", "w"][axis]
                                        }
                                    )
                                },
                            );
                            for entity in editor.state.selection.clone() {
                                editor.field_gesture(&entity, &gesture_path, true)?;
                            }
                        }
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
                        rebuild = true;
                        let Some(path) = self.pick_field.clone() else {
                            editor.action("editor.workspace", json!({"asset":asset.path}))?;
                            continue;
                        };
                        // Picking: a file that does not fit, or has no asset ID yet, is
                        // explained and picking goes on.
                        match self.asset_for_field(editor, asset, &path) {
                            Ok(reference) => {
                                self.pick_field = None;
                                self.status_note = None;
                                editor.apply(
                                    crate::inspector::edit_commands(
                                        &editor.state.selection,
                                        &path,
                                        &json!(reference),
                                    ),
                                    "Pick asset reference",
                                    None,
                                )?;
                            }
                            Err(why) => self.status_note = Some(why),
                        }
                    }
                    Binding::Fix(diagnostic, index) => {
                        apply_fix(editor, diagnostic, *index)?;
                        rebuild = true;
                    }
                    Binding::PickField(path) => {
                        self.pick_field = Some(path.clone());
                        self.status_note = None;
                        if self.fields.get(path).is_some_and(|f| {
                            matches!(f.widget, crate::inspector::Widget::Asset { .. })
                        }) {
                            editor.action("editor.focus", json!({"panel":"assets"}))?;
                        }
                        rebuild = true;
                    }
                    Binding::RenameValue => {
                        for entity in editor.state.selection.clone() {
                            editor.field_gesture(&entity, "name", true)?;
                        }
                        self.drafts
                            .insert("@name".into(), action.value.as_str().unwrap_or("").into());
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
    /// The reference `asset` gives the asset field at `path`, or why it cannot fill
    /// it: the wrong kind of file, or no asset ID yet (still importing).
    pub(super) fn asset_for_field(
        &self,
        editor: &Editor,
        asset: &assets::Asset,
        path: &str,
    ) -> Result<String, String> {
        // A compound element (`path#pointer`) declares its kind on its picker.
        let kind = match path.split_once('#') {
            None => self.fields.get(path).and_then(|f| match &f.widget {
                crate::inspector::Widget::Asset { kind } => Some(kind.clone()),
                _ => None,
            }),
            Some(_) => self.bindings.values().find_map(|b| match b {
                Binding::AssetPicker(target, kind) if target == path => Some(kind.clone()),
                _ => None,
            }),
        };
        let kind = kind.flatten();
        let label = match path.split_once('#') {
            Some((_, pointer)) => pointer.rsplit('/').next().unwrap_or(pointer),
            None => self.fields.get(path).map_or(path, |f| f.label.as_str()),
        };
        if !asset.fits(kind.as_deref(), path) {
            let wants = assets::field_accepts(kind.as_deref(), path).1;
            return Err(format!(
                "{} takes {wants}, not {}",
                super::inspector_panel::sentence(label),
                asset.name()
            ));
        }
        asset
            .current_reference(editor.host.store().workspace())
            .ok_or_else(|| {
                format!(
                    "{} has no asset ID yet: it is still importing. Pick it again when it is ready",
                    asset.name()
                )
            })
    }
    /// Commit pending inspector text and slider values as one transaction.
    pub fn commit_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.commit_graph_labels(editor)? && self.drafts.is_empty() {
            self.rebuild(editor)?;
            return Ok(true);
        }
        let material_keys = self
            .drafts
            .keys()
            .filter(|k| k.starts_with("@material|"))
            .cloned()
            .collect::<Vec<_>>();
        for key in &material_keys {
            let parts = key.split('|').collect::<Vec<_>>();
            if let [_, doc, node] = parts.as_slice() {
                let text = self.drafts.get(key).ok_or("material draft missing")?;
                // JSON, or lanes typed as `0.18, 0.42, 0.9`.
                let mut value: Value = serde_json::from_str(text)
                    .or_else(|_| serde_json::from_str(&format!("[{}]", text.trim())))?;
                if value.is_number() {
                    value = json!([value]);
                }
                editor.apply_to(doc,vec![json!({"cmd":"editor.material.value.set","args":{"node":node,"value":value}})],"Edit material node value")?;
            }
        }
        for key in &material_keys {
            self.drafts.remove(key);
        }
        if !material_keys.is_empty() && self.drafts.is_empty() {
            self.rebuild(editor)?;
            return Ok(true);
        }
        let animation_keys = self
            .drafts
            .keys()
            .filter(|k| k.starts_with("@animation|"))
            .cloned()
            .collect::<Vec<_>>();
        for key in &animation_keys {
            let parts = key.split('|').collect::<Vec<_>>();
            if let [_, doc, item, field] = parts.as_slice() {
                let text = self.drafts.get(key).ok_or("animation draft missing")?;
                let value = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
                self.set_animation_field(editor, doc, item, field, value)?;
            }
        }
        for key in &animation_keys {
            self.drafts.remove(key);
        }
        if !animation_keys.is_empty() && self.drafts.is_empty() {
            self.rebuild(editor)?;
            return Ok(true);
        }
        if let Some(text) = self.drafts.remove("@camera|fov") {
            self.execute(editor, "view.camera.fov.set", &commands::Target::Text(text))?;
            if self.drafts.is_empty() {
                self.rebuild(editor)?;
                return Ok(true);
            }
        }
        let setting_keys = self
            .drafts
            .keys()
            .filter(|k| k.starts_with("@setting|"))
            .cloned()
            .collect::<Vec<_>>();
        for key in &setting_keys {
            let text = self.drafts.remove(key).unwrap_or_default();
            let name = &key["@setting|".len()..];
            let value = match crate::settings::def_of(name).map(|d| d.kind) {
                Some(crate::settings::Kind::Number { .. }) => json!(
                    text.trim()
                        .trim_end_matches(['%', 'x', '×'])
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| format!("{name} needs a number"))?
                ),
                _ => json!(text),
            };
            editor.action("editor.settings.set", json!({"key":name,"value":value}))?;
        }
        if !setting_keys.is_empty() && self.drafts.is_empty() {
            self.rebuild(editor)?;
            return Ok(true);
        }
        let imported = self.commit_import_fields(editor)?;
        let vfx_keys = self
            .drafts
            .keys()
            .filter(|k| k.starts_with("@vfx|"))
            .cloned()
            .collect::<Vec<_>>();
        for key in &vfx_keys {
            let parts = key.split('|').collect::<Vec<_>>();
            if let [_, doc, emitter, stage, name, field] = parts.as_slice() {
                let text = self.drafts.get(key).ok_or("VFX edit missing")?;
                let value = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
                self.set_vfx_field(
                    editor,
                    doc,
                    &format!("{emitter}|{stage}|{name}"),
                    field,
                    value,
                )?;
            }
        }
        for key in &vfx_keys {
            self.drafts.remove(key);
        }
        if !vfx_keys.is_empty() && self.drafts.is_empty() {
            self.rebuild(editor)?;
            return Ok(true);
        }
        if imported && self.drafts.is_empty() {
            self.rebuild(editor)?;
            return Ok(true);
        }
        let mut commands = self.nested_commands(&editor.state.selection)?;
        if self.drafts.is_empty() && commands.is_empty() {
            return Ok(false);
        }
        for (key, text) in &self.drafts {
            if key.starts_with("@scrub|") {
                continue;
            }
            if key == "@name" {
                commands.extend(crate::inspector::edit_commands(
                    &editor.state.selection,
                    "name",
                    &json!(text),
                ));
                continue;
            }
            let edit = self.draft_edit(key, text)?;
            if edit.axis.is_none()
                && matches!(
                    edit.field.widget,
                    crate::inspector::Widget::Vector { color: true, .. }
                )
            {
                let rgba = matches!(
                    edit.field.widget,
                    crate::inspector::Widget::Vector { rgba: true, .. }
                );
                for axis in 0..3 {
                    let name = if rgba {
                        ["r", "g", "b"][axis]
                    } else {
                        ["x", "y", "z"][axis]
                    };
                    commands.extend(crate::inspector::edit_commands(
                        &editor.state.selection,
                        &format!("{}.{}", edit.field.path, name),
                        &edit.value[axis],
                    ));
                }
                continue;
            }
            commands.extend(crate::inspector::edit_commands(
                &editor.state.selection,
                &edit.path(),
                &edit.value,
            ));
        }
        let result = editor.apply(commands, "Edit inspector values", None);
        editor.end_field_gestures()?;
        result?;
        self.drafts.clear();
        self.rename = false;
        self.rebuild(editor)?;
        Ok(true)
    }

    /// Parse one Inspector field draft (`key` is `path#axis`) into what it commits.
    fn draft_edit(&self, key: &str, text: &str) -> ara_ui::Result<DraftEdit<'_>> {
        let (path, axis) = key.rsplit_once('#').ok_or("invalid inspector edit key")?;
        let field = self
            .fields
            .get(path)
            .ok_or("inspector schema field disappeared")?;
        let value = if axis.is_empty()
            && matches!(
                field.widget,
                crate::inspector::Widget::Vector { color: true, .. }
            ) {
            // `#RRGGBB` sRGB hex becomes linear channels; alpha keeps its value.
            let hex = text.trim().trim_start_matches('#');
            if hex.len() != 6 {
                return Err("Enter a colour as #RRGGBB".into());
            }
            let channel = |n: usize| -> ara_ui::Result<f64> {
                Ok((u8::from_str_radix(&hex[n * 2..n * 2 + 2], 16)? as f64 / 255.).powf(2.2))
            };
            let round = |v: f64| (v * 10000.).round() / 10000.;
            let mut rgb = vec![
                json!(round(channel(0)?)),
                json!(round(channel(1)?)),
                json!(round(channel(2)?)),
            ];
            // A `color` keeps its alpha; a colour-hinted vec3 has none.
            if let crate::inspector::Widget::Vector { count: 4, .. } = field.widget {
                rgb.push(field.value.get(3).cloned().unwrap_or(json!(1.)));
            }
            Value::Array(rgb)
        } else if matches!(
            field.widget,
            crate::inspector::Widget::Number { .. } | crate::inspector::Widget::Vector { .. }
        ) {
            let number = crate::inspector::parse_number(text, field.unit.as_deref())?;
            serde_json::from_str::<Value>(text)
                .ok()
                .filter(Value::is_number)
                .unwrap_or_else(|| json!(number))
        } else if matches!(field.widget, crate::inspector::Widget::Text) && !field.value.is_string()
        {
            serde_json::from_str(text)?
        } else {
            json!(text)
        };
        let axis = if axis.is_empty() {
            None
        } else {
            Some(axis.parse()?)
        };
        Ok(DraftEdit { field, axis, value })
    }

    /// The selected entities' records with the typed Inspector drafts applied, for a
    /// live Scene preview before the commit; None when nothing previews. Drafts that
    /// do not parse yet (half a colour) and names are left out.
    pub(crate) fn draft_records(
        &self,
        editor: &Editor,
        view: &crate::world::DocumentView,
    ) -> Option<BTreeMap<String, ara_doc::Record>> {
        use crate::world::EditWorldView;
        let mut records = BTreeMap::new();
        if let Some(scrub) = &self.scrub
            && let Some(field) = self.fields.get(&scrub.path)
        {
            for (entity, value) in scrub.values() {
                let value = if let Some(pointer) = &scrub.pointer {
                    let Some(whole) = compound::preview_value(&field.value, pointer, value) else {
                        continue;
                    };
                    whole
                } else {
                    value
                };
                let edit = DraftEdit {
                    field,
                    axis: scrub.axis,
                    value,
                };
                if let Some(base) = view.entities().get(entity)
                    && let Some(record) = edit.applied(base.record.clone())
                {
                    records.insert(entity.to_owned(), record);
                }
            }
        }
        for (key, text) in &self.drafts {
            if key.starts_with('@') {
                continue;
            }
            let Ok(edit) = self.draft_edit(key, text) else {
                continue;
            };
            for entity in &editor.state.selection {
                let base = records
                    .get(entity)
                    .cloned()
                    .or_else(|| view.entities().get(entity).map(|e| e.record.clone()));
                if let Some(record) = base.and_then(|base| edit.applied(base)) {
                    records.insert(entity.clone(), record);
                }
            }
        }
        (!records.is_empty()).then_some(records)
    }
}

/// One parsed Inspector draft: the field, an optional vector channel and the value.
struct DraftEdit<'a> {
    field: &'a crate::inspector::Field,
    axis: Option<usize>,
    value: Value,
}
impl DraftEdit<'_> {
    /// The `property.set` path: `Transform.position.x`, `MeshRenderer.color.r`.
    fn path(&self) -> String {
        let Some(axis) = self.axis else {
            return self.field.path.clone();
        };
        let color = matches!(
            self.field.widget,
            crate::inspector::Widget::Vector { rgba: true, .. }
        );
        let names = if color {
            ["r", "g", "b", "a"]
        } else {
            ["x", "y", "z", "w"]
        };
        format!("{}.{}", self.field.path, names[axis])
    }
    /// `record` with this edit applied as `property.set` would; a channel of a field
    /// the record leaves at its default starts from the field's effective value.
    /// None when the record's shape does not fit (a mixed selection's missing vector).
    fn applied(&self, record: ara_doc::Record) -> Option<ara_doc::Record> {
        let mut out = Value::Object(record);
        let mut at = &mut out;
        let mut keys: Vec<&str> = self.field.path.split('.').collect();
        let field = keys.pop()?.to_owned();
        for key in keys {
            at = at
                .as_object_mut()?
                .entry(key.to_owned())
                .or_insert_with(|| json!({}));
        }
        let object = at.as_object_mut()?;
        match self.axis {
            None if matches!(
                self.field.widget,
                crate::inspector::Widget::Vector {
                    color: true,
                    count: 4,
                    ..
                }
            ) =>
            {
                let mut value = self.value.clone();
                value[3] = object
                    .get(&field)
                    .and_then(|value| value.get(3))
                    .cloned()
                    .or_else(|| self.field.fallback.get(3).cloned())
                    .unwrap_or(json!(1.));
                object.insert(field, value);
            }
            None => {
                object.insert(field, self.value.clone());
            }
            Some(axis) => {
                let vector = object.entry(field).or_insert_with(|| {
                    if self.field.value.is_array() {
                        self.field.value.clone()
                    } else {
                        self.field.fallback.clone()
                    }
                });
                *vector.as_array_mut()?.get_mut(axis)? = self.value.clone();
            }
        }
        match out {
            Value::Object(record) => Some(record),
            _ => None,
        }
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
