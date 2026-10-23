use super::*;
use ara_ui::input::InputEvent;
use ara_ui_text::{Clipboard, EditEvent};
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
        if !pressed {
            return Ok(false);
        }
        let [shift, ctrl, _] = self.modifiers;
        editor.input_active();
        let field = self
            .input
            .focused()
            .and_then(|id| self.ui.by_id(id))
            .is_some_and(|i| matches!(self.ui.nodes[i].element.tag.as_str(), "field" | "input"));
        if ctrl && (scancode == 14 || (shift && scancode == 19)) {
            self.palette_command = None;
            editor.action("editor.palette", json!({"open":!editor.state.palette}))?;
            self.rebuild(editor)?;
            return Ok(true);
        }
        if scancode == 41 {
            editor.finish_gesture(true)?;
            self.context = false;
            self.pick_field = None;
            self.drafts.clear();
            self.rename = false;
            if editor.state.palette {
                editor.action("editor.palette", json!({"open":false}))?;
            }
            self.event(InputEvent::Cancel, clipboard)?;
            self.rebuild(editor)?;
            return Ok(true);
        }
        if scancode == 40 {
            self.dispatch(editor)?;
            let committed = self.commit_fields(editor)?;
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
        if ctrl && (scancode == 29 || scancode == 28) {
            if field {
                self.event(
                    InputEvent::Edit(if scancode == 28 || shift {
                        EditEvent::Redo
                    } else {
                        EditEvent::Undo
                    }),
                    clipboard,
                )?;
            } else {
                editor.action(
                    if scancode == 28 || shift {
                        "history.redo"
                    } else {
                        "history.undo"
                    },
                    json!({}),
                )?;
                self.rebuild(editor)?;
            }
            return Ok(true);
        }
        if field {
            let edit = match (scancode, ctrl) {
                (4, true) => Some(EditEvent::SelectAll),
                (6, true) => Some(EditEvent::Copy),
                (27, true) => Some(EditEvent::Cut),
                (25, true) => Some(EditEvent::Paste),
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
            return Ok(false);
        }
        match scancode {
            9 => {
                editor.action("editor.frame", json!({}))?;
            }
            26 | 8 | 21 => {
                let tool = match scancode {
                    26 => crate::state::Tool::Translate,
                    8 => crate::state::Tool::Rotate,
                    _ => crate::state::Tool::Scale,
                };
                editor.action("editor.tool", json!({"tool":tool}))?;
            }
            76 => {
                editor.apply(
                    vec![json!({"cmd":"entity.delete","args":{"entities":editor.state.selection}})],
                    "Delete selection",
                    None,
                )?;
            }
            59 => self.rename = true,
            _ => return Ok(false),
        }
        self.rebuild(editor)?;
        Ok(true)
    }
}
