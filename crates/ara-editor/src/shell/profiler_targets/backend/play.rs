//! The real Play mailbox behind the private UI port; no RPC, file or codec work.
use super::{Action, Backend, Outcome, Request, Row, Scope, Status, Ticket};
use ara_diag::Diagnostic;
use ara_play_host::{
    PlayProfileAction, PlayProfileOutcome, PlayProfileRequest, PlayProfileStatus,
    PlayProfileTarget, PlayProfileTargetRow, PlayProfileTicket, PlayProfilingHandle,
};

/// Attach the owned mailbox before its PlayService moves into the project Host.
pub(crate) fn attach(handle: PlayProfilingHandle) -> Box<dyn Backend> {
    Box::new(Play { handle, rows: Vec::new() })
}

struct Play {
    handle: PlayProfilingHandle,
    rows: Vec<Row>,
}

impl Backend for Play {
    fn refresh_targets(&mut self) -> Result<(), Vec<Diagnostic>> {
        self.handle.refresh_targets()?;
        let current = self.handle.targets();
        // Shell polls at 4 Hz; an unchanged owned inventory needs no new strings.
        if current.len() != self.rows.len()
            || self.rows.iter().zip(current).any(|(cached, live)| !same_row(cached, live))
        {
            self.rows = current.iter().map(row).collect();
        }
        Ok(())
    }

    fn targets(&self) -> &[Row] {
        &self.rows
    }

    fn request(&mut self, request: Request) -> Result<Ticket, Vec<Diagnostic>> {
        self.handle.request(request_to_play(request)).map(|ticket| Ticket(ticket.0))
    }

    fn poll(&mut self, ticket: Ticket) -> Option<Status> {
        // Preserve the returned token and scope for State::accept to verify;
        // never replace them with the selected target or the requested token.
        self.handle.poll(PlayProfileTicket(ticket.0)).map(status)
    }
}

fn same_row(cached: &Row, live: &PlayProfileTargetRow) -> bool {
    cached.target.world == live.target.world
        && cached.target.player == live.target.player
        && cached.target.source == live.target.source
        && cached.target.binding == live.target.binding
        && cached.name == live.name
        && cached.process_id == Some(live.process_id)
        && cached.recording == live.recording
        && cached.available == live.available
        && cached.reason == live.reason
}

fn row(value: &PlayProfileTargetRow) -> Row {
    Row {
        target: Scope {
            world: value.target.world.clone(),
            player: value.target.player,
            source: value.target.source.clone(),
            binding: value.target.binding,
        },
        name: value.name.clone(),
        process_id: Some(value.process_id),
        recording: value.recording,
        available: value.available,
        reason: value.reason.clone(),
    }
}

fn scope(value: PlayProfileTarget) -> Scope {
    Scope { world: value.world, player: value.player, source: value.source, binding: value.binding }
}

fn request_to_play(value: Request) -> PlayProfileRequest {
    PlayProfileRequest {
        session: value.session,
        target: PlayProfileTarget {
            world: value.target.world,
            player: value.target.player,
            source: value.target.source,
            binding: value.target.binding,
        },
        action: match value.action {
            Action::Start => PlayProfileAction::Start,
            Action::Stop => PlayProfileAction::Stop,
            Action::Snapshot => PlayProfileAction::Snapshot,
            Action::Freeze => PlayProfileAction::Freeze,
        },
        why: value.why,
    }
}

fn status(value: PlayProfileStatus) -> Status {
    Status {
        ticket: Ticket(value.ticket.0),
        target: scope(value.target),
        finished: value.finished,
        outcome: value.outcome.map(|value| match value {
            PlayProfileOutcome::Completed => Outcome::Completed,
            PlayProfileOutcome::Unchanged => Outcome::Unchanged,
            PlayProfileOutcome::Failed => Outcome::Failed,
            PlayProfileOutcome::Refused => Outcome::Refused,
        }),
        recording: value.recording,
        // The backend validates schema/capacity and decodes on its worker. Move
        // its <=120 closed-frame capture and already-published diagnostics once.
        profile: value.profile,
        diagnostics: value.diagnostics,
    }
}

#[cfg(test)]
#[path = "play_tests.rs"]
mod tests;
