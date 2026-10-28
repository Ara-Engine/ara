//! Natural native gestures, plus explicitly labeled protocol ownership probes.
use super::*;
mod diagnostics;
mod movement;
impl PilotCheck {
    #[allow(
        clippy::too_many_arguments,
        reason = "independent native owners and presented geometry"
    )]
    pub(super) fn input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        scene: &SceneView,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.started.elapsed() > PATIENCE {
            return Err(format!(
                "Pilot case {} phase {} exceeded bounded readiness: {}",
                CASES[self.index],
                self.phase,
                self.snapshot(editor, shell)
            )
            .into());
        }
        if self.complete.is_some() || self.pending_capture.is_some() {
            return Ok(());
        }
        if self.phase == 0 {
            if !self.prepared {
                native::focus_lost(window, events)?;
                fixture::setup(editor, shell, self.index)?;
                self.prepared = true;
                return Ok(());
            }
            if !scene_ready
                || !scene.snap_ready(&editor.state.doc)
                || scene
                    .selected_bounds(
                        &[BOX.into(), SPHERE.into(), COOKED.into()],
                        &shell.outlines.channels,
                    )
                    .is_none()
            {
                return Ok(());
            }
            self.before = fixture::records(editor)?;
            self.revision = editor.host.store().revision();
            self.baseline = editor.state.camera;
            self.pending_capture = Some("before");
            self.next();
            return Ok(());
        }
        match self.index {
            0..=2 => self.movement(editor, shell, window, scale, events, scene_ready)?,
            3 => {
                if self.phase == 1 {
                    self.click(shell, window, scale, "scene-unpilot", events)?;
                    self.next();
                } else if !shell.pilot_active() && shell.shown_node("scene-unpilot").is_none() {
                    verify::unchanged(editor, &self.before, self.revision)?;
                    if !scene_ready {
                        return Ok(());
                    }
                    self.done(json!({"nativeBadge":true,"authoringUnchanged":true,"pilot":shell.pilot_report()}));
                }
            }
            4 | 6 => {
                if !self.palette(shell, window, scale, "view.camera.pilot", events)? {
                    return Ok(());
                }
                match self.phase {
                    4 => {
                        if !shell.pilot_active() {
                            return Ok(());
                        }
                        self.baseline = editor.state.camera;
                        if self.index == 4 {
                            let result = ara_proto::Endpoint::call(
                                &mut editor.host,
                                "editor.camera",
                                json!({"session":self.agent,"yaw":self.baseline.yaw+0.3}),
                            )?;
                            self.gesture = json!({"ingress":"Protocol agent presence probe; not a measured native gesture","result":result});
                        } else {
                            let result = ara_proto::Endpoint::call(
                                &mut editor.host,
                                "cmd.apply",
                                json!({"session":self.agent,"doc":self.doc,
                                "why":"Native Pilot ownership fixture: agent deletes selected Camera","commands":[{"cmd":"entity.delete","args":{"entities":[CAMERA]}}]}),
                            )?;
                            self.gesture = json!({"ingress":"Protocol agent deletion fixture; not a measured native gesture","result":result});
                            self.before = fixture::records(editor)?;
                            self.revision = editor.host.store().revision();
                        }
                        editor.sync();
                        self.next();
                    }
                    _ => {
                        verify::unchanged(editor, &self.before, self.revision)?;
                        if self.index == 4 {
                            if !shell.pilot_active() || editor.state.camera != self.baseline {
                                return Err(
                                    "Agent presence unexpectedly retargeted human Pilot/view"
                                        .into(),
                                );
                            }
                            if !editor.state.presence.contains_key(&self.agent) {
                                return Err(
                                    "Agent view probe did not publish actual presence".into()
                                );
                            }
                            self.done(json!({"agentPresenceIsolated":true,"probe":self.gesture,"pilot":shell.pilot_report(),"presence":editor.state.presence.get(&self.agent)}));
                        } else if !shell.pilot_active()
                            && shell.shown_node("scene-unpilot").is_none()
                        {
                            if self.before.get(CAMERA).is_some()
                                || shell.pilot_report()["last"]["authoringChanged"] != false
                            {
                                return Err("Deleted Pilot target was restored or clobbered".into());
                            }
                            if !scene_ready {
                                return Ok(());
                            }
                            self.done(json!({"targetAbsent":true,"noPilotMutationAfterAgentDelete":true,"probe":self.gesture,"pilot":shell.pilot_report(),"status":shell.status_note}));
                        }
                    }
                }
            }
            5 => {
                if self.phase == 1 {
                    if !shell.pilot_active() {
                        return Err(
                            "Unowned-view probe requires the preceding explicit Pilot".into()
                        );
                    }
                    editor.action("editor.camera", json!({"yaw":editor.state.camera.yaw+0.2}))?;
                    self.moved = editor.state.camera;
                    self.next();
                } else if !shell.pilot_active() && shell.shown_node("scene-unpilot").is_none() {
                    verify::unchanged(editor, &self.before, self.revision)?;
                    if editor.state.camera != self.moved {
                        return Err("Pilot cancellation overwrote an unowned view update".into());
                    }
                    if !scene_ready {
                        return Ok(());
                    }
                    self.done(json!({"ingress":"Explicit protocol human navigation outside native camera ownership; not agent private presence",
                        "unownedViewPreserved":true,"authoringUnchanged":true,"pilot":shell.pilot_report(),"status":shell.status_note}));
                }
            }
            7 => self.diagnostics(editor, shell, window, scale, events, scene_ready)?,
            _ => return Err("Pilot native case index invalid".into()),
        }
        Ok(())
    }
}
