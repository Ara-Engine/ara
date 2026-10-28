use super::*;
use ara_runtime_render::probes::{ProbeStage, ProbeState};

fn state_name(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Idle => "Idle",
        ProbeState::Queued => "Queued",
        ProbeState::Capturing => "Capturing",
        ProbeState::Progress => "In progress",
        ProbeState::Done => "Published",
        ProbeState::Cancelled => "Cancelled",
        ProbeState::Failed => "Failed",
    }
}
fn may_cancel(job: &Job, row: &ProbeRow, available: bool) -> bool {
    available
        && job.cancellable()
        && (row.cancellable
            || matches!(
                row.state,
                ProbeState::Idle | ProbeState::Done | ProbeState::Cancelled | ProbeState::Failed
            ))
}

impl Shell {
    pub(in crate::shell) fn probe_section(&mut self, editor: &Editor, out: &mut String) {
        // Command bindings need mutable Shell while rows stay borrowed, not cloned.
        let state = std::mem::take(&mut self.probe_ui);
        self.probe_section_view(editor, &state, out);
        self.probe_ui = state;
    }
    fn probe_section_view(&mut self, editor: &Editor, state: &State, out: &mut String) {
        self.bind_command(
            "lighting-probe-refresh",
            "lighting.probe.refresh",
            commands::Target::None,
        );
        self.bind_command(
            "lighting-probe-recapture",
            "lighting.probe.recapture",
            commands::Target::None,
        );
        let selected = state.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .probes
                .iter()
                .any(|row| editor.state.selection.contains(&row.entity))
        });
        let reason = state.reason().unwrap_or(if selected {
            "Recapture selected Reflection Probes without changing authored components"
        } else {
            "Select a Reflection Probe reported by the current Scene renderer"
        });
        out.push_str(&format!("<panel id='lighting-probes' class='lighting-card probe-section'><text id='lighting-probes-title' class='lighting-title' text='Reflection probes' /><text id='lighting-probes-contract' class='lighting-help' text='Capture is derived renderer work. Component edits use normal Undo; a capture publishes only after all faces and filtering finish.' /><panel id='lighting-probes-actions' class='lighting-toolbar'><button id='lighting-probe-recapture' class='lighting-button' text='Recapture selected probes' disabled='{}' description='{}' action='Dispatch' /><button id='lighting-probe-refresh' class='lighting-button' text='Refresh state' description='Read the current Scene renderer and this editor session’s capture jobs' action='Dispatch' /></panel>",state.reason().is_some()||!selected,escape(reason)));
        if let Some(reason) = state.reason() {
            out.push_str(&format!("<text id='lighting-probes-unavailable' class='lighting-help' text='Capture unavailable: {}' />",escape(reason)));
        }
        if !state.available && state.snapshot.is_some() {
            out.push_str("<text id='lighting-probes-last-known' class='lighting-help' text='Last known rows are retained below. The current Scene snapshot is unavailable; these rows do not establish current readiness.' />");
        }
        let rows = state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.probes.as_slice())
            .unwrap_or(&[]);
        if rows.is_empty() {
            out.push_str("<text id='lighting-probes-empty' class='lighting-help' text='No Reflection Probe rows have been reported for this Scene. Create a Reflection Probe above, then inspect its bounds and capture settings.' />");
        }
        let ordered = rows
            .iter()
            .filter(|row| editor.state.selection.contains(&row.entity))
            .chain(
                rows.iter()
                    .filter(|row| !editor.state.selection.contains(&row.entity)),
            );
        for row in ordered.take(64) {
            let id = format!(
                "lighting-probe-{:016x}",
                ara_ui::source::stable_id(&format!("{}#{}", row.doc, row.entity))
            );
            let inspect = format!("{id}-inspect");
            let recapture = format!("{id}-recapture");
            let copy = format!("{id}-copy");
            self.bind_command(
                &inspect,
                "lighting.inspect",
                commands::Target::Entity(row.entity.clone()),
            );
            self.bind_command(
                &recapture,
                "lighting.probe.recapture",
                commands::Target::Entity(row.entity.clone()),
            );
            self.bind_command(
                &copy,
                "lighting.probe.copy",
                commands::Target::Entity(row.entity.clone()),
            );
            let work = match (row.completed_work, row.total_work) {
                (Some(done), Some(total)) => format!("{done} of {total} work units"),
                _ => "Work total unknown".into(),
            };
            let stage = match row.stage {
                Some(ProbeStage::Faces) => "faces",
                Some(ProbeStage::Filter) => "filtering",
                Some(ProbeStage::Publish) => "publishing",
                None => "no active stage",
            };
            let progress = row
                .progress
                .filter(|p| p.is_finite())
                .map_or(String::new(), |p| format!(" · {:.0}%", p * 100.));
            out.push_str(&format!("<panel id='{id}' class='probe-entry'><text id='{id}-entity' class='lighting-title' text='{}' /><text id='{id}-state' class='lighting-help' text='{} · {stage} · {work}{progress}' /><text id='{id}-freshness' class='lighting-help' text='Generation {} · {}{}{}' />",escape(&row.entity),state_name(row.state),row.generation,
                if row.stale {"Source changed"} else if row.source_hash.is_empty() {"Capture inputs have not been hashed yet"} else {"Renderer reports current inputs"},
                if row.retained_previous {" · previous valid capture retained"} else {""},
                if !row.enabled {" · disabled contribution"} else {""}));
            if let Some(receipt) = &row.last_receipt {
                out.push_str(&format!("<text id='{id}-receipt' class='lighting-help' text='Last receipt: {} px × {} faces · generation {} · renderer revision {} · {}' /><text id='{id}-cache' class='probe-identity' text='{}' /><text id='{id}-hash' class='probe-identity' text='Capture {} · source {}' />",receipt.resolution,receipt.faces,receipt.generation,receipt.source_revision,if receipt.published {"published"}else{"not published"},escape(&receipt.cache_path),escape(&receipt.capture_hash),escape(&receipt.source_hash)));
            } else {
                out.push_str(&format!("<text id='{id}-no-receipt' class='lighting-help' text='No published capture receipt has been reported.' />"));
            }
            for (index, diagnostic) in row.diagnostics.iter().take(8).enumerate() {
                out.push_str(&format!(
                    "<text id='{id}-diagnostic-{index}' class='lighting-help' text='{}: {}' />",
                    diagnostic.code,
                    escape(&diagnostic.message)
                ));
            }
            out.push_str(&format!("<panel id='{id}-actions' class='lighting-toolbar'><button id='{inspect}' class='lighting-button' text='Inspect' action='Dispatch' /><button id='{recapture}' class='lighting-button' text='Recapture' disabled='{}' description='{}' action='Dispatch' /><button id='{copy}' class='lighting-button' text='Copy state and receipt' action='Dispatch' />",state.reason().is_some(),escape(state.reason().unwrap_or("Request a real derived capture for this probe"))));
            if let Some(job) = state
                .job_order
                .iter()
                .rev()
                .filter_map(|id| state.jobs.get(id))
                .find(|job| {
                    job.doc == row.doc
                        && job.entities.contains(&row.entity)
                        && may_cancel(job, row, state.available)
                })
            {
                let cancel = format!("{id}-cancel");
                self.bind_command(
                    &cancel,
                    "lighting.probe.cancel",
                    commands::Target::Entity(row.entity.clone()),
                );
                out.push_str(&format!("<button id='{cancel}' class='lighting-button' text='Cancel own requests' disabled='{}' description='Cancel all cancellable requests from this editor session for this probe; other sessions keep their own subscriptions' action='Dispatch' />",!may_cancel(job,row,state.available)));
            }
            out.push_str("</panel></panel>");
            if matches!(
                row.state,
                ProbeState::Queued | ProbeState::Capturing | ProbeState::Progress
            ) && state.jobs.values().any(|job| {
                job.doc == row.doc
                    && job.entities.contains(&row.entity)
                    && job
                        .latest
                        .as_ref()
                        .is_some_and(|status| status["status"] == "cancelled")
            }) {
                out.push_str(&format!("<text id='{id}-retirement' class='lighting-help' text='Renderer still reports active work. Cancelled Host requests do not establish renderer retirement.' />"));
            }
        }
        if rows.len() > 64 {
            out.push_str("<text id='lighting-probes-bounded' class='lighting-help' text='Showing 64 renderer rows, selected probes first. Use the Scene lighting inventory to select any other probe and read its state here.' />");
        }
        for (index, id) in state.job_order.iter().rev().enumerate() {
            let Some(job) = state.jobs.get(id).filter(|job| job.doc == editor.state.doc) else {
                continue;
            };
            let text = job.latest.as_ref().map_or(
                "Queued Host request; waiting for status".into(),
                |status| {
                    format!(
                        "{} · {} · {}",
                        id,
                        status["status"].as_str().unwrap_or("unknown"),
                        status["message"].as_str().unwrap_or("No progress message")
                    )
                },
            );
            out.push_str(&format!(
                "<text id='lighting-probe-job-{index}' class='lighting-help' text='{}' />",
                escape(&text)
            ));
            if let Some(error) = &job.error {
                out.push_str(&format!("<text id='lighting-probe-job-{index}-status-error' class='lighting-help' text='{}' />",escape(error)));
            }
            if let Some(diagnostics) = job
                .latest
                .as_ref()
                .and_then(|status| status.get("diagnostics"))
                .and_then(Value::as_array)
            {
                for (number, diagnostic) in diagnostics.iter().take(4).enumerate() {
                    out.push_str(&format!("<text id='lighting-probe-job-{index}-error-{number}' class='lighting-help' text='{}' />",escape(diagnostic["message"].as_str().unwrap_or("Capture job reported a diagnostic"))));
                }
            }
        }
        out.push_str("</panel>");
    }
}
