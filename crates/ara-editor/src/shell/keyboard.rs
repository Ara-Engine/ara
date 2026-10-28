//! Physical keys route through, in order: shortcut recording, open menus, the
//! palette and dialogs, focused text fields, then the keymap (registry commands).
//! Alt (tapped alone) or F10 shows menu mnemonics.
use super::commands::{Target, keys::Chord, menus::MENUS};
use super::*;
use ara_ui::input::InputEvent;
use ara_ui_text::{Clipboard, EditEvent};

const MODIFIERS: [i32; 8] = [224, 225, 226, 227, 228, 229, 230, 231];
impl Shell {
    /// Route physical keyboard shortcuts, preserving a focused field's text undo.
    pub fn key(
        &mut self,
        editor: &mut Editor,
        scancode: i32,
        pressed: bool,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<bool> {
        if pressed {
            self.interaction.note();
        }
        if MODIFIERS.contains(&scancode) {
            self.native_input.key(
                &mut self.ui,
                &mut self.input,
                scancode,
                pressed,
                false,
                clipboard,
            )?;
            let modifiers = self.native_input.modifiers();
            self.modifiers = [
                modifiers.shift,
                if cfg!(target_os = "macos") {
                    modifiers.command
                } else {
                    modifiers.control
                },
                modifiers.alt,
            ];
        }
        if pressed && scancode == 41 && self.native_input.composing() {
            return self.native_input.key(
                &mut self.ui,
                &mut self.input,
                scancode,
                pressed,
                false,
                clipboard,
            );
        }
        if let Some(host) = &mut self.toolkit_verify {
            return host.key(
                &mut self.ui,
                &mut self.input,
                &mut self.native_input,
                scancode,
                pressed,
                clipboard,
            );
        }
        if pressed && scancode == 41 && self.input.scrubbing() {
            self.event(InputEvent::Cancel, clipboard)?;
            editor.end_field_gestures()?;
            return Ok(true);
        }
        if self.confirmations.is_open() {
            return self.confirmation_key(editor, scancode, pressed, clipboard);
        }
        if self.modal_root == Some("build-options") && self.current.is_some() {
            return Ok(true);
        }
        if self.choice_key(editor, scancode, pressed, clipboard)? {
            return Ok(true);
        }
        if self.palette_form_key(editor, scancode, pressed, clipboard)? {
            return Ok(true);
        }
        if self.modal_root == Some("build-options") {
            self.alt_tap = false;
            if !pressed {
                return Ok(true);
            }
            editor.input_active();
            if scancode == 41 {
                self.close_build_options(editor)?;
                return Ok(true);
            }
            // Selects have already had first refusal. Keep the remaining native
            // navigation inside the modal and swallow editor/global shortcuts.
            self.native_input.key(
                &mut self.ui,
                &mut self.input,
                scancode,
                true,
                false,
                clipboard,
            )?;
            self.dispatch(editor)?;
            return Ok(true);
        }
        if scancode == 226 || scancode == 230 {
            if pressed {
                self.alt_tap = !self.modifiers[0] && !self.modifiers[1];
            } else if std::mem::take(&mut self.alt_tap) && self.recording.is_none() {
                // Alt pressed and released alone toggles menu mnemonics.
                self.mnemonics = !self.mnemonics && self.menu.is_none();
                if !self.mnemonics {
                    self.menu = None;
                }
                self.rebuild(editor)?;
                return Ok(true);
            }
            return Ok(false);
        }
        if !pressed {
            return Ok(false);
        }
        self.alt_tap = false;
        let [shift, ctrl, alt] = self.modifiers;
        editor.input_active();
        if MODIFIERS.contains(&scancode) {
            return Ok(false);
        }
        let chord = Chord {
            ctrl,
            shift,
            alt,
            key: scancode,
        };
        if let Some(id) = self.recording.take() {
            if scancode != 41 {
                let mut map = editor
                    .state
                    .settings
                    .get("input.shortcuts")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                map.insert(id, chord.text().into());
                editor.action(
                    "editor.settings.set",
                    json!({"key":"input.shortcuts","value":map}),
                )?;
            }
            self.rebuild(editor)?;
            return Ok(true);
        }
        if self.key_search {
            self.key_search = false;
            if scancode != 41 {
                editor.action(
                    "editor.search",
                    json!({"panel":"shortcuts","query":chord.text()}),
                )?;
            }
            self.rebuild(editor)?;
            return Ok(true);
        }
        let field = self
            .input
            .focused()
            .and_then(|id| self.ui.by_id(id))
            .is_some_and(|i| matches!(self.ui.nodes[i].element.tag.as_str(), "field" | "input"));
        // A dialog owns its keys before menus or global shortcuts. Enter activates
        // the focused button; only an input uses the dialog's primary action.
        if self.dialog.is_some() {
            if scancode == 41 {
                self.dialog_action(editor, "close")?;
                self.rebuild(editor)?;
                return Ok(true);
            }
            if scancode == 40 || scancode == 88 {
                let action = self
                    .input
                    .focused()
                    .and_then(|id| self.bindings.get(&id))
                    .and_then(|binding| match binding {
                        Binding::DialogAction(action) => Some(*action),
                        _ => None,
                    })
                    .unwrap_or(if matches!(self.dialog, Some(Dialog::SaveLayout { .. })) {
                        "save"
                    } else {
                        "close"
                    });
                self.dispatch(editor)?;
                self.dialog_action(editor, action)?;
                self.rebuild(editor)?;
                return Ok(true);
            }
            if scancode == 43 {
                self.event(InputEvent::Tab { reverse: shift }, clipboard)?;
                return Ok(true);
            }
            if scancode == 44 && !field {
                self.event(InputEvent::Activate, clipboard)?;
                self.dispatch(editor)?;
                self.rebuild(editor)?;
                return Ok(true);
            }
            if alt || (58..=69).contains(&scancode) {
                return Ok(true);
            }
            if !field {
                return Ok(true);
            }
        }
        if self.menu.is_some() && self.menu_key(editor, scancode)? {
            return Ok(true);
        }
        if self.add_component.is_some() && self.add_component_key(editor, scancode)? {
            return Ok(true);
        }
        if self.mnemonics && self.menu.is_none() {
            if let Some(index) = MENUS.iter().position(|m| {
                Chord::parse(&m.mnemonic.to_string()).is_some_and(|c| c.key == scancode)
            }) {
                self.open_bar(index, true);
                self.mnemonics = false;
                self.rebuild(editor)?;
                return Ok(true);
            }
            if scancode == 41 || scancode == 67 {
                self.mnemonics = false;
                self.rebuild(editor)?;
                return Ok(true);
            }
        }
        if alt
            && !ctrl
            && !field
            && let Some(index) = MENUS.iter().position(|m| {
                Chord::parse(&m.mnemonic.to_string()).is_some_and(|c| c.key == scancode)
            })
        {
            // Alt+letter opens a menu unless the keymap binds it (Alt+1… toggles panels).
            if self.keymap.commands(chord).is_empty() {
                self.open_bar(index, true);
                self.rebuild(editor)?;
                return Ok(true);
            }
        }
        if scancode == 67 && !shift && !ctrl && !alt {
            // F10 activates the menu bar, as in every Windows application.
            self.open_bar(0, true);
            self.rebuild(editor)?;
            return Ok(true);
        }
        if editor.state.palette
            && self.palette_command.is_none()
            && matches!(scancode, 81 | 82 | 40 | 88)
        {
            if scancode == 40 || scancode == 88 {
                if let Some(id) = self.palette_row_id(self.palette_active) {
                    self.click(&id, clipboard)?;
                    self.dispatch(editor)?;
                }
                return Ok(true);
            }
            let count = self
                .ui
                .nodes
                .iter()
                .filter(|n| n.element.id.starts_with("command-") && n.element.tag == "button")
                .count();
            if count > 0 {
                self.palette_active = if scancode == 81 {
                    (self.palette_active + 1) % count
                } else {
                    (self.palette_active + count - 1) % count
                };
            }
            self.rebuild(editor)?;
            return Ok(true);
        }
        if matches!(scancode, 40 | 88) && self.asset_ops.rename_path().is_some() {
            // Apply the field's latest native text before lowering its rename to
            // the authority-owned file command; other Enter handlers must yield.
            self.dispatch(editor)?;
            if self.commit_asset_file_name(editor)? {
                self.rebuild(editor)?;
                return Ok(true);
            }
        }
        if scancode == 41 && self.cancel_asset_file_name() {
            self.rebuild(editor)?;
            return Ok(true);
        }
        if scancode == 41
            && !editor.state.palette
            && self.dialog.is_none()
            && self.picker_escape(editor)?
        {
            // Escape cancels the eyedropper or a drag, else restores the old colour.
            self.rebuild(editor)?;
            return Ok(true);
        }
        if scancode == 41 {
            if let Some(rename) = &mut self.asset_rename {
                rename.name = names::stem(names::file_name(&rename.path)).into();
                self.commit_asset_rename(editor)?;
            }
            self.finish_scrub(editor, true)?;
            editor.finish_gesture(true)?;
            let had_drag = self.drag.take().is_some();
            self.drop_preview = None;
            self.show_readout(None);
            self.pick_field = None;
            // Escape also dismisses a status message (a picking hint, a build note).
            self.status_note = None;
            self.drafts.clear();
            editor.end_field_gestures()?;
            self.rename = false;
            let had_overlay =
                self.menu.take().is_some() || self.dialog.take().is_some() || self.mnemonics;
            self.mnemonics = false;
            let had_modal = editor.state.canvas_add || editor.state.palette;
            if editor.state.canvas_add {
                editor.action("editor.canvas", json!({"add":false}))?;
            }
            if editor.state.palette {
                editor.action("editor.palette", json!({"open":false}))?;
            }
            self.event(InputEvent::Cancel, clipboard)?;
            // Esc only reaches the keymap (e.g. Unreal's Stop) when nothing else consumed it.
            if !had_drag
                && !had_overlay
                && !had_modal
                && !field
                && !self.selection_escape(editor)?
            {
                self.run_chord(editor, chord)?;
            }
            self.rebuild(editor)?;
            return Ok(true);
        }
        if scancode == 40
            && self.input.focused() == Some(ara_ui::source::stable_id("material-source"))
        {
            if ctrl {
                self.dispatch(editor)?;
                self.apply_resource_text(editor)?;
                self.rebuild(editor)?;
            } else {
                self.event(InputEvent::Edit(EditEvent::Insert("\n".into())), clipboard)?;
            }
            return Ok(true);
        }
        if scancode == 40 {
            if self.asset_folder_key(editor, scancode)? {
                return Ok(true);
            }
            // The field's pending action and its commit rebuild once, not twice.
            let committed = self.coalesce_rebuilds(editor, |shell, editor| {
                shell.dispatch(editor)?;
                shell.commit_fields(editor)
            })?;
            if !committed {
                self.event(InputEvent::Activate, clipboard)?;
            }
            return Ok(true);
        }
        if scancode == 43 {
            self.commit_fields(editor)?;
            self.event(InputEvent::Tab { reverse: shift }, clipboard)?;
            return Ok(true);
        }
        if field {
            if matches!(
                scancode,
                4 | 6 | 27 | 25 | 29 | 28 | 42 | 76 | 79 | 80 | 74 | 77
            ) && self.native_input.key(
                &mut self.ui,
                &mut self.input,
                scancode,
                true,
                false,
                clipboard,
            )? {
                return Ok(true);
            }
            // Plain typing belongs to the field; chords and function keys stay global.
            let function = (58..=69).contains(&scancode);
            if !ctrl && !alt && !function {
                return Ok(false);
            }
        }
        if self.dialog.is_some() {
            return Ok(true);
        }
        if self.asset_folder_key(editor, scancode)? {
            return Ok(true);
        }
        if (scancode == 67 && shift) || scancode == 101 {
            // Shift+F10 or the Menu key: context menu for the focused control.
            if let Some(i) = self.input.focused().and_then(|id| self.ui.by_id(id)) {
                let r = self.ui.nodes[i].rect;
                self.pointer = [r.x + 8., r.y + r.h * 0.5];
            }
            return self.open_context(editor);
        }
        if !alt
            && !field
            && (self.asset_selection_key(editor, scancode)?
                || self.hierarchy_key(editor, scancode)?)
        {
            return Ok(true);
        }
        self.run_chord(editor, chord)
    }
    /// Run the keymap command for a chord; conflicts prefer the focused context.
    fn run_chord(&mut self, editor: &mut Editor, chord: Chord) -> ara_ui::Result<bool> {
        let mut candidates = self.keymap.commands(chord).to_vec();
        if candidates.is_empty() {
            return Ok(false);
        }
        let graph =
            matches!(editor.state.focus.as_str(), "material" | "animation") && self.graph_visible();
        candidates.sort_by_key(|id| id.starts_with("graph.") != graph);
        let target = Target::None;
        let Some(id) = candidates.into_iter().find(|id| {
            commands::lookup(&editor.extensions, id)
                .is_some_and(|c| c.avail(&self.ctx(editor, &target)).ready())
        }) else {
            return Ok(false);
        };
        self.remember(&id);
        self.run_command(editor, &id, &target)?;
        Ok(true)
    }
}

#[cfg(test)]
mod dialog_tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Project(PathBuf);
    impl Project {
        fn new() -> ara_ui::Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let target = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
            let root = target.join(format!(
                "dialog-keys-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
            for (path, value) in [
                (
                    "ara.project.json",
                    json!({"name":"Dialog keys","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
                ),
                (
                    "levels/main.scene",
                    json!({"$schema":"ara://schema/scene/1","id":"sc_dialog00","records":{}}),
                ),
            ] {
                let physical = if path == "ara.project.json" {
                    root.join(path)
                } else {
                    root.join(ara_doc::AUTHORING_ROOT).join(path)
                };
                std::fs::write(physical, ara_json::to_canonical_string(&value))?;
            }
            Ok(Self(root))
        }
    }
    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct Clip;
    impl Clipboard for Clip {
        fn read(&mut self) -> ara_ui_text::Result<String> {
            Ok(String::new())
        }
        fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn build_options_traps_keys_stacks_select_and_dialog_then_restores_focus() -> ara_ui::Result<()>
    {
        let project = Project::new()?;
        let mut editor = Editor::open(&project.0)?;
        let mut shell = Shell::new(&mut editor, [900., 600.])?;
        // Drain the initial worker handoff before installing this modal's cached
        // index fixture. No export/profile creation or native file chooser runs.
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shell.browser.revision == 0
            && shell.browser.files.is_empty()
            && shell.browser.error.is_none()
            && std::time::Instant::now() < limit
        {
            shell.browser.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(shell.browser.revision > 0 || !shell.browser.files.is_empty());
        shell.browser.files = ["Windows shipping", "Mobile development"]
            .into_iter()
            .map(|name| {
                let path = format!("settings/builds/{name}.export.json");
                ara_host::project::AuthoredFile {
                    storage_path: path.clone(),
                    path,
                    scope: ara_host::project::AuthoredScope::Project,
                    role: ara_host::project::AuthoredRole::BuildPreset,
                }
            })
            .collect();
        shell.browser.revision += 1;
        let outside = ara_ui::source::stable_id("search-hierarchy");
        shell.input.focus(&mut shell.ui, outside)?;
        let revision = editor.host.store().revision();

        shell.dialog = Some(Dialog::save_layout());
        shell.rebuild(&mut editor)?;
        let decision_focus = shell.input.focused();
        assert!(shell.open_build_options(&mut editor).is_err());
        assert!(shell.dialog.is_some());
        assert_eq!(shell.modal_root, Some("dialog"));
        assert_eq!(shell.input.focused(), decision_focus);
        assert!(!shell.build_options_open());
        shell.key(&mut editor, 41, true, &mut Clip)?;
        assert_eq!(shell.input.focused(), Some(outside));
        editor.action("editor.palette", json!({"open":true}))?;
        shell.rebuild(&mut editor)?;
        assert_eq!(
            shell.modal_return_focus,
            Some(outside),
            "palette return focus"
        );
        shell.open_build_options(&mut editor)?;
        assert!(
            !editor.state.palette,
            "Opening Build closes the palette through navigation"
        );
        assert_eq!(shell.modal_root, Some("build-options"));
        assert_eq!(shell.modal_return_focus, Some(outside));
        assert_eq!(
            shell.input.focused(),
            Some(ara_ui::source::stable_id("build-profile-select"))
        );
        assert!(shell.input.focus(&mut shell.ui, outside).is_err());
        shell.key(&mut editor, 81, true, &mut Clip)?;
        assert!(shell.choice_open());
        let search = shell.input.focused();
        assert_eq!(
            search,
            Some(ara_ui::source::stable_id(
                "choice-build-profile-select-search"
            ))
        );

        let mut alert = shell
            .confirmations
            .alert(ara_ui::dialogs::DialogOptions::new(
                "Profile refresh",
                "A stacked notice keeps the build form and its Select intact.",
            ));
        shell.rebuild(&mut editor)?;
        let stacked_focus = shell.input.focused();
        assert!(shell.open_build_options(&mut editor).is_err());
        assert_eq!(shell.input.focused(), stacked_focus);
        assert!(shell.confirmations.is_open());
        shell.key(&mut editor, 41, true, &mut Clip)?;
        assert_eq!(alert.try_take(), Some(()));
        assert!(shell.build_options_open());
        assert!(shell.choice_open());
        assert_eq!(shell.input.focused(), search);
        shell.key(&mut editor, 41, true, &mut Clip)?;
        assert!(!shell.choice_open());
        assert!(shell.build_options_open());

        shell.key(&mut editor, 43, true, &mut Clip)?;
        assert!(shell.input.focused().is_some());
        assert!(shell.input.focus(&mut shell.ui, outside).is_err());
        shell.key(&mut editor, 67, true, &mut Clip)?;
        assert!(shell.menu.is_none(), "F10 stays inside build options");
        shell.key(&mut editor, 41, true, &mut Clip)?;
        assert!(!shell.build_options_open());
        assert_eq!(shell.modal_root, None);
        assert_eq!(
            shell.input.focused(),
            Some(outside),
            "return focus {:?}; target {:?}; input {}",
            shell.modal_return_focus,
            shell.ui.find("search-hierarchy").map(|i| (
                &shell.ui.nodes[i].rect,
                &shell.ui.nodes[i].clip,
                shell.ui.shown(i)
            )),
            shell.input.focus_graph(&shell.ui)
        );

        shell.open_build_options(&mut editor)?;
        shell.key(&mut editor, 81, true, &mut Clip)?;
        assert!(shell.choice_open());
        for event in [
            InputEvent::PointerDown([0., 0.]),
            InputEvent::PointerUp([0., 0.]),
        ] {
            shell.event(event, &mut Clip)?;
            shell.dispatch(&mut editor)?;
        }
        assert!(!shell.choice_open());
        assert!(
            shell.build_options_open(),
            "The first backdrop click dismisses only the Select"
        );
        shell.event(InputEvent::PointerDown([0., 0.]), &mut Clip)?;
        shell.dispatch(&mut editor)?;
        assert!(!shell.build_options_open());
        assert_eq!(shell.input.focused(), Some(outside));
        assert_eq!(editor.host.store().revision(), revision);
        Ok(())
    }

    #[test]
    fn focused_cancel_enter_dismisses_without_saving_or_running_global_shortcuts()
    -> ara_ui::Result<()> {
        let project = Project::new()?;
        let mut editor = Editor::open(&project.0)?;
        let mut shell = Shell::new(&mut editor, [900., 600.])?;
        shell.dialog = Some(Dialog::save_layout());
        shell.rebuild(&mut editor)?;
        shell
            .input
            .focus(&mut shell.ui, ara_ui::source::stable_id("dialog-cancel"))?;
        shell.key(&mut editor, 40, true, &mut Clip)?;
        assert!(shell.dialog.is_none());
        assert!(!project.0.join(".ara/layouts").exists());
        shell.open_bar(0, false);
        shell.dialog = Some(Dialog::about());
        shell.rebuild(&mut editor)?;
        shell.key(&mut editor, 67, true, &mut Clip)?;
        assert!(shell.dialog.is_some(), "F10 must not escape a modal");
        shell.key(&mut editor, 41, true, &mut Clip)?;
        assert!(shell.dialog.is_none());
        assert!(shell.menu.is_some(), "Escape closes only the active dialog");
        Ok(())
    }

    #[test]
    fn modal_root_replacement_restores_the_actual_palette_scope() -> ara_ui::Result<()> {
        let project = Project::new()?;
        let mut editor = Editor::open(&project.0)?;
        let mut shell = Shell::new(&mut editor, [900., 600.])?;
        let outside = ara_ui::source::stable_id("search-hierarchy");
        shell.input.focus(&mut shell.ui, outside)?;
        editor.action("editor.palette", json!({"open":true}))?;
        shell.rebuild(&mut editor)?;
        assert_eq!(shell.modal_root, Some("palette"));
        assert_eq!(shell.modal_return_focus, Some(outside));
        shell.dialog = Some(Dialog::save_layout());
        shell.rebuild(&mut editor)?;
        assert_eq!(shell.modal_root, Some("dialog"));
        shell
            .input
            .focus(&mut shell.ui, ara_ui::source::stable_id("dialog-cancel"))?;
        shell.dialog = None;
        shell.rebuild(&mut editor)?;
        assert_eq!(shell.modal_root, Some("palette"));
        shell
            .input
            .focus(&mut shell.ui, ara_ui::source::stable_id("palette-search"))?;
        editor.action("editor.palette", json!({"open":false}))?;
        shell.rebuild(&mut editor)?;
        assert_eq!(shell.modal_root, None);
        assert_eq!(shell.input.focused(), Some(outside));
        Ok(())
    }

    #[test]
    fn unsaved_quit_cancel_preserves_drafts_and_accept_commits_before_close() -> ara_ui::Result<()>
    {
        let project = Project::new()?;
        let mut editor = Editor::open(&project.0)?;
        let mut shell = Shell::new(&mut editor, [900., 600.])?;
        shell.drafts.insert("@camera|fov".into(), "65".into());
        assert!(!shell.request_quit(&mut editor)?);
        shell.key(&mut editor, 41, true, &mut Clip)?;
        assert_eq!(
            shell.drafts.get("@camera|fov").map(String::as_str),
            Some("65")
        );
        assert!(shell.window_request.is_none());
        assert!(!shell.request_quit(&mut editor)?);
        shell.key(&mut editor, 40, true, &mut Clip)?;
        assert!(shell.drafts.is_empty());
        assert_eq!(editor.state.camera.fov_y_degrees, 65.);
        // Host writes may still be pending; ordinary frames drive completion.
        editor.host.flush();
        shell.poll_dialogs(&mut editor)?;
        assert_eq!(shell.window_request, Some(WindowRequest::Quit));
        assert!(shell.request_quit(&mut editor)?);
        Ok(())
    }
}
