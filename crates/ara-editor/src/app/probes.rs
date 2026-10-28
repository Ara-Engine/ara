//! Application-owner adapter for derived probe jobs. This never submits GPU work.
use super::viewport::SceneView;
use crate::{
    Editor,
    probe_capture::{Event, Request, SceneBinding},
};
use ara_diag::{Diagnostic, codes};
use ara_render::Renderer;
use ara_runtime_render::probes::{ProbeFailure, ProbeJob, ProbeJobStatus, ProbeState};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};

const REQUESTS_PER_FRAME: usize = 4;
const EVENTS: usize = 128;
const CADENCE: Duration = Duration::from_millis(250);

struct Subscription {
    session: String,
    binding: SceneBinding,
    backend: ProbeJob,
    entities: Vec<String>,
    stale: bool,
    terminal_queued: bool,
    last_progress: Option<(Option<f32>, bool, String)>,
}

/// Changes the application uses to schedule the next ordinary Scene/UI frame.
#[derive(Default)]
pub(super) struct Update {
    pub scene_dirty: bool,
    pub changed: bool,
    pub busy: bool,
}

/// One owner for one SceneView lifetime. Retain it through normal final drain.
#[derive(Default)]
pub(super) struct Owner {
    jobs: BTreeMap<String, Subscription>,
    outgoing: VecDeque<Event>,
    cancel_sent: BTreeSet<u64>,
    bound_doc: Option<String>,
    /// Last UI Scene observed before protocol requests are pumped.
    observed_doc: Option<String>,
    cache_root: Option<PathBuf>,
    next_poll: Option<Instant>,
    last_snapshot: Option<(SceneBinding, u64)>,
    /// Host read-model version stays monotonic across backend/controller resets.
    query_version: u64,
    version_exhausted: bool,
    unavailable: Option<String>,
    blocker: Option<String>,
    draining: bool,
    refresh: bool,
    /// A terminal Host receipt alone does not attest renderer/cache retirement.
    retirement_pending: bool,
}

impl Owner {
    /// Invalidate the read model before pumping requests after a UI Scene switch.
    /// True asks the caller to tick the Host, then retry until false: an existing
    /// full event channel must drain before it can accept this invalidation.
    pub(super) fn invalidate_scene(&mut self, editor: &mut Editor) -> bool {
        self.observe_scene(&editor.state.doc);
        let delivered = self.flush(editor);
        delivered
            || self
                .outgoing
                .iter()
                .any(|event| matches!(event, Event::Unavailable { .. }))
    }

    fn observe_scene(&mut self, doc: &str) {
        if self.observed_doc.as_deref() == Some(doc) {
            return;
        }
        let previous = self
            .observed_doc
            .as_deref()
            .or(self.bound_doc.as_deref())
            .or_else(|| {
                self.last_snapshot
                    .as_ref()
                    .map(|(binding, _)| binding.doc.as_str())
            });
        let changed_scene = previous.is_some_and(|previous| previous != doc);
        self.observed_doc = Some(doc.to_owned());
        if !changed_scene {
            return;
        }
        self.last_snapshot = None;
        self.refresh = true;
        let reason = "The active Scene changed; its current renderer snapshot is not available yet";
        self.unavailable = Some(reason.into());
        self.outgoing
            .retain(|event| !matches!(event, Event::Snapshot { .. } | Event::Unavailable { .. }));
        self.outgoing.push_front(Event::Unavailable {
            reason: reason.into(),
        });
    }

