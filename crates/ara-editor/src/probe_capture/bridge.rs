use ara_diag::Diagnostic;
use ara_runtime_render::probes::{ProbeJob, ProbeJobStatus, ProbeSnapshot};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError};

/// Identity of the exact retained Scene/frame validated by the application owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SceneBinding {
    pub doc: String,
    pub scene_revision: u64,
    pub source_revision: u64,
}

/// Commands consumed without waiting at the application/render ownership boundary.
#[derive(Debug)]
pub(crate) enum Request {
    Recapture {
        job: String,
        session: String,
        doc: String,
        entities: Vec<String>,
        source_revision: u64,
        why: Option<String>,
    },
    Cancel { job: String, session: String },
    /// Publish a full snapshot (call the backend with since=None), or Unavailable.
    Refresh,
}

/// Only the owner may publish these after checking current Scene/frame provenance.
pub(crate) enum Event {
    Started { job: String, session: String, binding: SceneBinding, backend: ProbeJob },
    Status { job: String, session: String, binding: SceneBinding, status: ProbeJobStatus },
    /// Rejection before any backend generation was allocated.
    Rejected { job: String, session: String, diagnostics: Vec<Diagnostic> },
    /// Always a complete listing, including zero authored probes. Not a since delta.
    Snapshot { binding: SceneBinding, snapshot: ProbeSnapshot },
    /// No current renderer snapshot exists; old scene rows must not masquerade as current.
    Unavailable { reason: String },
}

struct Tracked {
    session: String,
    backend: Option<u64>,
    binding: Option<SceneBinding>,
    cancelled: bool,
    cancellable: bool,
}

enum TrackingUpdate {
    Started(String, SceneBinding, u64),
    Status(String, bool, bool),
    Rejected(String),
    None,
}

/// The application retains this until every queued/in-flight capture has retired.
/// A terminal Host cancellation alone never removes an owner resource.
pub(crate) struct Bridge {
    requests: Receiver<Request>,
    events: SyncSender<Event>,
    pending: VecDeque<Request>,
    tracked: BTreeMap<String, Tracked>,
    disconnected: bool,
}

impl Bridge {
    pub(super) fn new(requests: Receiver<Request>, events: SyncSender<Event>) -> Self {
        Self { requests, events, pending: VecDeque::new(), tracked: BTreeMap::new(), disconnected: false }
    }

    fn collect(&mut self) {
        while self.pending.len() < super::CHANNEL_CAPACITY {
            match self.requests.try_recv() {
                Ok(request) => {
                    match &request {
                        Request::Recapture { job, session, .. } => {
                            self.tracked.insert(job.clone(), Tracked {
                                session: session.clone(), backend: None, binding: None,
                                cancelled: false, cancellable: true,
                            });
                        }
                        Request::Cancel { job, session } => {
                            if let Some(tracked) = self.tracked.get_mut(job).filter(|t| &t.session == session) {
                                tracked.cancelled = true;
                            }
                        }
                        Request::Refresh => {}
                    }
                    self.pending.push_back(request);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => { self.disconnected = true; break; }
            }
        }
    }

    /// Drains bounded protocol work; never performs renderer work or waits.
    pub(crate) fn try_recv(&mut self) -> Option<Request> {
        self.collect();
        self.pending.pop_front()
    }

    /// Publish a typed owner observation. Backpressure returns the unchanged event;
    /// retry it before further work. Invalid attribution also leaves tracking intact.
    pub(crate) fn send(&mut self, event: Event) -> Result<(), Event> {
        self.collect();
        if !self.valid(&event) { return Err(event); }
        let update = match &event {
            Event::Started { job, binding, backend, .. } => TrackingUpdate::Started(job.clone(), binding.clone(), backend.0),
            Event::Status { job, status, .. } => TrackingUpdate::Status(job.clone(), status.cancellable, status.finished),
            Event::Rejected { job, .. } => TrackingUpdate::Rejected(job.clone()),
            Event::Snapshot { .. } | Event::Unavailable { .. } => TrackingUpdate::None,
        };
        match self.events.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event) | TrySendError::Disconnected(event)) => return Err(event),
        }
        match update {
            TrackingUpdate::Started(job, binding, backend) => {
                if let Some(tracked) = self.tracked.get_mut(&job) {
                    tracked.backend = Some(backend);
                    tracked.binding = Some(binding);
                }
            }
            TrackingUpdate::Status(job, cancellable, finished) => {
                if finished { self.tracked.remove(&job); }
                else if let Some(tracked) = self.tracked.get_mut(&job) { tracked.cancellable &= cancellable; }
            }
            TrackingUpdate::Rejected(job) => { self.tracked.remove(&job); }
            TrackingUpdate::None => {}
        }
        Ok(())
    }

    fn valid(&self, event: &Event) -> bool {
        match event {
            Event::Started { job, session, .. } | Event::Rejected { job, session, .. } => {
                self.tracked.get(job).is_some_and(|t| &t.session == session && t.backend.is_none())
            }
            Event::Status { job, session, binding, status } => self.tracked.get(job).is_some_and(|t| {
                &t.session == session && t.backend == Some(status.job.0) && t.binding.as_ref() == Some(binding)
            }),
            Event::Snapshot { binding, snapshot } => !binding.doc.is_empty() && binding.scene_revision == snapshot.scene_revision,
            Event::Unavailable { reason } => !reason.trim().is_empty(),
        }
    }

    /// Returns a backend token only when cancellation cannot cancel another live
    /// Host subscriber, including one from a different session sharing this generation.
    pub(crate) fn cancel_target(&self, job: &str) -> Option<ProbeJob> {
        let tracked = self.tracked.get(job)?;
        let backend = tracked.backend?;
        (tracked.cancelled && tracked.cancellable && self.tracked.values()
            .filter(|t| t.backend == Some(backend)).all(|t| t.cancelled))
            .then_some(ProbeJob(backend))
    }

    pub(crate) fn cancel_requested(&self, job: &str) -> bool {
        self.tracked.get(job).is_some_and(|t| t.cancelled)
    }

    /// Scene closure/drain cancels UI intent but retains every owner resource until
    /// its actual terminal Status (finished=true), including coalesced generations.
    pub(crate) fn begin_drain(&mut self) {
        self.collect();
        for (job, tracked) in &mut self.tracked {
            if !tracked.cancelled {
                tracked.cancelled = true;
                self.pending.push_back(Request::Cancel { job: job.clone(), session: tracked.session.clone() });
            }
        }
    }

    pub(crate) fn busy(&mut self) -> bool {
        self.collect();
        !self.tracked.is_empty() || !self.pending.is_empty()
    }

    /// Resource proof reports retirement, separately from a cancelled Host receipt.
    pub(crate) fn resource_report(&mut self) -> Value {
        self.collect();
        json!({"activeHostJobs":self.tracked.len(),"pendingRequests":self.pending.len(),
            "unassigned":self.tracked.values().filter(|t|t.backend.is_none()).count(),
            "cancelRequested":self.tracked.values().filter(|t|t.cancelled).count(),
            "ownerGenerations":self.tracked.values().filter_map(|t|t.backend).collect::<std::collections::BTreeSet<_>>().len(),
            "requestChannelClosed":self.disconnected,"retired":self.tracked.is_empty() && self.pending.is_empty()})
    }
}
