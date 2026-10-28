use super::*;
use std::sync::atomic::Ordering;

/// Native editor's owned bounded mailbox. All methods are nonblocking.
pub struct ScriptRecoveryHandle {
    tx: SyncSender<Message>,
    reply: SyncSender<Update>,
    rx: Receiver<Update>,
    sequence: Arc<AtomicU64>,
    live: Arc<AtomicBool>,
    root: PathBuf,
    roster: Vec<Target>,
    statuses: BTreeMap<ScriptRecoveryTicket, ScriptRecoveryStatus>,
    acknowledged: std::collections::BTreeSet<ScriptRecoveryTicket>,
}

pub(super) fn create(
    tx: SyncSender<Message>,
    sequence: Arc<AtomicU64>,
    live: Arc<AtomicBool>,
    root: PathBuf,
) -> ScriptRecoveryHandle {
    let (reply, rx) = mpsc::sync_channel(HISTORY * 2);
    let _ = tx.try_send(Message::Subscribe(reply.clone()));
    ScriptRecoveryHandle {
        tx,
        reply,
        rx,
        sequence,
        live,
        root,
        roster: Vec::new(),
        statuses: BTreeMap::new(),
        acknowledged: std::collections::BTreeSet::new(),
    }
}

impl ScriptRecoveryHandle {
    fn drain(&mut self) {
        for _ in 0..HISTORY * 2 {
            let Ok(update) = self.rx.try_recv() else {
                break;
            };
            match update {
                Update::Roster(roster) => self.roster = roster,
                Update::Status(status) => {
                    self.acknowledged.insert(status.ticket);
                    self.statuses.insert(status.ticket, status);
                }
            }
        }
        while self.statuses.len() > HISTORY {
            let Some(ticket) = self
                .statuses
                .iter()
                .find(|(_, status)| status.finished)
                .map(|(ticket, _)| *ticket)
            else {
                break;
            };
            self.statuses.remove(&ticket);
            self.acknowledged.remove(&ticket);
        }
        if !self.live.load(Ordering::Acquire) {
            self.roster.clear();
            for status in self.statuses.values_mut().filter(|status| !status.finished) {
                // With no owning Host left, publish only new local rows through its bus.
                // Never replay an acknowledged batch of scoped script diagnostics.
                if !self.acknowledged.contains(&status.ticket) {
                    for diagnostic in &status.diagnostics {
                        ara_diag::bus::emit_with(
                            diagnostic.clone(),
                            ara_diag::bus::Meta::default(),
                        );
                    }
                }
                status.finished = true;
                let diagnostic = problem(
                    &self.root,
                    "Script recovery refused: the owning Play service stopped",
                );
                ara_diag::bus::emit_with(diagnostic.clone(), ara_diag::bus::Meta::default());
                status.diagnostics.push(diagnostic);
                for result in &mut status.players {
                    result.outcome = ScriptRecoveryOutcome::Refused;
                }
            }
        }
    }

    /// Queues current-incarnation recovery; repeated pending selections coalesce.
    pub fn request(
        &mut self,
        request: ScriptRecoveryRequest,
    ) -> Result<ScriptRecoveryTicket, Vec<Diagnostic>> {
        self.drain();
        if request.session.is_empty()
            || request.session.len() > 8192
            || request.world.len() > 8192
            || request.why.as_ref().is_some_and(|why| why.len() > 8192)
        {
            return Err(vec![problem(
                &self.root,
                "Script recovery requires a session and bounded request strings",
            )]);
        }
        let targets: Vec<_> = self
            .roster
            .iter()
            .filter(|target| {
                target.world == request.world
                    && request.player.is_none_or(|player| player == target.player)
            })
            .cloned()
            .collect();
        if let Some((ticket, _)) = self.statuses.iter().find(|(_, status)| {
            !status.finished
                && status.world == request.world
                && status.players.len() == targets.len()
                && status.players.iter().zip(&targets).all(|(r, t)| {
                    r.player == t.player && r.source == t.source && r.binding == t.binding
                })
        }) {
            return Ok(*ticket);
        }
        if self
            .statuses
            .values()
            .filter(|status| !status.finished)
            .count()
            >= LIMIT
        {
            return Err(vec![problem(
                &self.root,
                "Too many pending script recovery requests",
            )]);
        }
        let ticket = ScriptRecoveryTicket(
            self.sequence
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                    (id < i64::MAX as u64).then_some(id + 1)
                })
                .map_err(|_| {
                    vec![problem(
                        &self.root,
                        "Script recovery ticket sequence exhausted",
                    )]
                })?,
        );
        let mut status = ScriptRecoveryStatus {
            ticket,
            world: request.world,
            finished: false,
            players: targets
                .iter()
                .map(|target| ScriptRecoveryPlayerResult {
                    player: target.player,
                    source: target.source.clone(),
                    binding: target.binding,
                    outcome: ScriptRecoveryOutcome::Refused,
                    modules: Vec::new(),
                    diagnostics: Vec::new(),
                })
                .collect(),
            diagnostics: Vec::new(),
        };
        if targets.is_empty() {
            status.diagnostics.push(problem(&self.root, "Script recovery refused: the selected world/player is stopped, unknown, or has not published its incarnation yet"));
        }
        self.tx
            .try_send(Message::Request {
                status: status.clone(),
                targets,
                reply: self.reply.clone(),
            })
            .map_err(|_| {
                vec![problem(
                    &self.root,
                    "Script recovery control queue is full or disconnected",
                )]
            })?;
        self.statuses.insert(ticket, status);
        self.drain();
        Ok(ticket)
    }

    /// Latest owned snapshot; `None` means unknown or explicitly retired history.
    pub fn poll(&mut self, ticket: ScriptRecoveryTicket) -> Option<ScriptRecoveryStatus> {
        self.drain();
        self.statuses.get(&ticket).cloned()
    }
}