    /// Call every application iteration after the Scene update, including when
    /// that update was skipped. No waits, I/O, extraction, or renderer submission.
    pub(super) fn poll(
        &mut self,
        editor: &mut Editor,
        scene: &mut SceneView,
        renderer: &mut Renderer,
    ) -> ara_ui::Result<Update> {
        let mut update = Update::default();
        update.changed |= self.flush(editor);
        if !self.outgoing.is_empty() {
            return Ok(self.update(scene, update));
        }
        let now = Instant::now();
        let due = self.next_poll.is_none_or(|next| now >= next);
        if due {
            self.next_poll = Some(now + CADENCE);
            self.poll_jobs(editor, scene);
        }
        for _ in 0..REQUESTS_PER_FRAME {
            if self.outgoing.len() >= EVENTS - 2 {
                break;
            }
            let Some(request) = editor.probe_captures.try_recv() else {
                break;
            };
            self.request(editor, scene, request);
        }
        if due {
            self.cancel_eligible(editor, scene);
        }
        if due
            && self.retirement_pending
            && (self.draining
                || self.jobs.is_empty()
                || self.jobs.iter().any(|(job, sub)| {
                    !sub.terminal_queued && editor.probe_captures.cancel_requested(job)
                }))
        {
            match scene.probe_retirement_pending(renderer) {
                Ok(false) => {
                    self.blocker = None;
                    self.retirement_pending = false;
                    update.changed = true;
                    update.scene_dirty = true;
                }
                Ok(true) => {
                    self.blocker = Some("Probe GPU/cache retirement is still pending".into())
                }
                Err(diagnostic) => self.blocker = Some(diagnostic.message),
            }
        }
        if due && self.outgoing.len() < EVENTS {
            self.snapshot(editor, scene);
            self.refresh = false;
        }
        update.changed |= self.flush(editor);
        Ok(self.update(scene, update))
    }

    fn update(&self, scene: &mut SceneView, mut update: Update) -> Update {
        update.busy = self.busy();
        update.scene_dirty |= self.jobs.values().any(|sub| !sub.terminal_queued);
        if update.scene_dirty {
            scene.wake_probe_work();
        }
        update
    }

    fn flush(&mut self, editor: &mut Editor) -> bool {
        let mut changed = false;
        while let Some(event) = self.outgoing.pop_front() {
            let terminal = match &event {
                Event::Status { job, status, .. } if status.finished => Some(job.clone()),
                _ => None,
            };
            match editor.probe_captures.send(event) {
                Ok(()) => {
                    changed = true;
                    if let Some(job) = terminal {
                        self.jobs.remove(&job);
                    }
                }
                Err(event) => {
                    self.outgoing.push_front(*event);
                    break;
                }
            }
        }
        self.cancel_sent
            .retain(|backend| self.jobs.values().any(|sub| sub.backend.0 == *backend));
        if self.jobs.is_empty() {
            self.blocker = None;
        }
        changed
    }

    fn request(&mut self, editor: &mut Editor, scene: &mut SceneView, request: Request) {
        match request {
            Request::Refresh => self.refresh = true,
            Request::Cancel { .. } => { /* Bridge already records this subscriber's intent. */ }
            Request::Recapture {
                job,
                session,
                doc,
                entities,
                source_revision,
                why,
            } => {
                let rejected = if self.draining || editor.probe_captures.cancel_requested(&job) {
                    Some(vec![
                        Diagnostic::error(
                            codes::PROBE_STALE,
                            "The probe capture intent was cancelled before allocation",
                        )
                        .doc(&doc),
                    ])
                } else {
                    self.bind(editor, scene).err()
                };
                if let Some(diagnostics) = rejected {
                    self.outgoing.push_back(Event::Rejected {
                        job,
                        session,
                        diagnostics,
                    });
                    return;
                }
                match scene.recapture_probes(
                    editor,
                    &doc,
                    source_revision,
                    &entities,
                    why.as_deref(),
                ) {
                    Ok((binding, backend)) => {
                        self.retirement_pending = true;
                        self.jobs.insert(
                            job.clone(),
                            Subscription {
                                session: session.clone(),
                                binding: binding.clone(),
                                backend,
                                entities,
                                stale: false,
                                terminal_queued: false,
                                last_progress: None,
                            },
                        );
                        self.outgoing.push_back(Event::Started {
                            job,
                            session,
                            binding,
                            backend,
                        });
                        self.refresh = true;
                    }
                    Err(diagnostics) => self.outgoing.push_back(Event::Rejected {
                        job,
                        session,
                        diagnostics,
                    }),
                }
            }
        }
    }

