use super::*;

pub(in crate::shell) struct Reading {
    pub editor: bool,
    pub current: Option<Arc<ara_profile::Profile>>,
    pub baseline: Option<Arc<ara_profile::Profile>>,
    pub recording: bool,
    pub frozen: bool,
    pub compare: bool,
    pub frame: Option<u64>,
    pub label: String,
    pub current_label: String,
    pub baseline_label: String,
    pub blocker: Option<String>,
    pub compare_blocker: Option<String>,
}

impl Shell {
    /// Clone only Arc handles at a markup rebuild; profiles/frames are never copied here.
    pub(in crate::shell) fn profiler_reading(&self, editor: &mut Editor) -> Reading {
        let state = &self.profiler_targets;
        let is_editor = state.selected == Target::Editor;
        let recording = if is_editor {
            editor.host.profiler().is_recording()
        } else {
            state.transport
                && state.refresh_error.is_none()
                && state
                    .row()
                    .is_some_and(|row| row.available && row.recording)
        };
        let current = if is_editor {
            editor.profile_current.as_ref().map(Arc::clone)
        } else {
            state.capture().map(|capture| Arc::clone(&capture.profile))
        };
        let frozen = if is_editor {
            current.is_some()
        } else {
            state.capture().is_some_and(|capture| capture.frozen)
        };
        let baseline = state
            .baseline
            .as_ref()
            .map(|capture| Arc::clone(&capture.profile))
            .or_else(|| editor.profile_baseline.as_ref().map(Arc::clone));
        let editor_label = format!("Editor · PID {}", std::process::id());
        let current_label = if is_editor {
            format!(
                "{} · {}",
                editor_label,
                if frozen {
                    "frozen capture"
                } else {
                    "live closed frames"
                }
            )
        } else {
            state.capture().map_or_else(
                || format!("{} · no snapshot", state.selected.label()),
                |capture| {
                    format!(
                        "{} · {} · {}",
                        capture.name,
                        capture.target.label(),
                        if capture.frozen {
                            "frozen capture"
                        } else {
                            "snapshot"
                        }
                    )
                },
            )
        };
        let baseline_label = state.baseline.as_ref().map_or_else(
            || {
                if baseline.is_some() {
                    editor_label
                } else {
                    "Not set".into()
                }
            },
            |capture| format!("{} · {}", capture.name, capture.target.label()),
        );
        let label = if is_editor {
            if recording {
                "Recording editor frames"
            } else if frozen {
                "Frozen editor capture"
            } else {
                "Editor closed-frame history"
            }
            .into()
        } else if current.is_none() {
            "No player snapshot received".into()
        } else if recording {
            if frozen {
                "Player recording · viewing frozen capture"
            } else if !state.follow {
                "Player recording · viewing held snapshot"
            } else {
                "Player recording · viewing latest snapshot"
            }
            .into()
        } else if state.blocker().is_some() {
            "Cached player snapshot · target unavailable/pending".into()
        } else if frozen {
            "Frozen player capture".into()
        } else if !state.follow {
            "Held player snapshot".into()
        } else {
            "Player snapshot".into()
        };
        Reading {
            editor: is_editor,
            current,
            baseline,
            recording,
            frozen,
            compare: if is_editor && state.baseline.is_none() {
                editor.state.profiler_compare
            } else {
                state.compare
            },
            frame: if is_editor {
                editor.state.profiler_frame
            } else {
                state.frame
            },
            label,
            current_label,
            baseline_label,
            blocker: state.blocker().map(str::to_owned),
            compare_blocker: self.profiler_compare_blocker(editor).map(str::to_owned),
        }
    }

