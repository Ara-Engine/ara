use super::bridge::{Event, Request, SceneBinding};
use super::params::{RenderProbeCancelParams, RenderProbeRecaptureParams, RenderProbesQueryParams};
use ara_diag::{Diagnostic, Location};
use ara_host::{Cx, Method, Service};
use ara_runtime_render::probes::{ProbeJobStatus, ProbeSnapshot, ProbeState};
use ara_schema::SchemaDb;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};

struct Pending {
    session: String,
    doc: String,
    entities: BTreeSet<String>,
    source_revision: u64,
    binding: Option<SceneBinding>,
    backend: Option<u64>,
    cancel_requested: bool,
    cancel_sent: bool,
    cancellable: bool,
    progress: Option<(Option<f32>, String)>,
}

struct Cached {
    binding: SceneBinding,
    snapshot: ProbeSnapshot,
}

/// Protocol receipts and a bounded read model; no renderer or filesystem ownership.
pub(crate) struct ProbeService {
    requests: SyncSender<Request>,
    events: Receiver<Event>,
    pending: BTreeMap<String, Pending>,
    cached: Option<Cached>,
    unavailable: String,
    refresh_pending: bool,
    owner_disconnected: bool,
}

impl ProbeService {
    pub(super) fn new(requests: SyncSender<Request>, events: Receiver<Event>) -> Self {
        Self {
            requests,
            events,
            pending: BTreeMap::new(),
            cached: None,
            unavailable: "The Scene renderer has not published a probe snapshot".into(),
            refresh_pending: false,
            owner_disconnected: false,
        }
    }

    fn refresh(&mut self) {
        if !self.owner_disconnected
            && !self.refresh_pending
            && self.requests.try_send(Request::Refresh).is_ok()
        {
            self.refresh_pending = true;
        }
    }

    fn recapture(
        &mut self,
        cx: &mut Cx<'_>,
        params: RenderProbeRecaptureParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        if self.owner_disconnected {
            return Err(vec![crate::invalid(
                "The Scene probe owner has disconnected",
            )]);
        }
        validate_entities(&params.entities, false)?;
        if params.why.as_deref().is_some_and(|why| why.len() > 4096) {
            return Err(vec![crate::invalid(
                "Probe capture explanation is limited to 4096 bytes",
            )]);
        }
        if self.pending.len() >= super::MAX_JOBS {
            return Err(vec![crate::invalid(
                "Probe capture queue is full; wait for existing jobs to retire",
            )]);
        }
        let key = params
            .doc
            .as_deref()
            .or_else(|| {
                self.cached
                    .as_ref()
                    .map(|cached| cached.binding.doc.as_str())
            })
            .or_else(|| cx.main_scene())
            .ok_or_else(|| {
                vec![crate::invalid(
                    "Choose a current Scene before recapturing probes",
                )]
            })?;
        let doc = scene_id(cx, key)?;
        let source_revision = cx.store().revision();
        let job = cx.start_job("render.probe.recapture");
        let pending = Pending {
            session: params.session.clone(),
            doc: doc.clone(),
            entities: params.entities.iter().cloned().collect(),
            source_revision,
            binding: None,
            backend: None,
            cancel_requested: false,
            cancel_sent: false,
            cancellable: true,
            progress: None,
        };
        let request = Request::Recapture {
            job: job.clone(),
            session: params.session,
            doc,
            entities: params.entities,
            source_revision,
            why: params.why,
        };
        if self.requests.try_send(request).is_err() {
            let diagnostics = vec![crate::invalid(
                "The Scene probe owner is unavailable or its queue is full",
            )];
            cx.finish_job(&job, Err(diagnostics.clone()));
            return Err(diagnostics);
        }
        self.pending.insert(job.clone(), pending);
        cx.job_progress(&job, 0., "Waiting for the current Scene render boundary");
        Ok(json!({"ok":true,"job":job,"queued":true,"ops":[],"txn":null}))
    }