    fn bind(&mut self, editor: &Editor, scene: &mut SceneView) -> Result<(), Vec<Diagnostic>> {
        let root = self
            .cache_root
            .get_or_insert_with(|| editor.host.root().join(".ara/cache/probes"));
        if self.bound_doc.as_deref() != Some(editor.state.doc.as_str())
            || !scene.probe_has_binding(root)
        {
            if !self.jobs.is_empty() {
                return Err(vec![Diagnostic::error(codes::PROBE_STALE,
                    "The previous probe owner is retiring; wait before binding a replacement Scene").doc(&editor.state.doc)]);
            }
            scene.bind_probe_scene(editor, root)?;
            self.bound_doc = Some(editor.state.doc.clone());
            self.last_snapshot = None;
        }
        Ok(())
    }

    fn poll_jobs(&mut self, editor: &mut Editor, scene: &SceneView) {
        for (job, sub) in &mut self.jobs {
            if sub.terminal_queued || self.outgoing.len() >= EVENTS {
                continue;
            }
            let source_changed = editor.state.doc == sub.binding.doc
                && !scene.probe_source_matches(editor, &sub.binding);
            let Some(mut status) = scene.probe_job(sub.backend) else {
                if source_changed {
                    sub.stale = true;
                    editor.probe_captures.request_cancel(job);
                }
                self.blocker = Some(format!(
                    "Backend probe job {} is unavailable; retirement remains unconfirmed",
                    sub.backend.0
                ));
                continue;
            };
            // A published Done receipt observed now is real even during a later
            // Scene close/cache drain. Only previously observed in-flight work
            // can be rejected as a stale completion.
            if observe_generation(sub, &mut status, source_changed) {
                editor.probe_captures.request_cancel(job);
            }
            let changed = status.finished
                || sub
                    .last_progress
                    .as_ref()
                    .is_none_or(|(progress, cancellable, message)| {
                        *progress != status.progress
                            || *cancellable != status.cancellable
                            || *message != status.message
                    });
            if changed {
                sub.last_progress =
                    Some((status.progress, status.cancellable, status.message.clone()));
                sub.terminal_queued = status.finished;
                self.outgoing.push_back(Event::Status {
                    job: job.clone(),
                    session: sub.session.clone(),
                    binding: sub.binding.clone(),
                    status,
                });
            }
        }
    }

    fn cancel_eligible(&mut self, editor: &mut Editor, scene: &mut SceneView) {
        for (job, sub) in &self.jobs {
            if sub.terminal_queued || self.cancel_sent.contains(&sub.backend.0) {
                continue;
            }
            if let Some(backend) = editor.probe_captures.cancel_target(job) {
                if scene.cancel_probe_job(backend).is_some() {
                    self.cancel_sent.insert(backend.0);
                } else {
                    self.blocker = Some(format!(
                        "Backend probe job {} did not acknowledge cancellation; ownership remains active",
                        backend.0
                    ));
                }
            }
        }
    }

