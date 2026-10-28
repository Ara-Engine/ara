//! Each natural move or lens gesture has its own single content Undo/Redo.
use super::*;
impl PilotCheck {
    pub(super) fn movement(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if !self.palette(shell, window, scale, "view.camera.pilot", events)? {
            return Ok(());
        }
        match self.phase {
            4 => {
                if !shell.pilot_active() || !scene_ready {
                    return Ok(());
                }
                self.before = fixture::records(editor)?;
                self.revision = editor.host.store().revision();
                self.baseline = editor.state.camera;
                if self.index == 1 {
                    let node = shell
                        .shown_node("view-fov")
                        .ok_or("Native lens field absent")?;
                    let rect = shell.ui.nodes[node].rect;
                    self.grip = [rect.x + rect.w - 8., rect.y + rect.h * 0.5];
                    self.motion(shell, window, scale, self.grip, [0.; 2], events)?;
                    native::button(window, true, events)?;
                } else {
                    let point = Self::viewport_point(shell)?;
                    self.motion(shell, window, scale, point, [0.; 2], events)?;
                    Self::secondary(window, true, events)?;
                    self.motion(
                        shell,
                        window,
                        scale,
                        [point[0] + 20., point[1]],
                        [20., 0.],
                        events,
                    )?;
                    native::key(window, 26, true, events)?;
                    native::key(window, 7, true, events)?;
                    self.holding = true;
                    self.held = 0;
                }
                self.next();
            }
            5 => {
                verify::unchanged(editor, &self.before, self.revision)?;
                if self.index == 1 {
                    self.motion(
                        shell,
                        window,
                        scale,
                        [self.grip[0] + 16., self.grip[1]],
                        [0.; 2],
                        events,
                    )?;
                    self.next();
                } else if self.held >= HOLD {
                    self.moved = editor.state.camera;
                    if self.moved == self.baseline || shell.pilot_report()["pending"] != true {
                        return Err(
                            "Native RMB/WASD did not produce a transient owned Pilot move".into(),
                        );
                    }
                    if self.index == 2 {
                        native::tap(window, 41, events)?;
                    } else {
                        native::key(window, 26, false, events)?;
                        native::key(window, 7, false, events)?;
                        Self::secondary(window, false, events)?;
                    }
                    self.holding = false;
                    self.next();
                }
            }
            6 => {
                if self.index == 1 {
                    verify::unchanged(editor, &self.before, self.revision)?;
                    native::button(window, false, events)?;
                    self.next();
                } else if self.index == 2 {
                    verify::unchanged(editor, &self.before, self.revision)?;
                    if editor.state.camera != self.baseline
                        || !shell.pilot_active()
                        || shell.pilot_report()["pending"] != false
                    {
                        return Err(
                            "Native Escape failed to restore the held Pilot view without authoring"
                                .into(),
                        );
                    }
                    native::key(window, 26, false, events)?;
                    native::key(window, 7, false, events)?;
                    Self::secondary(window, false, events)?;
                    self.next();
                } else {
                    self.check_committed(editor, shell, window, scale, events, scene_ready)?;
                }
            }
            7 => {
                if self.index == 2 {
                    verify::unchanged(editor, &self.before, self.revision)?;
                    if scene_ready {
                        self.done(json!({"cancelled":true,"authoringUnchanged":true,"heldFrames":self.held,"pilot":shell.pilot_report(),"restoredView":editor.state.camera}));
                    }
                } else if self.index == 1 {
                    self.check_committed(editor, shell, window, scale, events, scene_ready)?;
                } else {
                    native::shortcut(window, 29, events)?;
                    self.next();
                }
            }
            8 => {
                if self.index == 1 {
                    native::shortcut(window, 29, events)?;
                    self.next();
                } else {
                    self.redo(editor, window, events)?;
                }
            }
            9 => {
                if self.index == 1 {
                    self.redo(editor, window, events)?;
                } else {
                    self.check_redone(editor, shell, scene_ready)?;
                }
            }
            _ => self.check_redone(editor, shell, scene_ready)?,
        }
        Ok(())
    }
    fn check_committed(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if shell.pilot_report()["last"]["outcome"] != "committed" || !scene_ready {
            return Ok(());
        }
        self.moved = editor.state.camera;
        self.gesture = verify::committed(
            editor,
            &self.before,
            self.revision,
            &self.moved,
            self.index == 1,
        )?;
        self.committed = fixture::records(editor)?;
        self.pending_capture = Some("committed");
        // A normal Scene click blurs the numeric input before content shortcuts.
        let rect = shell.viewport().ok_or("Scene absent before content undo")?;
        let point = [rect.x + 30., rect.y + rect.h - 30.];
        self.motion(shell, window, scale, point, [0.; 2], events)?;
        native::button(window, true, events)?;
        native::button(window, false, events)?;
        self.next();
        Ok(())
    }
    fn redo(
        &mut self,
        editor: &Editor,
        window: &Window,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if fixture::records(editor)? != self.before {
            return Err(
                "One native Ctrl+Z did not restore the complete pre-gesture fixture".into(),
            );
        }
        let shift = 225;
        native::key(window, shift, true, events)?;
        native::shortcut(window, 29, events)?;
        native::key(window, shift, false, events)?;
        self.next();
        Ok(())
    }
    fn check_redone(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if fixture::records(editor)? != self.committed {
            return Err(
                "One native Ctrl+Shift+Z did not restore the complete committed fixture".into(),
            );
        }
        if scene_ready {
            self.done(json!({"gesture":self.gesture,"nativeUndoRedo":true,"heldFrames":self.held,
            "parentNonuniformScale":[2,3,2],"pilot":shell.pilot_report(),"ingress":"SDL palette, RMB/WASD or numeric grip, Scene focus and Ctrl+Z/Ctrl+Shift+Z"}));
        }
        Ok(())
    }
}
