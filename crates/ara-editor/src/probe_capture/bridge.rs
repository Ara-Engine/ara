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
    /// Canonical document ID of the retained Scene.
    pub doc: String,
    /// Renderer content counter (ProbeSnapshot.scene_revision / receipt source_revision).
    pub scene_revision: u64,
    /// Host Store revision used to build that retained Scene/frame.
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
    Cancel {
        job: String,
        session: String,
    },
    /// Publish a full snapshot (call the backend with since=None), or Unavailable.
    Refresh,
}

/// Only the owner may publish these after checking current Scene/frame provenance.
pub(crate) enum Event {
    Started {
        job: String,
        session: String,
        binding: SceneBinding,
        backend: ProbeJob,
    },
    Status {
        job: String,
        session: String,
        binding: SceneBinding,
        status: ProbeJobStatus,
    },
    /// Rejection before any backend generation was allocated.
    Rejected {
        job: String,
        session: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// Always a complete listing, including zero authored probes. Not a since delta.
    Snapshot {
        binding: SceneBinding,
        snapshot: ProbeSnapshot,
    },
    /// No current renderer snapshot exists; old scene rows must not masquerade as current.
    Unavailable { reason: String },
}

struct Tracked {
    session: String,
    doc: String,
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
    draining: bool,
}

impl Bridge {
    pub(super) fn new(requests: Receiver<Request>, events: SyncSender<Event>) -> Self {
        Self {
            requests,
            events,
            pending: VecDeque::new(),
            tracked: BTreeMap::new(),
            disconnected: false,
            draining: false,
        }
    }