    fn snapshot(&mut self, editor: &mut Editor, scene: &mut SceneView) {
        let result = self
            .bind(editor, scene)
            .and_then(|()| scene.probe_snapshot(editor));
        match result {
            Ok((binding, mut snapshot)) => {
                self.unavailable = None;
                let changed = self
                    .last_snapshot
                    .as_ref()
                    .is_none_or(|(old, version)| *old != binding || *version != snapshot.version);
                let version = match self.project_version(&binding, snapshot.version) {
                    Ok(version) => version,
                    Err(diagnostic) => {
                        self.blocker = Some(diagnostic.message.clone());
                        if !self.version_exhausted {
                            self.version_exhausted = true;
                            if let Err(error) = editor.action(
                                "diag.report",
                                json!({"engine":true,"diagnostics":[diagnostic]}),
                            ) {
                                self.blocker = Some(format!(
                                    "Probe query version exhausted; diagnostic reporting failed: {error}"
                                ));
                            }
                            self.outgoing.push_back(Event::Unavailable {
                                reason:
                                    "Probe query version exhausted; restart this editor session"
                                        .into(),
                            });
                        }
                        return;
                    }
                };
                if self.refresh || changed {
                    // Preserve the backend version in last_snapshot/provenance;
                    // wire since compares this monotonic Host projection counter.
                    snapshot.version = version;
                    self.outgoing
                        .push_back(Event::Snapshot { binding, snapshot });
                }
            }
            Err(diagnostics) => {
                let reason = diagnostics.first().map_or(
                    "No current probe renderer snapshot is available",
                    |diagnostic| diagnostic.message.as_str(),
                );
                if self.refresh || self.unavailable.as_deref() != Some(reason) {
                    self.unavailable = Some(reason.to_owned());
                    self.last_snapshot = None;
                    self.outgoing.push_back(Event::Unavailable {
                        reason: reason.to_owned(),
                    });
                }
            }
        }
    }

    fn project_version(
        &mut self,
        binding: &SceneBinding,
        backend_version: u64,
    ) -> Result<u64, Diagnostic> {
        if self
            .last_snapshot
            .as_ref()
            .is_none_or(|(old, version)| old != binding || *version != backend_version)
        {
            let next = self.query_version.checked_add(1).ok_or_else(|| {
                Diagnostic::error(
                    crate::codes::RENDER,
                    "Probe query projection version exhausted; restart this editor session",
                )
            })?;
            self.query_version = next;
            self.last_snapshot = Some((binding.clone(), backend_version));
        }
        Ok(self.query_version)
    }

    /// True means SAFE to adopt/drop the old Scene. Call before Scene publication
    /// and keep calling poll even while false. Never destroy a retained owner to
    /// escape a missing backend status or an unacknowledged cancellation.
    pub(super) fn prepare_scene_change(
        &mut self,
        editor: &mut Editor,
        scene: &mut SceneView,
        _renderer: &mut Renderer,
    ) -> ara_ui::Result<bool> {
        let mut safe = true;
        for (job, sub) in &self.jobs {
            if sub.binding.doc != editor.state.doc
                || !scene.probe_source_matches(editor, &sub.binding)
            {
                safe = false;
                if sub.binding.doc != editor.state.doc {
                    editor.probe_captures.cancel_scene(&sub.binding.doc);
                } else {
                    editor.probe_captures.request_cancel(job);
                }
            }
        }
        Ok(safe && (!self.jobs.is_empty() || !self.retirement_pending))
    }

    /// Use before raster destruction/path replacement even when the doc is unchanged.
    /// True means SAFE. This does not permanently drain later fresh requests.
    pub(super) fn prepare_replacement(&mut self, editor: &mut Editor) -> bool {
        for job in self.jobs.keys() {
            editor.probe_captures.request_cancel(job);
        }
        if self.jobs.is_empty() && !self.retirement_pending {
            self.bound_doc = None;
            self.last_snapshot = None;
            true
        } else {
            false
        }
    }

    pub(super) fn begin_drain(&mut self, editor: &mut Editor) {
        self.draining = true;
        editor.probe_captures.begin_drain();
    }
    pub(super) fn busy(&self) -> bool {
        !self.jobs.is_empty() || !self.outgoing.is_empty() || self.retirement_pending
    }
    pub(super) fn resource_report(&self) -> Value {
        json!({"activeHostSubscriptions":self.jobs.len(),"queuedEvents":self.outgoing.len(),"draining":self.draining,
            "ownerGenerations":self.jobs.values().map(|sub| sub.backend.0).collect::<BTreeSet<_>>().len(),
            "terminalAwaitingDelivery":self.jobs.values().filter(|sub|sub.terminal_queued).count(),"blocker":self.blocker,"refreshRequested":self.refresh,
            "queryVersion":self.query_version,"backendSnapshot":self.last_snapshot.as_ref().map(|(binding,version)|json!({"binding":binding,"version":version})),
            "jobs":self.jobs.iter().map(|(job,sub)|json!({"job":job,"session":sub.session,"binding":sub.binding,
                "backend":sub.backend.0,"stale":sub.stale,"terminalQueued":sub.terminal_queued})).collect::<Vec<_>>(),
            "rendererRetirementPending":self.retirement_pending,"retired":!self.busy()})
    }
}

