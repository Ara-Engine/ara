//! Shared native dialogs and deferred host actions; polling never waits on file IO.
use super::*;
use ara_ui::dialogs::{DialogFuture, DialogOptions};

pub(super) enum Pending {
    Command {
        id: String,
        target: commands::Target,
        response: DialogFuture<bool>,
    },
    Quit(DialogFuture<bool>),
    SavingQuit,
    Alert(DialogFuture<()>),
}

impl Shell {
    /// Whether native viewport, context-menu and drag handlers must yield input
    /// to a modal before interpreting it as an editor gesture.
    pub(crate) fn modal_active(&self) -> bool {
        self.confirmations.is_open() || self.modal_root.is_some()
    }

    pub(super) fn confirmation_key(
        &mut self,
        editor: &mut Editor,
        key: i32,
        pressed: bool,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<bool> {
        use ara_ui::{
            input::{Direction, InputEvent},
            widgets::Widget,
        };
        use ara_ui_text::EditEvent;
        if !pressed || self.current.is_some() {
            return Ok(true);
        }
        let field = self
            .input
            .focused()
            .is_some_and(|id| matches!(self.ui.widgets.get(&id), Some(Widget::Input(_))));
        let [shift, ctrl, alt] = self.modifiers;
        let event = match key {
            41 => Some(InputEvent::Cancel),
            40 | 88 => Some(InputEvent::Activate),
            44 if !field => Some(InputEvent::Activate),
            43 => Some(InputEvent::Tab { reverse: shift }),
            79 | 80 if field => Some(InputEvent::Edit(EditEvent::Move {
                forward: key == 79,
                select: shift,
            })),
            79 => Some(InputEvent::Navigate(Direction::Right)),
            80 => Some(InputEvent::Navigate(Direction::Left)),
            81 => Some(InputEvent::Navigate(Direction::Down)),
            82 => Some(InputEvent::Navigate(Direction::Up)),
            4 if field && ctrl => Some(InputEvent::Edit(EditEvent::SelectAll)),
            6 if field && ctrl => Some(InputEvent::Edit(EditEvent::Copy)),
            27 if field && ctrl => Some(InputEvent::Edit(EditEvent::Cut)),
            25 if field && ctrl => Some(InputEvent::Edit(EditEvent::Paste)),
            29 if field && ctrl => Some(InputEvent::Edit(if shift {
                EditEvent::Redo
            } else {
                EditEvent::Undo
            })),
            42 if field => Some(InputEvent::Edit(EditEvent::Backspace)),
            76 if field => Some(InputEvent::Edit(EditEvent::Delete)),
            _ => None,
        };
        if let Some(event) = event {
            self.event(event, clipboard)?;
            self.poll_dialogs(editor)?;
            Ok(true)
        } else {
            Ok(!field || ctrl || alt || (58..=69).contains(&key))
        }
    }

    /// Intercept UI actions that need a human decision. Authoring commands which
    /// already support undo, including entity deletion, remain immediate.
    pub(crate) fn confirm_command(
        &mut self,
        editor: &mut Editor,
        id: &str,
        target: &commands::Target,
    ) -> ara_ui::Result<bool> {
        if self.confirmation_bypass {
            return Ok(false);
        }
        if id == "file.exit" {
            if self.request_quit(editor)? {
                self.quit_approved = true;
                self.window_request = Some(WindowRequest::Quit);
            }
            return Ok(true);
        }
        if id != "layout.reset" {
            return Ok(false);
        }
        if self
            .pending_dialogs
            .iter()
            .any(|p| matches!(p, Pending::Command { id: pending, .. } if pending == id))
        {
            return Ok(true);
        }
        let mut options = DialogOptions::new(
            "Reset editor layout?",
            "Restore the default panel arrangement and close floating windows. Save the current layout first if you want to keep it.",
        );
        options.accept_label = "Reset layout".into();
        options.destructive = true;
        let response = self.confirmations.confirm(options);
        self.pending_dialogs.push(Pending::Command {
            id: id.into(),
            target: target.clone(),
            response,
        });
        self.menu = None;
        Ok(true)
    }

    /// Ask before closing with unsaved content or field drafts. True lets the app
    /// close now; false leaves a native dialog or background save in progress.
    pub(crate) fn request_quit(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if std::mem::take(&mut self.quit_approved) {
            return Ok(true);
        }
        if self
            .pending_dialogs
            .iter()
            .any(|p| matches!(p, Pending::Quit(_) | Pending::SavingQuit))
        {
            return Ok(false);
        }
        self.dispatch(editor)?;
        let files = editor.action("session.status", json!({}))?;
        let unsaved = editor.host.unsaved()
            || files["files"]["unsaved"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
            || files["files"]["diverged"]
                .as_object()
                .is_some_and(|v| !v.is_empty())
            || !self.drafts.is_empty()
            || self.scrub.is_some();
        if !unsaved {
            return Ok(true);
        }
        let mut options = DialogOptions::new(
            "Save changes before closing?",
            "Save pending field edits and wait for project documents to finish writing. Keep editing to return to the editor.",
        );
        options.accept_label = "Save and close".into();
        options.cancel_label = "Keep editing".into();
        let response = self.confirmations.confirm(options);
        self.pending_dialogs.push(Pending::Quit(response));
        self.menu = None;
        self.rebuild(editor)?;
        Ok(false)
    }

    /// Keep the editor usable after a failed action, with the same diagnostic in
    /// Problems and a dismissible shared modal. Repeated failures do not stack.
    pub(crate) fn alert_error(
        &mut self,
        editor: &mut Editor,
        error: &(dyn std::error::Error + 'static),
    ) {
        editor.report_failure(error);
        let message = error.to_string();
        self.status_note = Some(message.clone());
        if self.dialog_error.is_some() {
            return;
        }
        let mut options = DialogOptions::new(
            "Action could not complete",
            format!(
                "{message}\n\nYour editor is still running. Details are available in Problems."
            ),
        );
        options.accept_label = "Continue editing".into();
        let response = self.confirmations.alert(options);
        self.pending_dialogs.push(Pending::Alert(response));
        self.dialog_error = Some(message);
    }

    /// Drain native dialog completions once per app frame. Accepted saves use the
    /// host's existing file worker and debounce; no synchronous flush runs here.
    pub(crate) fn poll_dialogs(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let returning = self.current.clone();
        self.leave();
        let result = self.poll_dialogs_main(editor);
        if let Some(id) = returning {
            self.enter(&id);
        }
        result
    }

    fn poll_dialogs_main(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = false;
        for _ in 0..8 {
            let Some(pending) = self.pending_dialogs.last_mut() else {
                break;
            };
            let accepted = match pending {
                Pending::Command { response, .. } | Pending::Quit(response) => response.try_take(),
                Pending::Alert(response) => response.try_take().map(|()| true),
                Pending::SavingQuit => {
                    if editor.host.unsaved() {
                        break;
                    }
                    Some(true)
                }
            };
            let Some(accepted) = accepted else {
                break;
            };
            let Some(pending) = self.pending_dialogs.pop() else {
                break;
            };
            changed = true;
            match pending {
                Pending::Command { id, target, .. } if accepted => {
                    self.confirmation_bypass = true;
                    let result = self.execute(editor, &id, &target);
                    self.confirmation_bypass = false;
                    if let Err(error) = result {
                        self.alert_error(editor, &*error);
                    }
                }
                Pending::Quit(_) if accepted => {
                    let result = self
                        .finish_scrub(editor, false)
                        .and_then(|_| self.commit_fields(editor))
                        .and_then(|_| {
                            editor
                                .finish_gesture(false)
                                .map(|_| true)
                                .map_err(Into::into)
                        });
                    match result {
                        Ok(_) => {
                            self.status_note = Some("Saving changes before closing…".into());
                            self.pending_dialogs.push(Pending::SavingQuit);
                        }
                        Err(error) => self.alert_error(editor, &*error),
                    }
                }
                Pending::SavingQuit => {
                    let status = editor.action("session.status", json!({}))?;
                    let dirty = status["files"]["unsaved"]
                        .as_array()
                        .is_some_and(|v| !v.is_empty());
                    let diverged = status["files"]["diverged"]
                        .as_object()
                        .is_some_and(|v| !v.is_empty());
                    if dirty || diverged {
                        let error: Box<dyn std::error::Error> = "Project changes are not saved. Resolve the file conflict or write error in Problems, then close again.".into();
                        self.alert_error(editor, &*error);
                    } else {
                        self.quit_approved = true;
                        self.window_request = Some(WindowRequest::Quit);
                    }
                }
                Pending::Alert(_) => self.dialog_error = None,
                _ => {}
            }
        }
        if changed {
            self.rebuild(editor)?;
        }
        Ok(changed)
    }

    /// Replace a modal's actual root identity rather than treating every modal as
    /// one boolean. Palette → dialog and dialog → palette preserve a valid trap.
    pub(super) fn sync_modal_scopes(&mut self, editor: &Editor) -> ara_ui::Result<()> {
        let desired = if self.dialog.is_some() {
            Some(("dialog", self.dialog.as_ref().and_then(Dialog::field)))
        } else if editor.state.palette {
            Some(("palette", Some("palette-search")))
        } else if editor.state.canvas_add && self.ui.find("node-palette").is_some() {
            Some(("node-palette", Some("search-nodes")))
        } else {
            None
        };
        let root = desired.map(|(root, _)| root);
        if root != self.modal_root {
            self.confirmations.suspend(&mut self.ui, &mut self.input);
            if self.modal_root.is_some() {
                self.input.pop(&mut self.ui);
            }
            if let Some((root, field)) = desired {
                self.panel_opened = (!self.reduce_motion).then(std::time::Instant::now);
                self.input.push(
                    &mut self.ui,
                    ara_ui::source::stable_id(root),
                    field.map(ara_ui::source::stable_id),
                    true,
                )?;
            }
            self.modal_root = root;
        }
        self.confirmations.sync(&mut self.ui, &mut self.input)
    }
}
