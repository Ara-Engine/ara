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
        match scancode {
            225 | 229 => self.modifiers[0] = pressed,
            224 | 228 => self.modifiers[1] = pressed,
            226 | 230 => self.modifiers[2] = pressed,
            _ => {}
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
        if let Some(id) = self.recording {
            self.recording = None;
            if scancode != 41 {
                let mut map = editor
                    .state
                    .settings
                    .get("input.shortcuts")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                map.insert(id.into(), chord.text().into());
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
        if self.menu.is_some() && self.menu_key(editor, scancode)? {
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
                let id = format!("command-{}", self.palette_active);
                if self.ui.find(&id).is_some() {
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
        if scancode == 41 {
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
            if !had_drag && !had_overlay && !had_modal && !field {
                self.run_chord(editor, chord)?;
            }
            self.rebuild(editor)?;
            return Ok(true);
        }
        if (scancode == 40 || scancode == 88) && self.dialog.is_some() {
            self.dispatch(editor)?;
            self.dialog_action(editor, "save")?;
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
            let edit = match (scancode, ctrl) {
                (4, true) => Some(EditEvent::SelectAll),
                (6, true) => Some(EditEvent::Copy),
                (27, true) => Some(EditEvent::Cut),
                (25, true) => Some(EditEvent::Paste),
                (29, true) => Some(if shift {
                    EditEvent::Redo
                } else {
                    EditEvent::Undo
                }),
                (28, true) => Some(EditEvent::Redo),
                (42, _) => Some(EditEvent::Backspace),
                (76, _) => Some(EditEvent::Delete),
                (79, _) => Some(EditEvent::Move {
                    forward: true,
                    select: shift,
                }),
                (80, _) => Some(EditEvent::Move {
                    forward: false,
                    select: shift,
                }),
                _ => None,
            };
            if let Some(edit) = edit {
                self.event(InputEvent::Edit(edit), clipboard)?;
                return Ok(true);
            }
            // Plain typing belongs to the field; chords and function keys stay global.
            let function = (58..=69).contains(&scancode);
            if !ctrl && !alt && !function {
                return Ok(false);
            }
        }
        if (scancode == 67 && shift) || scancode == 101 {
            // Shift+F10 or the Menu key: context menu for the focused control.
            if let Some(i) = self.input.focused().and_then(|id| self.ui.by_id(id)) {
                let r = self.ui.nodes[i].rect;
                self.pointer = [r.x + 8., r.y + r.h * 0.5];
            }
            return self.open_context(editor);
        }
        self.run_chord(editor, chord)
    }
    /// Run the keymap command for a chord; conflicts prefer the focused context.
    fn run_chord(&mut self, editor: &mut Editor, chord: Chord) -> ara_ui::Result<bool> {
        let mut candidates: Vec<&'static str> = self.keymap.commands(chord).to_vec();
        if candidates.is_empty() {
            return Ok(false);
        }
        let graph =
            matches!(editor.state.focus.as_str(), "material" | "animation") && self.graph_visible();
        candidates.sort_by_key(|id| id.starts_with("graph.") != graph);
        let target = Target::None;
        let Some(id) = candidates.into_iter().find(|id| {
            commands::get(id).is_some_and(|c| c.avail(&self.ctx(editor, &target)).ready())
        }) else {
            return Ok(false);
        };
        self.remember(id);
        self.run_command(editor, id, &target)?;
        Ok(true)
    }
}