    pub(in crate::shell) fn profiler_target_toolbar(
        &mut self,
        reading: &Reading,
        out: &mut String,
    ) {
        let state = &self.profiler_targets;
        let mut options = vec![(Value::Null, "Editor".to_owned())];
        for row in &state.rows {
            options.push((
                json!(row.target),
                format!(
                    "{} · {}{}",
                    row.name,
                    row.target.label(),
                    if row.available { "" } else { " · unavailable" }
                ),
            ));
        }
        if let Target::Player(scope) = &state.selected
            && !state.rows.iter().any(|row| &row.target == scope)
        {
            options.push((json!(scope), format!("{} · unavailable", scope.label())));
        }
        let current = state.selected.value();
        let pending = state
            .pending
            .as_ref()
            .map(|pending| {
                format!(
                    "Pending profiling request {} · {}",
                    pending.ticket.0,
                    pending.target.label()
                )
            })
            .or_else(|| {
                state
                    .editor_job
                    .as_ref()
                    .map(|job| format!("Editor capture save pending · {job}"))
            });
        let error = state.error.clone();
        let refresh_error = state.refresh_error.clone();
        let transport = state.transport;
        let empty_targets = state.rows.is_empty();
        let process = state.row().map(|row| {
            row.process_id
                .map_or("Process ID not reported".into(), |pid| {
                    format!("Process PID {pid}")
                })
        });
        let diagnostics: Vec<_> = state
            .diagnostics
            .iter()
            .take(8)
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect();
        out.push_str("<panel id='profiler-targets' class='profiler-targets'><panel id='profiler-target-row' class='profiler-target-row'>");
        self.choice_button(
            "profile-target",
            "Profiler target",
            choices::Select {
                destination: choices::Destination::ProfilerTarget,
                options,
                current,
            },
            out,
        );
        for (id, command, label) in [
            (
                "profile-target-refresh",
                "profiler.refresh",
                "Refresh targets",
            ),
            (
                "profile-snapshot",
                "profiler.snapshot",
                if reading.editor {
                    "Freeze snapshot"
                } else {
                    "Follow target"
                },
            ),
        ] {
            self.bind_command(id, command, commands::Target::None);
            let disabled = command == "profiler.snapshot" && reading.blocker.is_some();
            out.push_str(&format!("<button id='{id}' class='ghost' style='height:auto;min-height:26px;max-width:100%;white-space:normal' text='{label}' disabled='{disabled}' description='{}' action='Dispatch' />",escape(if disabled{reading.blocker.as_deref().unwrap_or("Unavailable")}else if command=="profiler.snapshot"&&!reading.editor{"Read closed frames and follow new snapshots while this exact player records"}else{"Read current profiler target metadata or closed frames; no project mutation"})));
        }
        out.push_str(&format!("</panel><text id='profile-current-scope' class='profiler-target-caption' text='Current: {}' /><text id='profile-baseline-scope' class='profiler-target-caption' text='Baseline: {}' />",escape(&reading.current_label),escape(&reading.baseline_label)));
        if let Some(process) = process {
            out.push_str(&format!(
                "<text id='profile-target-process' class='profiler-target-caption' text='{}' />",
                escape(&process)
            ));
        }
        if !transport {
            out.push_str("<text id='profile-player-transport' class='profiler-target-caption' text='Player targets unavailable: profiling transport is not attached. Editor recording remains available.' />");
        } else if empty_targets {
            out.push_str("<text id='profile-player-targets-empty' class='profiler-target-caption' text='No live player profiling targets have been reported.' />");
        }
        if let Some(reason) = &reading.blocker {
            out.push_str(&format!("<text id='profile-target-unavailable' class='profiler-target-caption' text='{}' />",escape(reason)));
        }
        if let Some(pending) = pending {
            out.push_str(&format!(
                "<text id='profile-target-pending' class='profiler-target-caption' text='{}' />",
                escape(&pending)
            ));
        }
        if let Some(error) = error {
            out.push_str(&format!(
                "<text id='profile-target-error' class='profiler-target-caption' text='{}' />",
                escape(&error)
            ));
        }
        if let Some(error) = refresh_error {
            out.push_str(&format!("<text id='profile-target-refresh-error' class='profiler-target-caption' text='Last target list retained: {}' />",escape(&error)));
        }
        for (index, diagnostic) in diagnostics.iter().enumerate() {
            out.push_str(&format!("<text id='profile-target-diagnostic-{index}' class='profiler-target-caption' text='{}' />",escape(diagnostic)));
        }
        out.push_str("</panel>");
    }
}