    fn cancel(
        &mut self,
        cx: &mut Cx<'_>,
        params: RenderProbeCancelParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        let record = cx
            .job_record(&params.job)
            .filter(|record| {
                record.name == "render.probe.recapture"
                    && record.session.as_deref() == Some(params.session.as_str())
            })
            .ok_or_else(|| {
                vec![crate::invalid(
                    "No probe capture job is owned by this session",
                )]
            })?;
        let cancellable =
            record.cancellable && self.pending.get(&params.job).is_some_and(|p| p.cancellable);
        let requested = cancellable && cx.cancel_job(&params.job);
        if requested && let Some(pending) = self.pending.get_mut(&params.job) {
            pending.cancel_requested = true;
        }
        self.send_cancellations(cx);
        Ok(
            json!({"ok":true,"job":params.job,"cancelRequested":requested,
            "cancellable":cancellable,"ops":[],"txn":null}),
        )
    }

    fn query(
        &mut self,
        cx: &Cx<'_>,
        params: RenderProbesQueryParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        if let Some(entities) = &params.entities {
            validate_entities(entities, true)?;
        }
        let doc = params
            .doc
            .as_deref()
            .map(|doc| scene_id(cx, doc))
            .transpose()?;
        self.refresh();
        let source_revision = cx.store().revision();
        let Some(cached) = self.cached.as_ref().filter(|cached| {
            cached.binding.source_revision == source_revision
                && doc.as_deref().is_none_or(|doc| cached.binding.doc == doc)
        }) else {
            let reason = if self
                .cached
                .as_ref()
                .is_some_and(|cached| cached.binding.source_revision != source_revision)
            {
                "The authored Scene changed; its current renderer snapshot is not available yet"
            } else if self.cached.is_some() {
                "No current renderer snapshot exists for the requested Scene"
            } else {
                self.unavailable.as_str()
            };
            return Ok(unavailable(reason));
        };
        let unchanged = params
            .since
            .is_some_and(|since| since >= cached.snapshot.version);
        let probes: Vec<_> = cached
            .snapshot
            .probes
            .iter()
            .filter(|row| {
                !unchanged
                    && row.doc == cached.binding.doc
                    && params
                        .entities
                        .as_ref()
                        .is_none_or(|entities| entities.contains(&row.entity))
            })
            .collect();
        Ok(
            json!({"version":cached.snapshot.version,"sceneRevision":cached.snapshot.scene_revision,
            "capabilities":cached.snapshot.capabilities,"probes":probes}),
        )
    }

