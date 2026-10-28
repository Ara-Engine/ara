use super::*;
impl Shell {
    /// The Editor owns the real adapter. Taking/restoring its box never clones a capture.
    pub(crate) fn poll_profiler_target_state(
        &mut self,
        editor: &mut Editor,
    ) -> ara_ui::Result<bool> {
        let mut backend = editor.play_profiling.take();
        let result = match backend.as_mut() {
            Some(handle) => self.poll_profiler_targets(&editor.session, Some(handle.as_mut())),
            None => self.poll_profiler_targets(&editor.session, None),
        };
        editor.play_profiling = backend;
        let mut changed = result?;
        let now = Instant::now();
        if self.profiler_targets.editor_job.is_some()
            && self
                .profiler_targets
                .next_editor_poll
                .is_none_or(|next| now >= next)
        {
            self.profiler_targets.next_editor_poll = Some(now + POLL);
            let id = self
                .profiler_targets
                .editor_job
                .as_ref()
                .expect("pending editor capture");
            match editor.action("jobs.status", json!({"job":id})) {
                Ok(status) if status["status"] != "running" => {
                    self.profiler_targets.editor_job = None;
                    changed = true;
                    if status["status"] != "done" {
                        self.profiler_targets.error=Some("The editor capture save did not complete successfully; inspect its job diagnostic".into());
                    }
                }
                Err(error) => {
                    let message = format!("Editor capture job state unavailable: {error}");
                    changed |= self.profiler_targets.error.as_ref() != Some(&message);
                    self.profiler_targets.error = Some(message);
                }
                _ => {}
            }
        }
        Ok(changed)
    }

    pub(crate) fn choose_profiler_target(
        &mut self,
        editor: &mut Editor,
        value: Value,
    ) -> ara_ui::Result<()> {
        let selected = if value.is_null() {
            Target::Editor
        } else {
            Target::Player(serde_json::from_value::<Scope>(value)?)
        };
        if let Target::Player(scope) = &selected
            && !self
                .profiler_targets
                .rows
                .iter()
                .any(|row| &row.target == scope)
            && selected != self.profiler_targets.selected
        {
            return Err(
                "That player incarnation is no longer in the live profiler target list; refresh it"
                    .into(),
            );
        }
        if selected != self.profiler_targets.selected {
            self.input.cancel_interaction(&mut self.ui);
            self.profiler_targets.selected = selected;
            self.profiler_targets.frame = None;
            self.profiler_targets.compare = false;
            self.profiler_targets.follow = true;
            self.profiler_targets.next_poll = None;
            self.profiler_targets.next_snapshot = None;
            self.profile_scope = None;
            if editor.state.profiler_compare {
                editor.action("editor.workspace", json!({"compare":false}))?;
            }
        }
        self.poll_profiler_target_state(editor)?;
        self.rebuild(editor)
    }

    fn request_player_profile(
        &mut self,
        editor: &mut Editor,
        action: Action,
        purpose: Purpose,
    ) -> ara_ui::Result<()> {
        if let Some(reason) = self.profiler_targets.blocker() {
            return Err(reason.to_owned().into());
        }
        let Target::Player(target) = &self.profiler_targets.selected else {
            return Err("Choose a live player target".into());
        };
        let target = target.clone();
        let backend = editor
            .play_profiling
            .as_mut()
            .ok_or("The player profiling transport is not attached")?;
        let ticket = match backend.request(Request {
            session: editor.session.clone(),
            target: target.clone(),
            action,
            why: Some("Native Profiler target action".into()),
        }) {
            Ok(ticket) => ticket,
            Err(diagnostics) => {
                self.profiler_targets.error = Some(
                    diagnostics
                        .first()
                        .map_or("Player profiling request was refused", |diagnostic| {
                            diagnostic.message.as_str()
                        })
                        .into(),
                );
                self.profiler_targets.diagnostics = diagnostics.into_iter().take(16).collect();
                return self.rebuild(editor);
            }
        };
        self.profiler_targets.error = None;
        self.profiler_targets.pending = Some(Pending {
            ticket,
            target,
            purpose,
            session: editor.session.clone(),
        });
        self.profiler_targets.next_poll = None;
        if purpose == Purpose::Freeze {
            self.profiler_targets.follow = false;
        }
        self.rebuild(editor)
    }