fn observe_generation(
    sub: &mut Subscription,
    status: &mut ProbeJobStatus,
    source_changed: bool,
) -> bool {
    if !status.finished && source_changed {
        sub.stale = true;
    }
    if status.finished && sub.stale {
        reject_stale_completion(sub, status);
    }
    sub.stale && !status.finished
}

fn reject_stale_completion(sub: &Subscription, status: &mut ProbeJobStatus) {
    let receipts = std::mem::take(&mut status.receipts);
    if !receipts.is_empty() {
        status.diagnostics.push(
            Diagnostic::error(
                codes::PROBE_STALE,
                "A late probe receipt arrived after source invalidation; the Host rejects it",
            )
            .doc(&sub.binding.doc)
            .data(json!({"rejectedReceipts":receipts})),
        );
    }
    for entity in &sub.entities {
        let diagnostic = Diagnostic::error(codes::PROBE_STALE,
            "Probe source changed while this generation was in flight; its replacement receipt is rejected")
            .doc(&sub.binding.doc).record(entity).path("ReflectionProbe");
        if let Some(failure) = status
            .failures
            .iter_mut()
            .find(|failure| failure.doc == sub.binding.doc && failure.entity == *entity)
        {
            failure.state = ProbeState::Failed;
            failure.diagnostics.push(diagnostic);
        } else {
            status.failures.push(ProbeFailure {
                doc: sub.binding.doc.clone(),
                entity: entity.clone(),
                state: ProbeState::Failed,
                diagnostics: vec![diagnostic],
            });
        }
    }
    status.message = "Probe source changed; late capture generation rejected".into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_runtime_render::probes::ProbeReceipt;

    fn subscription() -> Subscription {
        Subscription {
            session: "session-original".into(),
            binding: SceneBinding {
                doc: "sc_probe".into(),
                scene_revision: 7,
                source_revision: 19,
            },
            backend: ProbeJob(3),
            entities: vec!["e_probe".into()],
            stale: false,
            terminal_queued: false,
            last_progress: None,
        }
    }
    fn status(finished: bool) -> ProbeJobStatus {
        ProbeJobStatus {
            job: ProbeJob(3),
            finished,
            progress: Some(if finished { 1. } else { 0.5 }),
            message: "Measured renderer work".into(),
            cancellable: !finished,
            receipts: if finished {
                vec![ProbeReceipt {
                    doc: "sc_probe".into(),
                    entity: "e_probe".into(),
                    generation: 2,
                    source_revision: 7,
                    source_hash: "source".into(),
                    capture_hash: "capture".into(),
                    resolution: 256,
                    faces: 6,
                    cache_path: ".ara/cache/probes/capture".into(),
                    published: true,
                }]
            } else {
                vec![]
            },
            failures: vec![],
            diagnostics: vec![],
        }
    }

    #[test]
    fn source_change_observed_in_flight_rejects_late_receipt_with_exact_probe_location() {
        let mut sub = subscription();
        assert!(observe_generation(&mut sub, &mut status(false), true));
        let mut terminal = status(true);
        assert!(!observe_generation(&mut sub, &mut terminal, true));
        assert!(terminal.finished);
        assert!(terminal.receipts.is_empty());
        assert_eq!(terminal.failures.len(), 1);
        assert_eq!(terminal.failures[0].doc, "sc_probe");
        assert_eq!(terminal.failures[0].entity, "e_probe");
        assert_eq!(terminal.failures[0].diagnostics[0].code, codes::PROBE_STALE);
        assert_eq!(
            terminal.failures[0].diagnostics[0]
                .location
                .record
                .as_deref(),
            Some("e_probe")
        );
        assert_eq!(terminal.job.0, sub.backend.0);
        assert_eq!(sub.session, "session-original");
    }

    #[test]
    fn done_already_published_when_close_is_observed_remains_a_real_receipt() {
        let mut sub = subscription();
        let mut terminal = status(true);
        assert!(!observe_generation(&mut sub, &mut terminal, true));
        assert!(!sub.stale);
        assert!(terminal.failures.is_empty());
        assert_eq!(terminal.receipts.len(), 1);
        assert!(terminal.receipts[0].published);
        assert_eq!(terminal.receipts[0].source_revision, 7);
    }

    #[test]
    fn scene_invalidation_preserves_terminal_delivery_and_retirement_ownership() {
        let mut owner = Owner::default();
        let sub = subscription();
        owner.bound_doc = Some(sub.binding.doc.clone());
        owner.observed_doc = Some(sub.binding.doc.clone());
        owner.last_snapshot = Some((sub.binding.clone(), 8));
        owner.retirement_pending = true;
        owner.outgoing.push_back(Event::Status {
            job: "host-job".into(),
            session: sub.session.clone(),
            binding: sub.binding.clone(),
            status: status(true),
        });
        owner.outgoing.push_back(Event::Unavailable {
            reason: "Previous read-model state".into(),
        });
        owner.jobs.insert("host-job".into(), sub);

        owner.observe_scene("sc_newscene");
        assert!(
            matches!(owner.outgoing.front(), Some(Event::Unavailable { reason }) if reason.contains("active Scene changed"))
        );
        assert_eq!(owner.outgoing.len(), 2);
        assert!(
            matches!(owner.outgoing.back(), Some(Event::Status { job, status, .. }) if job == "host-job" && status.finished)
        );
        assert_eq!(owner.jobs.len(), 1);
        assert_eq!(owner.jobs["host-job"].binding.doc, "sc_probe");
        assert!(owner.retirement_pending);
        assert!(owner.busy());
        assert!(owner.last_snapshot.is_none());
        assert!(owner.refresh);

        owner.observe_scene("sc_newscene");
        assert_eq!(
            owner.outgoing.len(),
            2,
            "same Scene does not invalidate again"
        );
    }

    #[test]
    fn observing_current_bound_scene_preserves_its_valid_snapshot() {
        let mut owner = Owner::default();
        let binding = subscription().binding;
        owner.bound_doc = Some(binding.doc.clone());
        owner.last_snapshot = Some((binding.clone(), 8));
        owner.observe_scene(&binding.doc);
        owner.observe_scene(&binding.doc);
        assert_eq!(owner.last_snapshot.as_ref(), Some(&(binding, 8)));
        assert!(owner.outgoing.is_empty());
        assert!(!owner.refresh);
    }

    #[test]
    fn query_versions_survive_backend_reset_and_fail_instead_of_wrapping() {
        let mut owner = Owner::default();
        let binding = subscription().binding;
        assert_eq!(owner.project_version(&binding, 8).ok(), Some(1));
        assert_eq!(owner.project_version(&binding, 8).ok(), Some(1));
        owner.last_snapshot = None; // A replacement controller with the same Scene.
        assert_eq!(owner.project_version(&binding, 1).ok(), Some(2));
        assert_eq!(owner.project_version(&binding, 2).ok(), Some(3));
        owner.query_version = u64::MAX;
        owner.last_snapshot = None;
        let error = owner
            .project_version(&binding, 1)
            .expect_err("never wrap a since cursor");
        assert_eq!(error.code, crate::codes::RENDER);
        assert_eq!(owner.query_version, u64::MAX);
        assert!(owner.last_snapshot.is_none());
    }
}
