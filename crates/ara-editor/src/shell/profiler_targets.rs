//! Target identity and immutable player captures over a small private asynchronous port.
use super::*;
pub(crate) use crate::play_profiling as backend;
mod controls;
#[cfg(test)]
mod tests;
mod view;
use backend::{Action, Backend, Outcome, Request, Row, Scope, Status, Ticket};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
const POLL: Duration = Duration::from_millis(250);
const LIVE_SNAPSHOT: Duration = Duration::from_secs(1);
pub(super) const CSS: &str = ".profiler-targets { flex-shrink:0;min-width:0px;gap:4px;padding:6px; }.profiler-target-row { flex-direction:row;flex-wrap:wrap;min-width:0px;gap:6px; }.profiler-target-caption { white-space:normal;min-width:0px;flex-shrink:0;font-size:11px;color:var(--muted-foreground); }.profiler-actions { flex-wrap:wrap;height:auto;min-height:28px;flex-shrink:0;min-width:0px; }";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Target {
    #[default]
    Editor,
    Player(Scope),
}
impl Target {
    fn value(&self) -> Value {
        match self {
            Self::Editor => Value::Null,
            Self::Player(scope) => json!(scope),
        }
    }
    fn label(&self) -> String {
        match self {
            Self::Editor => "Editor".into(),
            Self::Player(scope) => scope.label(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Record,
    Snapshot,
    Freeze,
    Baseline,
}
struct Pending {
    ticket: Ticket,
    target: Scope,
    purpose: Purpose,
    session: String,
}
struct Capture {
    target: Target,
    name: String,
    profile: Arc<ara_profile::Profile>,
    serial: u64,
    frozen: bool,
}

/// Local reading state; target changes never retarget a pending ticket or capture.
pub(crate) struct State {
    selected: Target,
    rows: Vec<Row>,
    error: Option<String>,
    refresh_error: Option<String>,
    diagnostics: Vec<ara_diag::Diagnostic>,
    pending: Option<Pending>,
    current: Option<Capture>,
    baseline: Option<Capture>,
    frame: Option<u64>,
    compare: bool,
    follow: bool,
    serial: u64,
    next_poll: Option<Instant>,
    next_snapshot: Option<Instant>,
    transport: bool,
    editor_job: Option<String>,
    next_editor_poll: Option<Instant>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            selected: Target::Editor,
            rows: Vec::new(),
            error: None,
            refresh_error: None,
            diagnostics: Vec::new(),
            pending: None,
            current: None,
            baseline: None,
            frame: None,
            compare: false,
            follow: true,
            serial: 0,
            next_poll: None,
            next_snapshot: None,
            transport: false,
            editor_job: None,
            next_editor_poll: None,
        }
    }
}
impl State {
    fn row(&self) -> Option<&Row> {
        let Target::Player(scope) = &self.selected else {
            return None;
        };
        self.rows.iter().find(|row| &row.target == scope)
    }
    fn blocker(&self) -> Option<&str> {
        if self.selected == Target::Editor {
            return None;
        }
        if self.pending.is_some() {
            return Some("Wait for the pending player profiling request to finish");
        }
        if !self.transport {
            return Some("The player profiling transport is not attached");
        }
        if let Some(error) = self.refresh_error.as_deref() {
            return Some(error);
        }
        if let Target::Player(scope) = &self.selected
            && (scope.world.trim().is_empty() || scope.source.trim().is_empty())
        {
            return Some("The profiling target has no valid world/source incarnation identity");
        }
        match self.row() {
            Some(row) if row.available => None,
            Some(row) => row
                .reason
                .as_deref()
                .or(Some("This target does not provide profiling")),
            None => {
                Some("This exact player incarnation is no longer live; select a current target")
            }
        }
    }
    fn capture(&self) -> Option<&Capture> {
        self.current
            .as_ref()
            .filter(|capture| capture.target == self.selected)
    }
    fn recording(&self) -> bool {
        self.row().is_some_and(|row| row.recording)
    }
    fn observe_rows(&mut self, rows: &[Row]) -> bool {
        if self.rows == rows {
            return false;
        }
        self.rows = rows.to_vec();
        true
    }
    fn accept(&mut self, status: Status) -> bool {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        if status.ticket != pending.ticket {
            self.error=Some("A profiling response had another request's ticket; the original request remains pending".into());
            return true;
        }
        if !status.finished {
            return false;
        }
        let pending = self
            .pending
            .take()
            .expect("matching finished profiling ticket");
        self.diagnostics = status.diagnostics.into_iter().take(16).collect();
        if status.target != pending.target {
            self.error=Some("A profiling response did not match its requested world/player incarnation; data was not adopted".into());
            return true;
        }
        if let Some(row) = self.rows.iter_mut().find(|row| row.target == status.target) {
            row.recording = status.recording;
        }
        if !matches!(
            status.outcome,
            Some(Outcome::Completed | Outcome::Unchanged)
        ) {
            self.follow = false;
            self.error = Some(
                match status.outcome {
                    Some(Outcome::Refused) => "The player refused this profiling request",
                    Some(Outcome::Failed) => "The player profiling request failed",
                    _ => "The profiling request finished without an outcome",
                }
                .into(),
            );
            return true;
        }
        self.error = None;
        if let Some(mut profile) = status.profile {
            profile.frames.retain(|frame| frame.duration_ns > 0);
            let Some(serial) = self.serial.checked_add(1) else {
                self.error =
                    Some("Profiler capture identity exhausted; restart the editor session".into());
                return true;
            };
            self.serial = serial;
            let name = self
                .rows
                .iter()
                .find(|row| row.target == pending.target)
                .map_or_else(|| pending.target.label(), |row| row.name.clone());
            let capture = Capture {
                target: Target::Player(pending.target),
                name,
                profile: Arc::new(profile),
                serial,
                frozen: matches!(pending.purpose, Purpose::Freeze | Purpose::Baseline),
            };
            if pending.purpose == Purpose::Baseline {
                self.baseline = Some(capture);
            } else {
                self.current = Some(capture);
                self.frame = None;
            }
            self.compare = false;
        } else if matches!(
            pending.purpose,
            Purpose::Freeze | Purpose::Baseline | Purpose::Snapshot
        ) {
            let target = Target::Player(pending.target);
            let existing = self
                .current
                .as_mut()
                .filter(|capture| capture.target == target);
            if let (Some(Outcome::Unchanged), Some(existing)) = (status.outcome, existing) {
                if pending.purpose == Purpose::Baseline {
                    self.baseline = Some(Capture {
                        target: existing.target.clone(),
                        name: existing.name.clone(),
                        profile: Arc::clone(&existing.profile),
                        serial: existing.serial,
                        frozen: true,
                    });
                } else if pending.purpose == Purpose::Freeze {
                    existing.frozen = true;
                }
            } else {
                self.follow = false;
                self.error = Some(
                    "The profiling response contained no completed-frame capture for this target"
                        .into(),
                );
            }
        }
        true
    }
}

impl Shell {
    /// Poll the private port, not the network. Backend work remains single-flight.
    /// Root takes/restores its Editor handle around this call to keep borrows disjoint.
    pub(crate) fn poll_profiler_targets(
        &mut self,
        session: &str,
        backend: Option<&mut dyn Backend>,
    ) -> ara_ui::Result<bool> {
        let state = &mut self.profiler_targets;
        let Some(backend) = backend else {
            let changed = state.transport;
            state.transport = false;
            return Ok(changed);
        };
        let mut changed = !state.transport;
        state.transport = true;
        let now = Instant::now();
        if state.next_poll.is_some_and(|next| now < next) {
            return Ok(changed);
        }
        state.next_poll = Some(now + POLL);
        match backend.refresh_targets() {
            Ok(()) => {
                changed |= state.refresh_error.is_some();
                state.refresh_error = None;
                changed |= state.observe_rows(backend.targets());
            }
            Err(diagnostics) => {
                let message = diagnostics
                    .first()
                    .map_or("Player profiling targets could not be refreshed", |d| {
                        d.message.as_str()
                    })
                    .to_owned();
                changed |= state.refresh_error.as_ref() != Some(&message);
                state.refresh_error = Some(message);
                state.diagnostics = diagnostics.into_iter().take(16).collect();
            }
        }
        if let Some(pending) = &state.pending
            && let Some(status) = backend.poll(pending.ticket)
        {
            changed |= state.accept(status);
        }
        if state.pending.is_none()
            && state.follow
            && (state.recording() || state.capture().is_none())
            && state.blocker().is_none()
            && state.next_snapshot.is_none_or(|next| now >= next)
        {
            let Target::Player(target) = &state.selected else {
                return Ok(changed);
            };
            let target = target.clone();
            match backend.request(Request {
                session: session.into(),
                target: target.clone(),
                action: Action::Snapshot,
                why: Some("Read selected player's closed profiler frames".into()),
            }) {
                Ok(ticket) => {
                    state.pending = Some(Pending {
                        ticket,
                        target,
                        purpose: Purpose::Snapshot,
                        session: session.into(),
                    });
                    state.next_snapshot = Some(now + LIVE_SNAPSHOT);
                    changed = true;
                }
                Err(diagnostics) => {
                    state.follow = false;
                    state.error = Some(
                        diagnostics
                            .first()
                            .map_or("Player profiling snapshot was refused", |d| {
                                d.message.as_str()
                            })
                            .into(),
                    );
                    state.diagnostics = diagnostics.into_iter().take(16).collect();
                    changed = true;
                }
            }
        }
        Ok(changed)
    }
}
