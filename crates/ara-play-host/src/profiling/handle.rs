use super::*;
use std::{collections::BTreeMap, sync::atomic::Ordering};

/// Native editor's owned nonblocking control/query mailbox.
pub struct PlayProfilingHandle {
    tx: SyncSender<Message>,
    reply: SyncSender<Update>,
    rx: Receiver<Update>,
    sequence: Arc<AtomicU64>,
    live: Arc<AtomicBool>,
    root: PathBuf,
    targets: Vec<PlayProfileTargetRow>,
    statuses: BTreeMap<PlayProfileTicket, (PlayProfileAction, PlayProfileStatus)>,
    refresh_pending: bool,
}

pub(super) fn create(controller: &Controller, root: &Path) -> PlayProfilingHandle {
    let (reply, rx) = mpsc::sync_channel(HISTORY * 2);
    let _ = controller.tx.try_send(Message::Subscribe(reply.clone()));
    PlayProfilingHandle {
        tx: controller.tx.clone(),
        reply,
        rx,
        sequence: Arc::clone(&controller.sequence),
        live: Arc::clone(&controller.live),
        root: root.to_path_buf(),
        targets: Vec::new(),
        statuses: BTreeMap::new(),
        refresh_pending: true,
    }
}

impl PlayProfilingHandle {
    fn drain(&mut self) {
        for _ in 0..HISTORY * 2 {
            let Ok(update) = self.rx.try_recv() else {
                break;
            };
            match update {
                Update::Targets(targets) => {
                    self.targets = targets;
                    self.refresh_pending = false;
                }
                Update::Status(status) => {
                    if let Some((_, previous)) = self.statuses.get_mut(&status.ticket) {
                        *previous = status;
                    }
                }
            }
        }
        while self.statuses.len() > HISTORY {
            let Some(ticket) = self
                .statuses
                .iter()
                .find(|(_, (_, status))| status.finished)
                .map(|(ticket, _)| *ticket)
            else {
                break;
            };
            self.statuses.remove(&ticket);
        }
        if !self.live.load(Ordering::Acquire) {
            for row in &mut self.targets {
                row.available = false;
                row.recording = false;
                row.reason = Some("The owning Play service stopped".into());
            }
            for (_, status) in self
                .statuses
                .values_mut()
                .filter(|(_, status)| !status.finished)
            {
                let diagnostic = problem(
                    &self.root,
                    "Profiling refused: the owning Play service stopped",
                );
                ara_diag::bus::emit_with(diagnostic.clone(), ara_diag::bus::Meta::default());
                status.diagnostics.push(diagnostic);
                status.finished = true;
                status.outcome = Some(PlayProfileOutcome::Refused);
            }
        }
    }

    /// Refreshes the owned target cache asynchronously; repeated refreshes coalesce.
    pub fn refresh_targets(&mut self) -> Result<(), Vec<Diagnostic>> {
        self.drain();
        if !self.refresh_pending {
            self.tx
                .try_send(Message::Refresh(self.reply.clone()))
                .map_err(|_| {
                    vec![problem(
                        &self.root,
                        "Profiling target queue is full or disconnected",
                    )]
                })?;
            self.refresh_pending = true;
        }
        Ok(())
    }

    /// Last published owned inventory, including unavailable exited incarnations.
    pub fn targets(&self) -> &[PlayProfileTargetRow] {
        &self.targets
    }

    /// Queues an exact-incarnation action. No player, compilation or capture work runs here.
    pub fn request(
        &mut self,
        request: PlayProfileRequest,
    ) -> Result<PlayProfileTicket, Vec<Diagnostic>> {
        self.drain();
        if request.session.is_empty()
            || request.session.len() > 8192
            || request.target.world.len() > 8192
            || request.target.source.len() > 8192
            || request.why.as_ref().is_some_and(|why| why.len() > 8192)
        {
            return Err(vec![problem(
                &self.root,
                "Profiling requires a session and bounded request strings",
            )]);
        }
        if let Some((ticket, (action, _))) = self
            .statuses
            .iter()
            .rev()
            .find(|(_, (_, status))| !status.finished && status.target == request.target)
            && *action == request.action
        {
            return Ok(*ticket);
        }
        if self
            .statuses
            .values()
            .filter(|(_, status)| !status.finished)
            .count()
            >= LIMIT
        {
            return Err(vec![problem(
                &self.root,
                "Too many pending profiling requests",
            )]);
        }
        let ticket = PlayProfileTicket(
            self.sequence
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .map_err(|_| vec![problem(&self.root, "Profiling ticket sequence exhausted")])?,
        );
        let status = PlayProfileStatus {
            ticket,
            target: request.target.clone(),
            finished: false,
            outcome: None,
            recording: self
                .targets
                .iter()
                .find(|row| row.target == request.target)
                .is_some_and(|row| row.recording),
            profile: None,
            diagnostics: Vec::new(),
        };
        self.tx
            .try_send(Message::Request {
                request: request.clone(),
                status: Box::new(status.clone()),
                reply: self.reply.clone(),
            })
            .map_err(|_| {
                vec![problem(
                    &self.root,
                    "Profiling control queue is full or disconnected",
                )]
            })?;
        self.statuses.insert(ticket, (request.action, status));
        Ok(ticket)
    }

    /// Latest owned result; unknown and retired tickets return `None`.
    pub fn poll(&mut self, ticket: PlayProfileTicket) -> Option<PlayProfileStatus> {
        self.drain();
        self.statuses.get(&ticket).map(|(_, status)| status.clone())
    }
}