    fn collect(&mut self) {
        while self.pending.len() < super::CHANNEL_CAPACITY {
            match self.requests.try_recv() {
                Ok(request) => {
                    match &request {
                        Request::Recapture {
                            job, session, doc, ..
                        } => {
                            self.tracked.insert(
                                job.clone(),
                                Tracked {
                                    session: session.clone(),
                                    doc: doc.clone(),
                                    backend: None,
                                    binding: None,
                                    cancelled: self.draining,
                                    cancellable: true,
                                },
                            );
                        }
                        Request::Cancel { job, session } => {
                            if let Some(tracked) =
                                self.tracked.get_mut(job).filter(|t| &t.session == session)
                            {
                                tracked.cancelled = true;
                            }
                        }
                        Request::Refresh => {}
                    }
                    self.pending.push_back(request);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
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
    pub(crate) fn send(&mut self, event: Event) -> Result<(), Box<Event>> {
        self.collect();
        if !self.valid(&event) {
            return Err(Box::new(event));
        }
        let update = match &event {
            Event::Started {
                job,
                binding,
                backend,
                ..
            } => TrackingUpdate::Started(job.clone(), binding.clone(), backend.0),
            Event::Status { job, status, .. } => {
                TrackingUpdate::Status(job.clone(), status.cancellable, status.finished)
            }
            Event::Rejected { job, .. } => TrackingUpdate::Rejected(job.clone()),
            Event::Snapshot { .. } | Event::Unavailable { .. } => TrackingUpdate::None,
        };
        match self.events.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event) | TrySendError::Disconnected(event)) => {
                return Err(Box::new(event));
            }
        }
        match update {
            TrackingUpdate::Started(job, binding, backend) => {
                if let Some(tracked) = self.tracked.get_mut(&job) {
                    tracked.backend = Some(backend);
                    tracked.binding = Some(binding);
                }
            }
            TrackingUpdate::Status(job, cancellable, finished) => {
                if finished {
                    self.tracked.remove(&job);
                } else if let Some(tracked) = self.tracked.get_mut(&job) {
                    tracked.cancellable &= cancellable;
                }
            }
            TrackingUpdate::Rejected(job) => {
                self.tracked.remove(&job);
            }
            TrackingUpdate::None => {}
        }
        Ok(())
    }

    fn valid(&self, event: &Event) -> bool {
        match event {
            Event::Started { job, session, .. } | Event::Rejected { job, session, .. } => self
                .tracked
                .get(job)
                .is_some_and(|t| &t.session == session && t.backend.is_none()),
            Event::Status {
                job,
                session,
                binding,
                status,
            } => self.tracked.get(job).is_some_and(|t| {
                &t.session == session
                    && t.backend == Some(status.job.0)
                    && t.binding.as_ref() == Some(binding)
            }),
            Event::Snapshot { binding, snapshot } => {
                !binding.doc.is_empty() && binding.scene_revision == snapshot.scene_revision
            }
            Event::Unavailable { reason } => !reason.trim().is_empty(),
        }
    }

    /// Returns a backend token only when cancellation cannot cancel another live
    /// Host subscriber, including one from a different session sharing this generation.
    pub(crate) fn cancel_target(&self, job: &str) -> Option<ProbeJob> {
        let tracked = self.tracked.get(job)?;
        let backend = tracked.backend?;
        (tracked.cancelled
            && tracked.cancellable
            && !self
                .tracked
                .values()
                .any(|t| t.backend.is_none() && !t.cancelled)
            && self
                .tracked
                .values()
                .filter(|t| t.backend == Some(backend))
                .all(|t| t.cancelled && t.cancellable))
        .then_some(ProbeJob(backend))
    }

    pub(crate) fn cancel_requested(&self, job: &str) -> bool {
        self.tracked.get(job).is_some_and(|t| t.cancelled)
    }

    /// Invalidating one source generation must not cancel a later request for the
    /// same Scene, or another subscriber still using a coalesced backend job.
    pub(crate) fn request_cancel(&mut self, job: &str) {
        self.collect();
        if let Some(tracked) = self.tracked.get_mut(job).filter(|t| !t.cancelled) {
            tracked.cancelled = true;
            self.pending.push_back(Request::Cancel {
                job: job.to_owned(),
                session: tracked.session.clone(),
            });
        }
    }

    /// Scene closure/drain cancels UI intent but retains every owner resource until
    /// its actual terminal Status (finished=true), including coalesced generations.
    pub(crate) fn begin_drain(&mut self) {
        self.draining = true;
        self.cancel_matching(None);
    }

    /// Call before replacing/closing a Scene owner. Keep polling its backend jobs
    /// through retirement; a new Scene must not inherit the previous bindings.
    pub(crate) fn cancel_scene(&mut self, doc: &str) {
        self.cancel_matching(Some(doc));
    }

    fn cancel_matching(&mut self, doc: Option<&str>) {
        self.collect();
        for (job, tracked) in &mut self.tracked {
            if !tracked.cancelled && doc.is_none_or(|doc| doc == tracked.doc) {
                tracked.cancelled = true;
                self.pending.push_back(Request::Cancel {
                    job: job.clone(),
                    session: tracked.session.clone(),
                });
            }
        }
    }

    pub(crate) fn busy(&mut self) -> bool {
        self.collect();
        !self.tracked.is_empty() || self.pending_work() != 0
    }

    // Read-model refreshes own no renderer generation, file task or Host job.
    // They remain deliverable but must not manufacture a shutdown obligation.
    fn pending_work(&self) -> usize {
        self.pending
            .iter()
            .filter(|request| !matches!(request, Request::Refresh))
            .count()
    }

    /// Resource proof reports retirement, separately from a cancelled Host receipt.
    pub(crate) fn resource_report(&mut self) -> Value {
        self.collect();
        let pending_work = self.pending_work();
        json!({"activeHostJobs":self.tracked.len(),"pendingRequests":pending_work,
            "pendingRefreshes":self.pending.len() - pending_work,
            "unassigned":self.tracked.values().filter(|t|t.backend.is_none()).count(),
            "cancelRequested":self.tracked.values().filter(|t|t.cancelled).count(),
            "ownerGenerations":self.tracked.values().filter_map(|t|t.backend).collect::<std::collections::BTreeSet<_>>().len(),
            "jobs":self.tracked.iter().map(|(job,t)|json!({"job":job,"session":t.session,"doc":t.doc,
                "backend":t.backend,"binding":t.binding,"cancelRequested":t.cancelled,"cancellable":t.cancellable})).collect::<Vec<_>>(),
            "draining":self.draining,"requestChannelClosed":self.disconnected,"retired":self.tracked.is_empty() && pending_work == 0})
    }
}