    pub(crate) fn profiler_record_target(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.profiler_targets.selected == Target::Editor {
            if !editor.host.profiler().is_recording() {
                editor.profile_current = None;
            }
            self.profiler_record(editor)?;
            return self.rebuild(editor);
        }
        let action = if self.profiler_targets.recording() {
            Action::Stop
        } else {
            Action::Start
        };
        self.profiler_targets.follow = true;
        self.request_player_profile(editor, action, Purpose::Record)
    }
    pub(crate) fn profiler_snapshot_target(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.profiler_targets.selected == Target::Editor {
            return self.profiler_freeze_target(editor, false);
        }
        self.profiler_targets.follow = true;
        self.profiler_targets.next_snapshot = Some(Instant::now() + LIVE_SNAPSHOT);
        self.request_player_profile(editor, Action::Snapshot, Purpose::Snapshot)
    }
    pub(crate) fn profiler_freeze_target(
        &mut self,
        editor: &mut Editor,
        baseline: bool,
    ) -> ara_ui::Result<()> {
        if self.profiler_targets.selected == Target::Editor {
            if self.profiler_targets.editor_job.is_some() {
                return Err(
                    "An editor capture save is already pending; wait for its receipt".into(),
                );
            }
            let response = editor.action("editor.profile.freeze", json!({"baseline":baseline}))?;
            self.profiler_targets.editor_job = response["job"].as_str().map(str::to_owned);
            if baseline {
                self.profiler_targets.baseline = None;
            }
            self.profiler_targets.compare = false;
            return self.rebuild(editor);
        }
        self.request_player_profile(
            editor,
            Action::Freeze,
            if baseline {
                Purpose::Baseline
            } else {
                Purpose::Freeze
            },
        )
    }
    pub(crate) fn refresh_profiler_targets(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.profiler_targets.next_poll = None;
        self.poll_profiler_target_state(editor)?;
        self.rebuild(editor)
    }

    pub(crate) fn profiler_target_blocker(&self) -> Option<&str> {
        self.profiler_targets.blocker()
    }
    pub(crate) fn profiler_freeze_blocker(&self) -> Option<&str> {
        if self.profiler_targets.selected == Target::Editor
            && self.profiler_targets.editor_job.is_some()
        {
            Some("An editor capture save is already pending")
        } else {
            self.profiler_targets.blocker()
        }
    }
    pub(crate) fn profiler_compare_blocker(&self, editor: &Editor) -> Option<&str> {
        let current = if self.profiler_targets.selected == Target::Editor {
            editor.profile_current.is_some()
        } else {
            self.profiler_targets.capture().is_some()
        };
        if !current {
            return Some("Freeze or snapshot the selected target before comparing");
        }
        if self.profiler_targets.baseline.is_none() && editor.profile_baseline.is_none() {
            return Some("Set an explicitly scoped baseline first");
        }
        if self.profiler_targets.follow && self.profiler_targets.recording() {
            return Some("Freeze the selected player snapshot before comparing");
        }
        None
    }
    pub(crate) fn profiler_compare_active(&self, editor: &Editor) -> bool {
        if self.profiler_targets.selected == Target::Editor
            && self.profiler_targets.baseline.is_none()
        {
            editor.state.profiler_compare
        } else {
            self.profiler_targets.compare
        }
    }
    pub(crate) fn compare_profiler_targets(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if let Some(reason) = self.profiler_compare_blocker(editor) {
            return Err(reason.to_owned().into());
        }
        if self.profiler_targets.selected == Target::Editor
            && self.profiler_targets.baseline.is_none()
        {
            editor.action(
                "editor.workspace",
                json!({"compare":!editor.state.profiler_compare}),
            )?;
        } else {
            self.profiler_targets.compare = !self.profiler_targets.compare;
        }
        self.rebuild(editor)
    }