    fn send_cancellations(&mut self, cx: &Cx<'_>) {
        if self.owner_disconnected {
            return;
        }
        for (job, pending) in &mut self.pending {
            pending.cancel_requested |= cx.job_cancelled(job);
            if pending.cancel_requested
                && !pending.cancel_sent
                && self
                    .requests
                    .try_send(Request::Cancel {
                        job: job.clone(),
                        session: pending.session.clone(),
                    })
                    .is_ok()
            {
                pending.cancel_sent = true;
            }
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, event: Event) {
        match event {
            Event::Snapshot { binding, snapshot } => {
                self.refresh_pending = false;
                if binding.doc.is_empty()
                    || snapshot.scene_revision != binding.scene_revision
                    || snapshot.probes.iter().any(|row| row.doc != binding.doc)
                {
                    cx.log(Diagnostic::error(
                        crate::codes::RENDER,
                        "Rejected a probe snapshot with inconsistent Scene provenance",
                    ));
                    return;
                }
                if self.cached.as_ref().is_some_and(|cached| {
                    cached.binding == binding && snapshot.version < cached.snapshot.version
                }) {
                    return;
                }
                self.cached = Some(Cached { binding, snapshot });
            }
            Event::Unavailable { reason } => {
                self.refresh_pending = false;
                self.cached = None;
                self.unavailable = reason;
            }
            Event::Rejected {
                job,
                session,
                diagnostics,
            } => {
                if self
                    .pending
                    .get(&job)
                    .is_none_or(|pending| pending.session != session || pending.backend.is_some())
                {
                    return;
                }
                let pending = self.pending.remove(&job).expect("matching pending request");
                let diagnostics = locate(
                    diagnostics,
                    &pending.doc,
                    pending.entities.iter().next().map(String::as_str),
                );
                cx.finish_job(
                    &job,
                    Err(if diagnostics.is_empty() {
                        vec![failure(
                            &pending.doc,
                            None,
                            "The Scene owner rejected the probe capture",
                        )]
                    } else {
                        diagnostics
                    }),
                );
            }
            Event::Started {
                job,
                session,
                binding,
                backend,
            } => {
                let Some(pending) = self
                    .pending
                    .get_mut(&job)
                    .filter(|pending| pending.session == session && pending.backend.is_none())
                else {
                    return;
                };
                pending.backend = Some(backend.0);
                pending.binding = Some(binding.clone());
                if pending.doc != binding.doc || pending.source_revision != binding.source_revision
                {
                    pending.cancel_requested = true;
                    cx.finish_job(&job, Err(vec![failure(&pending.doc, None,
                        "The Scene/frame source changed before this probe capture started; request it again for the current Scene")]));
                }
            }
            Event::Status {
                job,
                session,
                binding,
                status,
            } => {
                let Some(pending) = self.pending.get_mut(&job).filter(|pending| {
                    pending.session == session
                        && pending.backend == Some(status.job.0)
                        && pending.binding.as_ref() == Some(&binding)
                }) else {
                    return;
                };
                pending.cancellable &= status.cancellable;
                if !status.finished {
                    cx.set_job_cancellable(&job, pending.cancellable);
                }
                if status
                    .progress
                    .is_some_and(|p| !p.is_finite() || !(0.0..=1.0).contains(&p))
                {
                    pending.cancel_requested = true;
                    cx.finish_job(
                        &job,
                        Err(vec![failure(
                            &pending.doc,
                            None,
                            "The probe owner reported invalid progress",
                        )]),
                    );
                } else if pending.progress.as_ref().is_none_or(|(progress, message)| {
                    *progress != status.progress || *message != status.message
                }) {
                    cx.job_progress(&job, status.progress.unwrap_or(0.), &status.message);
                    pending.progress = Some((status.progress, status.message.clone()));
                }
                if status.finished {
                    let pending = self
                        .pending
                        .remove(&job)
                        .expect("matching pending owner generation");
                    finish(cx, &job, pending, status);
                }
            }
        }
    }
}

impl Service for ProbeService {
    fn name(&self) -> &'static str {
        "editor-probe-capture"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<RenderProbeRecaptureParams>(
                db,
                "render.probe.recapture",
                "Queue derived reflection-probe captures at the current Scene render boundary.",
            ),
            Method::new::<RenderProbeCancelParams>(
                db,
                "render.probe.cancel",
                "Cancel this session's cancellable reflection-probe capture job.",
            ),
            Method::new::<RenderProbesQueryParams>(
                db,
                "render.probes.query",
                "Read the last current-Scene probe capabilities, state and real capture receipts.",
            )
            .read_only(),
        ]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        match method {
            "render.probe.recapture" => self.recapture(cx, ara_host::parse(params)?),
            "render.probe.cancel" => self.cancel(cx, ara_host::parse(params)?),
            "render.probes.query" => self.query(cx, ara_host::parse(params)?),
            _ => Err(vec![crate::invalid("Unknown probe capture method")]),
        }
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        for _ in 0..super::CHANNEL_CAPACITY {
            match self.events.try_recv() {
                Ok(event) => self.event(cx, event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // A closed owner has not proved GPU retirement. Keep the resource
                    // registry pending; report failed receipts without inventing drain.
                    if !self.owner_disconnected {
                        self.owner_disconnected = true;
                        self.cached = None;
                        self.unavailable =
                            "The Scene probe owner disconnected before confirming retirement"
                                .into();
                        for (job, pending) in &self.pending {
                            cx.finish_job(
                                job,
                                Err(vec![failure(&pending.doc, None, &self.unavailable)]),
                            );
                        }
                    }
                    break;
                }
            }
        }
        self.send_cancellations(cx);
    }
}

fn scene_id(cx: &Cx<'_>, key: &str) -> Result<String, Vec<Diagnostic>> {
    let workspace = cx.store().workspace();
    workspace
        .find(key)
        .and_then(|id| workspace.doc(id.as_str()))
        .filter(|doc| doc.kind == "scene")
        .map(|doc| doc.id.to_string())
        .ok_or_else(|| {
            vec![crate::invalid(
                "Probe capture needs a loaded Scene document",
            )]
        })
}

