use super::*;
use ara_ui::input::InputEvent;
use ara_ui_text::Clipboard;
#[cfg(test)]
#[path = "input/dock_resize_tests.rs"]
mod dock_resize_tests;
impl Shell {
    /// Read-only native semantic state for diagnostics and offscreen verification.
    pub(crate) fn native_input_state(&self) -> &ara_ui::native_input::Router {
        &self.native_input
    }
    /// SDL is the editor's sole committed-text ingress on every native surface.
    pub(crate) fn native_text(
        &mut self,
        text: &str,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<bool> {
        self.native_input.commit(
            &mut self.ui,
            &mut self.input,
            ara_ui::native_input::TextSource::Sdl,
            text,
            clipboard,
        )
    }

    pub(crate) fn native_preedit(
        &mut self,
        text: &str,
        start: i32,
        length: i32,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<bool> {
        self.native_input.preedit(
            &mut self.ui,
            &mut self.input,
            ara_ui::native_input::TextSource::Sdl,
            text,
            start,
            length,
            clipboard,
        )
    }

    pub(crate) fn native_focus_lost(
        &mut self,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<()> {
        self.modifiers = [false; 3];
        self.native_input
            .focus_lost(&mut self.ui, &mut self.input, clipboard)?;
        for surface in &mut self.floats {
            surface
                .native_input
                .focus_lost(&mut surface.ui, &mut surface.input, clipboard)?;
        }
        Ok(())
    }

    pub(crate) fn hover_tooltip(&mut self) -> ara_ui::Result<()> {
        if self.modal_active() {
            self.tooltip_target = None;
            self.tooltip_text = None;
            self.tooltip_delay.reset();
            if let Some(i) = self.ui.find("tooltip") {
                self.ui.set_visible(i, false)?;
            }
            return Ok(());
        }
        let target = self
            .ui
            .scrollbar_at(self.pointer)
            .is_none()
            .then(|| {
                self.ui.paint_order.iter().rev().find_map(|i| {
                    let node = &self.ui.nodes[*i];
                    (node.rect.contains(self.pointer[0], self.pointer[1])
                        && node.clip.contains(self.pointer[0], self.pointer[1]))
                    .then(|| {
                        node.element
                            .attrs
                            .get("description")
                            .filter(|text| !text.is_empty())
                            .map(|text| (ara_ui::source::stable_id(&node.element.id), text.clone()))
                    })
                    .flatten()
                })
            })
            .flatten();
        if target != self.tooltip_target {
            self.tooltip_target = target;
            self.tooltip_text = None;
            self.tooltip_delay.reset();
            if let Some(i) = self.ui.find("tooltip") {
                self.ui.set_visible(i, false)?;
            }
        }
        self.advance_tooltip()
    }
    /// Advance a pending hover on idle frames without rescanning the editor tree.
    pub(super) fn advance_tooltip(&mut self) -> ara_ui::Result<()> {
        let now = self
            .tooltip_clock
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        self.advance_tooltip_at(now)
    }
    /// Deterministic clock seam shared by idle advancement and offscreen journeys.
    pub(super) fn advance_tooltip_at(&mut self, now: u64) -> ara_ui::Result<()> {
        let target = self.tooltip_target.as_ref().map(|(id, _)| *id);
        if !self.tooltip_delay.update(target, now) || self.tooltip_text.is_some() {
            return Ok(());
        }
        let Some((anchor, text)) = self.tooltip_target.as_ref() else {
            return Ok(());
        };
        let Some(index) = self.ui.by_id(*anchor).filter(|i| self.ui.shown(*i)) else {
            return Ok(());
        };
        let anchor = self.ui.nodes[index].element.id.clone();
        let text = text.clone();
        if let Some(i) = self.ui.find("tooltip-text") {
            self.ui.set_text(i, &text)?;
        }
        if let Some(i) = self.ui.find("tooltip") {
            let node = &mut self.ui.nodes[i];
            node.element.attrs.insert("anchor".into(), anchor);
            node.element
                .attrs
                .insert("placement".into(), "bottom-start".into());
            node.paint_offset = [0.; 2];
            self.ui.set_visible(i, true)?;
        }
        self.tooltip_text = Some(text);
        Ok(())
    }
    /// Route input through retained hit testing, focus and native widgets.
    pub fn event(
        &mut self,
        event: InputEvent,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<()> {
        if matches!(event, InputEvent::PointerDown(_) | InputEvent::Wheel { .. })
            || (matches!(event, InputEvent::PointerMove(_))
                && (self.input.captured().is_some()
                    || self.input.scrolling()
                    || self.input.scrubbing()))
        {
            self.note_pointer_chord();
        }
        if let Some(host) = &mut self.toolkit_verify {
            host.event(&mut self.ui, &mut self.input, event, clipboard)?;
            self.native_input
                .sync_focus(&mut self.ui, &self.input, clipboard)?;
            return Ok(());
        }
        if self.confirmations.is_open() {
            if self.current.is_none() {
                self.confirmations
                    .event(&mut self.ui, &mut self.input, event, clipboard)?;
            }
            self.native_input
                .sync_focus(&mut self.ui, &self.input, clipboard)?;
            return Ok(());
        }
        if let Some(event) = self.choice_event(event, clipboard)? {
            self.input.event(&mut self.ui, event, clipboard)?;
        } else {
            self.choice_input_dirty = true;
        }
        self.native_input
            .sync_focus(&mut self.ui, &self.input, clipboard)?;
        self.sync_scroll_views()
    }
    /// Dispatch queued widget actions through the typed engine endpoint.
    pub fn dispatch(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if let Some(host) = &mut self.toolkit_verify {
            let changed = host.dispatch(&mut self.ui, &mut self.input)?;
            if changed {
                self.rebuild(editor)?;
            }
            return Ok(changed);
        }
        if self.current.is_none() {
            self.confirmations.dispatch(&mut self.ui, &mut self.input)?;
        }
        let choices_changed = self.dispatch_choices(editor)?;
        let actions: Vec<_> = self.ui.drain_actions().collect();
        let bindings = std::mem::take(&mut self.bindings);
        let mut rebuild = choices_changed
            || std::mem::take(&mut self.choice_input_dirty)
            || self.sync_collections();
        let mut committed_numeric = false;
        let result = (|| -> ara_ui::Result<()> {
            for mut action in actions {
                if action.name == "ResizeAssets" {
                    if let Some(ratio) = action.value.as_f64()
                        && let Some(i) = self.ui.find("assets-content")
                    {
                        let width = self.ui.nodes[i].rect.w * ratio as f32;
                        editor.action("editor.workspace", json!({"sidebarWidth":width}))?;
                        rebuild = true;
                    }
                    continue;
                }
                if action.name == "ResizeDock" {
                    if let Some(index) = self.ui.by_id(action.node) {
                        let id = &self.ui.nodes[index].element.id;
                        let prefix = self.current.as_deref().map_or_else(
                            || "splitter-root".to_owned(),
                            |window| format!("splitter-f{window}"),
                        );
                        // The maximized main view is a temporary tree (`max`),
                        // while each floating surface keeps its own persisted root.
                        if let Some(path) = id.strip_prefix(&prefix) {
                            let path = if path.is_empty() {
                                Some(Vec::new())
                            } else {
                                path.strip_prefix('-').and_then(|path| {
                                    path.split('-')
                                        .map(|part| match part {
                                            "a" => Some(false),
                                            "b" => Some(true),
                                            _ => None,
                                        })
                                        .collect::<Option<Vec<_>>>()
                                })
                            }
                            .ok_or("Invalid dock splitter path")?;
                            if let Some(ratio) = action.value.as_f64() {
                                editor.action(
                                    "editor.layout",
                                    json!({"window":self.current,"path":path,"ratio":ratio}),
                                )?;
                                rebuild = true;
                            }
                        }
                    }
                    continue;
                }
                let Some(binding) = bindings.get(&action.node) else {
                    continue;
                };
                // A native grip emits one numeric value on release; ordinary text
                // edits keep their draft protocol until Enter or focus moves.
                if action.value.is_number()
                    && self.ui.by_id(action.node).is_some_and(|i| {
                        matches!(self.ui.nodes[i].element.tag.as_str(), "field" | "input")
                    })
                {
                    action.value = action.value.to_string().into();
                    committed_numeric = true;
                }
                match binding {
                    Binding::Play(intent) => {
                        self.dispatch_play(editor, intent, action.value)?;
                        rebuild = true;
                    }
                    Binding::Collection(action) => {
                        self.collection_action(editor, action)?;
                        rebuild = true;
                    }
                    Binding::AssetRenameValue => {
                        if let Some(rename) = &mut self.asset_rename {
                            rename.name = action.value.as_str().unwrap_or("").into();
                        }
                    }
                    Binding::AssetFile(action_kind) => {
                        self.asset_file_action(editor, action_kind, action.value)?;
                        rebuild = true;
                    }
                    Binding::AssetFolder(path, arrow) => {
                        self.asset_folder_action(editor, path, *arrow)?;
                        rebuild = true;
                    }
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
                        let appended = if id == "field.set" {
                            match &target {
                                commands::Target::Text(target) => target
                                    .split_once('=')
                                    .and_then(|(path, _)| self.append_collection_target(path)),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        let close_palette = editor.state.palette;
                        self.remember(id);
                        self.menu = None;
                        self.add_component = None;
                        self.mnemonics = false;
                        if close_palette {
                            editor.action("editor.palette", json!({"open":false}))?;
                        }
                        self.execute(editor, id, &target)?;
                        if let Some((field, pointer)) = appended {
                            self.reveal_appended_collection(editor, &field, &pointer)?;
                        }
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
                        self.open_menu_from(
                            super::menu::Source::Assets(path.clone(), kind.clone()),
                            action.node,
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
                    Binding::AddComponentFilter => {
                        if let Some(add) = &mut self.add_component {
                            add.query = action.value.as_str().unwrap_or("").into();
                            add.active = 0;
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
                        self.open_menu_from(
                            super::menu::Source::Context(*kind, target.clone()),
                            action.node,
                        );
                        rebuild = true;
                    }
                    Binding::CameraFov => {
                        let orthographic = editor.state.camera.projection
                            == crate::state::CameraProjection::Orthographic;
                        let field = if orthographic {
                            "orthographicHeight"
                        } else {
                            "fovYDegrees"
                        };
                        if let Some(value) = action.value.as_f64() {
                            editor.action("editor.camera", json!({(field):value}))?;
                            rebuild = true;
                        } else {
                            let draft = if orthographic {
                                "@camera|orthographicHeight"
                            } else {
                                "@camera|fov"
                            };
                            self.drafts
                                .insert(draft.into(), action.value.as_str().unwrap_or("").into());
                        }
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
                        self.menu = None;
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
                    Binding::ColorPicker(target, hints) => {
                        self.toggle_color_picker(editor, target, *hints)?;
                        rebuild = true;
                    }
                    Binding::Picker(control) => {
                        rebuild |= self.picker_control(editor, *control, &action.value)?;
                    }
                    Binding::Diagnostic(diagnostic) => {
                        if let Some(record) = &diagnostic.location.record {
                            let mut args = json!({"entities":[record]});
                            if let Some(doc) = &diagnostic.location.doc {
                                args["doc"] = doc.clone().into();
                            }
                            editor.action("editor.select", args)?;
                            editor.action(
                                "editor.focus",
                                json!({"panel":"inspector","field":diagnostic.location.path}),
                            )?;
                        } else if let Some(file) = diagnostic.location.file.as_deref() {
                            self.open_script_location(
                                editor,
                                file,
                                diagnostic.location.line,
                                diagnostic.location.column,
                            )?;
                        } else if let Some(doc) = diagnostic.location.doc.as_deref()
                            && let Some(path) = editor.host.store().workspace().path_of(doc)
                        {
                            self.open_script_location(
                                editor,
                                path,
                                diagnostic.location.line,
                                diagnostic.location.column,
                            )?;
                        }
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
                        self.add_component = None;
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
                                self.field_commands(
                                    editor,
                                    &editor.state.selection,
                                    &path,
                                    json!(entity),
                                )?,
                                "Pick entity reference",
                                None,
                            )?;
                        } else {
                            self.select_hierarchy_item(editor, entity)?;
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
                            self.select_asset_item(editor, asset)?;
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
                        let fix = diagnostic.fixes.get(*index).ok_or("fix disappeared")?;
                        if let ara_diag::FixAction::Cli { args } = &fix.action {
                            self.dialog = Some(Dialog::cli_fix(&fix.title, args));
                        } else {
                            apply_fix(editor, diagnostic, *index)?;
                        }
                        rebuild = true;
                    }
                    Binding::PanelRetry(panel) => {
                        self.retry_panel(panel);
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
        if committed_numeric {
            rebuild |= self.commit_fields(editor)?;
        }
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
        if self.commit_play_fields(editor)? {
            self.rebuild(editor)?;
            return Ok(true);
        }
        if self.commit_picker_text(editor)? && self.drafts.keys().all(|k| k.starts_with("@picker|"))
        {
            self.rebuild(editor)?;
            return Ok(true);
        }
        if self.commit_asset_rename(editor)? {
            self.rebuild(editor)?;
            return Ok(true);
        }
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
        if let Some(text) = self.drafts.remove("@camera|orthographicHeight") {
            let height: f32 = text
                .trim()
                .parse()
                .map_err(|_| "Orthographic height needs a number of metres")?;
            editor.action("editor.camera", json!({"orthographicHeight":height}))?;
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
        let mut commands = self.nested_commands(editor, &editor.state.selection)?;
        if self.drafts.is_empty() && commands.is_empty() {
            return Ok(false);
        }
        for (key, text) in &self.drafts {
            if key.starts_with("@scrub|") || key.starts_with("@picker|") {
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
        let result = if commands.is_empty() {
            Ok(Value::Null)
        } else {
            editor.apply(commands, "Edit inspector values", None)
        };
        editor.end_field_gestures()?;
        result?;
        // A picker gesture's preview marker outlives a commit around it.
        self.drafts.retain(|key, _| key.starts_with("@picker|"));
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
            // `#RRGGBB` sRGB hex becomes linear channels; alpha keeps its value, and an
            // HDR colour its intensity (the hex is its base colour).
            let mut linear = compound::hex_linear(text)?;
            if let crate::inspector::Widget::Vector { hdr: true, .. } = field.widget {
                let channel = |n: usize| field.value.get(n).and_then(Value::as_f64).unwrap_or(0.);
                let (_, stops) =
                    super::color::math::split_hdr([channel(0), channel(1), channel(2)]);
                linear = linear.map(|c| super::color::math::stable(c * stops.exp2()));
            }
            let mut rgb: Vec<Value> = linear.into_iter().map(|v| json!(v)).collect();
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
        } else if matches!(field.widget, crate::inspector::Widget::Entity) && text.trim().is_empty()
        {
            Value::Null
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
        Ok(DraftEdit {
            field,
            axis,
            value,
            keeps_alpha: true,
        })
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
                    keeps_alpha: true,
                };
                if let Some(base) = view.entities().get(entity)
                    && let Some(record) = edit.applied(base.record.clone())
                {
                    records.insert(entity.to_owned(), record);
                }
            }
        }
        // The colour picker previews its whole value on the entities it edits.
        if let Some((field, value, entities)) = self
            .color_picker
            .as_ref()
            .and_then(|picker| picker.preview(&self.drafts))
        {
            let edit = DraftEdit {
                field,
                axis: None,
                value,
                keeps_alpha: false,
            };
            for entity in entities {
                if let Some(base) = view.entities().get(entity)
                    && let Some(record) = edit.applied(base.record.clone())
                {
                    records.insert(entity.clone(), record);
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
    /// A whole colour keeps the record's alpha (typed hex has none); the picker's
    /// value carries its own.
    keeps_alpha: bool,
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
            None if self.keeps_alpha
                && matches!(
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