    pub(in crate::shell) fn bind_profiler_frame(&mut self, id: &str, frame: u64) {
        let serial = self
            .profiler_targets
            .capture()
            .map_or(0, |capture| capture.serial);
        self.bind_command(id,"profiler.frame",commands::Target::Text(ara_json::to_canonical_string(&json!({"target":self.profiler_targets.selected.value(),"capture":serial,"frame":frame}))));
    }
    pub(in crate::shell) fn profiler_frame_id(&self, frame: u64) -> String {
        if self.profiler_targets.selected == Target::Editor {
            return format!("profile-frame-{frame}");
        }
        let context = ara_json::to_canonical_string(&self.profiler_targets.selected.value());
        format!(
            "profile-frame-{:016x}-{}-{frame}",
            ara_ui::source::stable_id(&context),
            self.profiler_targets
                .capture()
                .map_or(0, |capture| capture.serial)
        )
    }
    pub(crate) fn choose_profiler_frame(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        let commands::Target::Text(text) = target else {
            return Err("Choose a profiler frame".into());
        };
        let value: Value = serde_json::from_str(text)?;
        let scope = if value["target"].is_null() {
            Target::Editor
        } else {
            Target::Player(serde_json::from_value(value["target"].clone())?)
        };
        let frame = value["frame"]
            .as_u64()
            .ok_or("Profiler frame identity is missing")?;
        if scope != self.profiler_targets.selected {
            return Err("That frame belongs to a different profiling target; the current selection was preserved".into());
        }
        if scope == Target::Editor {
            let exists = if let Some(profile) = &editor.profile_current {
                profile
                    .frames
                    .iter()
                    .any(|candidate| candidate.id == frame && candidate.duration_ns > 0)
            } else {
                editor
                    .host
                    .profiler()
                    .profile()
                    .frames
                    .iter()
                    .any(|candidate| candidate.id == frame && candidate.duration_ns > 0)
            };
            if !exists {
                return Err(
                    "That editor profiler frame is no longer in the current capture".into(),
                );
            }
            editor.action("editor.workspace", json!({"frame":frame}))?;
        } else {
            let capture = self
                .profiler_targets
                .capture()
                .ok_or("No completed frame snapshot exists for this exact player incarnation")?;
            if value["capture"].as_u64() != Some(capture.serial)
                || !capture
                    .profile
                    .frames
                    .iter()
                    .any(|candidate| candidate.id == frame)
            {
                return Err(
                    "That profiler frame was superseded; select a frame in the current snapshot"
                        .into(),
                );
            }
            self.profiler_targets.frame = Some(frame);
            self.profiler_targets.follow = false;
        }
        self.rebuild(editor)
    }
    pub(crate) fn profiler_targets_busy(&self) -> bool {
        self.profiler_targets.pending.is_some() || self.profiler_targets.editor_job.is_some()
    }
    pub(crate) fn profiler_targets_report(&self) -> Value {
        let capture = |capture: &Capture| json!({"target":capture.target.value(),"name":capture.name,"serial":capture.serial,"frames":capture.profile.frames.len(),"frozen":capture.frozen});
        json!({"selected":self.profiler_targets.selected.value(),"transport":self.profiler_targets.transport,
            "pending":self.profiler_targets.pending.as_ref().map(|pending|json!({"ticket":pending.ticket.0,"target":pending.target,"session":pending.session})),
            "editorCaptureJob":self.profiler_targets.editor_job,"current":self.profiler_targets.current.as_ref().map(capture),
            "baseline":self.profiler_targets.baseline.as_ref().map(capture),"error":self.profiler_targets.error,"refreshError":self.profiler_targets.refresh_error})
    }
}