fn validate_entities(entities: &[String], allow_empty: bool) -> Result<(), Vec<Diagnostic>> {
    if (!allow_empty && entities.is_empty())
        || entities.len() > super::MAX_ENTITIES
        || entities
            .iter()
            .any(|entity| entity.trim().is_empty() || entity.len() > 1024)
        || entities.iter().collect::<BTreeSet<_>>().len() != entities.len()
    {
        return Err(vec![crate::invalid(
            "Choose up to 256 distinct, nonempty probe entity addresses",
        )]);
    }
    Ok(())
}

fn unavailable(reason: &str) -> Value {
    json!({"version":0,"sceneRevision":0,"probes":[],"capabilities":{
        "reflectionProbes":false,"giDiffuse":false,"overdraw":false,
        "reasons":{"reflectionProbes":reason,"giDiffuse":reason,"overdraw":reason}}})
}

fn failure(doc: &str, entity: Option<&str>, message: &str) -> Diagnostic {
    let diagnostic = Diagnostic::error(crate::codes::RENDER, message)
        .doc(doc)
        .path("ReflectionProbe");
    if let Some(entity) = entity {
        diagnostic.record(entity)
    } else {
        diagnostic
    }
}

fn locate(diagnostics: Vec<Diagnostic>, doc: &str, entity: Option<&str>) -> Vec<Diagnostic> {
    let location = Location {
        doc: Some(doc.to_owned()),
        record: entity.map(str::to_owned),
        path: Some("ReflectionProbe".into()),
        ..Location::default()
    };
    diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.within(&location))
        .collect()
}

fn finish(cx: &mut Cx<'_>, job: &str, pending: Pending, status: ProbeJobStatus) {
    if cx.job_cancelled(job) {
        return;
    }
    let binding = pending
        .binding
        .expect("finished status has a validated owner binding");
    let mut diagnostics = locate(status.diagnostics, &pending.doc, None);
    let mut covered = BTreeSet::new();
    let mut invalid = false;
    for receipt in &status.receipts {
        invalid |= receipt.doc != pending.doc
            || !pending.entities.contains(&receipt.entity)
            || !covered.insert(receipt.entity.as_str())
            || receipt.source_revision != binding.scene_revision
            || receipt.generation == 0
            || receipt.faces != 6
            || receipt.resolution == 0
            || !receipt.published
            || receipt.source_hash.is_empty()
            || receipt.capture_hash.is_empty()
            || !receipt.cache_path.starts_with(".ara/cache/")
            || receipt.cache_path.contains(['\\', '\0'])
            || receipt.cache_path.split('/').any(|part| part == "..");
    }
    let mut cancelled = !status.failures.is_empty();
    for item in &status.failures {
        invalid |= item.doc != pending.doc
            || !pending.entities.contains(&item.entity)
            || !covered.insert(item.entity.as_str());
        cancelled &= matches!(item.state, ProbeState::Cancelled);
        if item.diagnostics.is_empty() {
            diagnostics.push(failure(
                &item.doc,
                Some(&item.entity),
                "Probe capture failed or was cancelled before publication",
            ));
        } else {
            diagnostics.extend(locate(
                item.diagnostics.clone(),
                &item.doc,
                Some(&item.entity),
            ));
        }
    }
    invalid |= covered.len() != pending.entities.len();
    if invalid {
        diagnostics.push(failure(&pending.doc, None, "The completed probe capture did not provide valid, source-matching coverage for the requested probes"));
    }
    if cancelled && !invalid && cx.cancel_job(job) {
        return;
    }
    if diagnostics.iter().any(Diagnostic::is_error) || !status.failures.is_empty() || invalid {
        if diagnostics.is_empty() {
            diagnostics.push(failure(&pending.doc, None, "Probe capture failed"));
        }
        if let Some(first) = diagnostics.first_mut() {
            first.data = Some(Box::new(
                json!({"probeCapture":{"receipts":status.receipts,"failures":status.failures,
                "scene":binding,"sourceRevision":pending.source_revision},"detail":first.data.take()}),
            ));
        }
        cx.finish_job(job, Err(diagnostics));
    } else {
        cx.finish_job(
            job,
            Ok(json!({"ok":true,"ops":[],"txn":null,
            "receipts":status.receipts,"failures":[],"diagnostics":diagnostics})),
        );
    }
}
