use super::*;
const CADENCE: Duration = Duration::from_millis(250);
const POLL_BUDGET: usize = 8;
const MAX_PENDING: usize = 64;
const TERMINAL_HISTORY: usize = 8;

pub(super) struct Pending {
    pub session: String,
    pub world: String,
    pub started: Instant,
    pub latest: Option<ScriptRecoveryStatus>,
    pub invalid_reply: bool,
}

/// Only this editor's tickets, never a global player/world completion flag.
#[derive(Default)]
pub(crate) struct State {
    pub(super) pending: BTreeMap<u64, Pending>,
    order: VecDeque<u64>,
    pub(super) terminal: VecDeque<ScriptRecoveryStatus>,
    pub(super) request_error: Option<(String, Vec<ara_diag::Diagnostic>)>,
    next_poll: Option<Instant>,
}

impl State {
    pub(super) fn full(&self) -> bool {
        self.pending.len() >= MAX_PENDING
    }
    pub(super) fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
    pub(super) fn pending_world(&self, world: &str) -> bool {
        self.pending.values().any(|pending| pending.world == world)
    }
    pub(super) fn start(
        &mut self,
        request: ScriptRecoveryRequest,
        submit: impl FnOnce(
            ScriptRecoveryRequest,
        ) -> Result<ScriptRecoveryTicket, Vec<ara_diag::Diagnostic>>,
    ) -> bool {
        if self.full() || self.pending_world(&request.world) {
            return false;
        }
        let world = request.world.clone();
        let session = request.session.clone();
        match submit(request) {
            Ok(ticket) if !self.pending.contains_key(&ticket.0) => {
                self.pending.insert(
                    ticket.0,
                    Pending {
                        session,
                        world,
                        started: Instant::now(),
                        latest: None,
                        invalid_reply: false,
                    },
                );
                self.order.push_back(ticket.0);
                self.next_poll = None;
                self.request_error = None;
            }
            Ok(_) => {
                // Never overwrite or release an earlier subscription on a bad reply.
                self.request_error = Some((
                    world,
                    vec![ara_diag::Diagnostic::error(
                        crate::codes::ACTION,
                        "C# recovery returned a duplicate owned ticket; the earlier request remains pending",
                    )],
                ));
            }
            Err(diagnostics) => {
                self.request_error = Some((world, diagnostics));
            }
        }
        true
    }

    pub(super) fn poll(
        &mut self,
        now: Instant,
        mut read: impl FnMut(ScriptRecoveryTicket) -> Option<ScriptRecoveryStatus>,
    ) -> bool {
        if self.pending.is_empty() || self.next_poll.is_some_and(|next| now < next) {
            return false;
        }
        self.next_poll = Some(now + CADENCE);
        let mut changed = false;
        for _ in 0..self.order.len().min(POLL_BUDGET) {
            let Some(ticket) = self.order.pop_front() else {
                break;
            };
            let Some(pending) = self.pending.get_mut(&ticket) else {
                continue;
            };
            match read(ScriptRecoveryTicket(ticket)) {
                None => self.order.push_back(ticket),
                Some(status) if status.ticket.0 != ticket || status.world != pending.world => {
                    // Preserve ownership until an actual matching terminal reply.
                    changed |= !pending.invalid_reply;
                    pending.invalid_reply = true;
                    self.order.push_back(ticket);
                }
                Some(status) if status.finished => {
                    self.pending.remove(&ticket);
                    self.terminal.push_front(status);
                    self.terminal.truncate(TERMINAL_HISTORY);
                    changed = true;
                }
                Some(status) => {
                    pending.latest = Some(status);
                    pending.invalid_reply = false;
                    self.order.push_back(ticket);
                    changed = true;
                }
            }
        }
        changed
    }

    pub(super) fn note(&self, current_world: Option<&str>) -> Option<String> {
        if let Some((world, diagnostics)) = &self.request_error {
            let detail = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.is_error())
                .or_else(|| diagnostics.first())
                .map(|diagnostic| diagnostic.message.as_str())
                .unwrap_or("The backend did not provide a diagnostic");
            return Some(format!(
                "C# reload request for {world} was not queued: {detail}"
            ));
        }
        let current = self
            .pending
            .iter()
            .find(|(_, pending)| Some(pending.world.as_str()) == current_world);
        if let Some((ticket, pending)) = current.or_else(|| self.pending.iter().next()) {
            return Some(format!(
                "C# reload #{ticket} pending for {}{}",
                pending.world,
                if Some(pending.world.as_str()) == current_world {
                    ""
                } else {
                    " (previous world; still owned)"
                }
            ));
        }
        self.terminal
            .iter()
            .find(|status| Some(status.world.as_str()) == current_world)
            .or_else(|| self.terminal.front())
            .map(summary)
    }

    pub(super) fn report(&self) -> Value {
        json!({"busy":self.busy(),"pollBudget":POLL_BUDGET,"pollMilliseconds":250,"ageResolutionSeconds":1,
            "pending":self.pending.iter().map(|(ticket,pending)|json!({"ticket":ticket,"session":pending.session,"world":pending.world,
                "ageSeconds":pending.started.elapsed().as_secs(),"invalidReply":pending.invalid_reply,"finished":false})).collect::<Vec<_>>(),
            "terminalHistory":self.terminal.iter().map(|status|json!({"ticket":status.ticket.0,"world":status.world,"finished":status.finished})).collect::<Vec<_>>()})
    }
}

pub(super) fn outcome(outcome: &ScriptRecoveryOutcome) -> &'static str {
    match outcome {
        ScriptRecoveryOutcome::Reloaded => "Reloaded",
        ScriptRecoveryOutcome::Unchanged => "Unchanged",
        ScriptRecoveryOutcome::Failed => "Failed",
        ScriptRecoveryOutcome::Refused => "Refused",
    }
}
pub(super) fn summary(status: &ScriptRecoveryStatus) -> String {
    if !status.finished {
        return format!(
            "C# reload #{} pending for {}",
            status.ticket.0, status.world
        );
    }
    if status.players.is_empty() {
        return format!(
            "C# reload #{} finished for {}; no player result was reported",
            status.ticket.0, status.world
        );
    }
    let players = status
        .players
        .iter()
        .take(16)
        .map(|player| format!("player {} {}", player.player, outcome(&player.outcome)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "C# reload #{} for {}: {players}{}",
        status.ticket.0,
        status.world,
        if status.players.len() > 16 {
            " (additional player results in Console)"
        } else {
            ""
        }
    )
}
