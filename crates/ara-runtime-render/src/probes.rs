//! Reflection-probe capture jobs (J7): the renderer side of `render.probe.recapture`,
//! `render.probe.cancel` and `render.probes.query`. A capture is derived runtime work,
//! never an authoring edit: no ops, no transaction, no document writes. Hosts (the
//! editor's Host-job adapter) own the typed service methods and job lifecycle; this
//! module owns the retained typed state they read.
//!
//! [`ProbeCaptures`] lives with its [`crate::RasterView`] on the render owner thread.
//! `recapture` resolves and validates on that thread and only queues; bounded GPU
//! work, readback polls and cache handoff run in the view's frame path. `snapshot`
//! reads retained state; `cancel` marks a job's generation.
//!
//! Identity is the scene's canonical document ID plus each probe entity's resolved
//! address ([`ProbeKey`]). `sourceHash` covers every capture input portably (probe
//! settings and pose, content, lighting, environment); it never hashes process-local
//! counters.
use ara_diag::{Diagnostic, codes};
pub use ara_render::capture::RenderCapabilities;
use ara_runtime::extract::RenderFrame;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A probe's identity: canonical document ID plus the entity's resolved address
/// (prefab-instance path included). `Ord` is the canonical tie order for overlapping
/// probes of equal priority.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeKey {
    /// Canonical document ID of the scene.
    pub doc: String,
    /// Resolved entity address within that document.
    pub entity: String,
}

/// A recapture job: one per request, covering every probe it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProbeJob(pub u64);

/// `render.probe.recapture`'s probes, resolved by the host against its retained scene.
#[derive(Debug, Clone, Copy)]
pub struct RecaptureRequest<'a> {
    /// The probes to capture.
    pub probes: &'a [ProbeKey],
    /// Why, for the job's message.
    pub why: Option<&'a str>,
}

/// Where a probe's capture stands. `Capturing` is setup and the first face;
/// `Progress` means bounded capture or filter work is underway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeState {
    /// Nothing requested since the last receipt (or ever).
    Idle,
    /// Requested and validated; waiting for the renderer.
    Queued,
    /// Setup and the first face.
    Capturing,
    /// Faces or filtering underway.
    Progress,
    /// Published; `lastReceipt` describes it.
    Done,
    /// Cancelled before publishing; the last valid capture is kept.
    Cancelled,
    /// Failed or discarded as stale; the last valid capture is kept.
    Failed,
}

/// The bounded work a capture is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeStage {
    /// Rendering the six cube faces.
    Faces,
    /// Reducing and GGX-prefiltering the faces.
    Filter,
    /// Atomically swapping in the new capture (brief, not cancellable).
    Publish,
}

/// A completed, published capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReceipt {
    /// Canonical document ID.
    pub doc: String,
    /// Resolved entity address.
    pub entity: String,
    /// The request generation this capture completed.
    pub generation: u64,
    /// The renderer's content revision it captured (ordering only, process-local).
    pub source_revision: u64,
    /// Portable hash of every capture input.
    pub source_hash: String,
    /// Hash of the completed, filtered capture.
    pub capture_hash: String,
    /// Texels per face side.
    pub resolution: u32,
    /// Faces captured (6).
    pub faces: u32,
    /// The capture's cache file, relative to the project (under `.ara/cache`).
    pub cache_path: String,
    /// Whether it is the capture surfaces shade with now.
    pub published: bool,
}

/// One probe's row in `render.probes.query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRow {
    /// Canonical document ID.
    pub doc: String,
    /// Resolved entity address.
    pub entity: String,
    /// The probe's `enabled` setting.
    pub enabled: bool,
    /// Latest request generation (0: never requested).
    pub generation: u64,
    /// Where its capture stands.
    pub state: ProbeState,
    /// The bounded work underway, if any.
    pub stage: Option<ProbeStage>,
    /// Measured progress 0-1, or `None` when the total work is unknown.
    pub progress: Option<f32>,
    /// Work units done (faces, then filtering).
    pub completed_work: Option<u32>,
    /// Work units in all, or `None` when unknown.
    pub total_work: Option<u32>,
    /// Whether a cancel can still stop it.
    pub cancellable: bool,
    /// The renderer's content revision now (ordering only).
    pub scene_revision: u64,
    /// Portable hash of the probe's current capture inputs (empty until the capture
    /// engine has computed it).
    pub source_hash: String,
    /// The published capture no longer matches the current inputs.
    pub stale: bool,
    /// A previous valid capture is still in use while a replacement is pending.
    pub retained_previous: bool,
    /// The published capture, if any.
    pub last_receipt: Option<ProbeReceipt>,
    /// Located problems with this probe.
    pub diagnostics: Vec<Diagnostic>,
}

/// A member of a job that did not complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeFailure {
    /// Canonical document ID.
    pub doc: String,
    /// Resolved entity address.
    pub entity: String,
    /// `Cancelled` or `Failed`.
    pub state: ProbeState,
    /// Why.
    pub diagnostics: Vec<Diagnostic>,
}

/// A job's status for `jobs.status`: ordinary progress and message plus structured
/// receipts, failures and diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeJobStatus {
    /// The job.
    pub job: ProbeJob,
    /// Every member has a receipt or a failure.
    pub finished: bool,
    /// Measured progress over all members, or `None` when unknown.
    pub progress: Option<f32>,
    /// One line for the job list.
    pub message: String,
    /// Whether a cancel can still stop any member.
    pub cancellable: bool,
    /// Completed members.
    pub receipts: Vec<ProbeReceipt>,
    /// Cancelled or failed members.
    pub failures: Vec<ProbeFailure>,
    /// Job-level problems.
    pub diagnostics: Vec<Diagnostic>,
}

/// `render.probe.cancel`'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOutcome {
    /// A cancel was recorded for at least one in-flight member.
    pub cancel_requested: bool,
    /// Whether any member was still cancellable.
    pub cancellable: bool,
}

/// `render.probes.query`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeSnapshot {
    /// Changes on any row transition, receipt or staleness change.
    pub version: u64,
    /// The renderer's content revision (ordering only).
    pub scene_revision: u64,
    /// What the renderer can do now, with reasons.
    pub capabilities: RenderCapabilities,
    /// Matching rows; empty when `since` is at least `version`.
    pub probes: Vec<ProbeRow>,
}

/// Which rows a snapshot returns.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeFilter<'a> {
    /// Only this document's probes.
    pub doc: Option<&'a str>,
    /// Only these entity addresses.
    pub entities: Option<&'a [String]>,
}

/// Cache file format version (`<sourceHash>.capture`: header line, then the image).
const CACHE_VERSION: u32 = 1;

/// The retained probe capture state of one view (see the module docs).
#[derive(Debug)]
pub struct ProbeCaptures {
    doc: Option<String>,
    cache_root: PathBuf,
    capabilities: RenderCapabilities,
    probes: BTreeMap<String, Probe>,
    jobs: BTreeMap<ProbeJob, Job>,
    next_job: u64,
    scene_revision: u64,
    version: u64,
    /// Request bakes for probes without a valid capture (Play: no host drives jobs).
    auto: bool,
    io: std::sync::mpsc::Sender<Io>,
    landed: std::sync::mpsc::Receiver<Io>,
}

/// One probe of the frame and its capture lifecycle.
#[derive(Debug, Clone)]
struct Probe {
    entity: u64,
    enabled: bool,
    invalid: Option<String>,
    /// Portable hash of every capture input now.
    source_hash: String,
    /// Latest request generation.
    generation: u64,
    /// The generation in flight, if any.
    flight: Option<Flight>,
    /// The published capture.
    receipt: Option<ProbeReceipt>,
    /// How the last unpublished generation ended (cancelled or failed).
    ended: Option<(ProbeState, Vec<Diagnostic>)>,
    /// A cache load is in flight or done for `source_hash` (one attempt per input).
    loaded: Option<String>,
    /// That cache read has not come back yet.
    loading: bool,
}

/// A generation between request and receipt.
#[derive(Debug, Clone)]
struct Flight {
    generation: u64,
    source_hash: String,
    source_revision: u64,
    state: ProbeState,
    stage: Option<ProbeStage>,
    completed: u32,
    /// The renderer accepted the bake; its column's bake count then.
    requested: Option<u64>,
    /// A cancel is asked for.
    cancel: bool,
    /// Published and read back: hashing and the cache write are under way.
    writing: bool,
    /// Frames the renderer has not held a record for it yet.
    misses: u32,
}

/// A recapture job: its members (probe, generation) and their outcomes.
#[derive(Debug, Clone, Default)]
struct Job {
    members: Vec<(String, u64)>,
    why: String,
    receipts: Vec<ProbeReceipt>,
    failures: Vec<ProbeFailure>,
}

/// Off-thread cache work and its results.
#[derive(Debug)]
enum Io {
    /// A published capture hashed and written (or not).
    Written {
        entity: String,
        generation: u64,
        receipt: Result<ProbeReceipt, String>,
    },
    /// A cached capture read for `source_hash` (or missing or unreadable).
    Loaded {
        entity: String,
        source_hash: String,
        found: Result<Option<(ara_render::look::captures::CaptureImage, ProbeReceipt)>, String>,
    },
}

impl Default for ProbeCaptures {
    fn default() -> Self {
        let (io, landed) = std::sync::mpsc::channel();
        Self {
            doc: None,
            cache_root: PathBuf::new(),
            capabilities: RenderCapabilities::default(),
            probes: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_job: 1,
            scene_revision: 0,
            version: 0,
            auto: false,
            io,
            landed,
        }
    }
}

impl ProbeCaptures {
    /// A controller whose captures cache under `cache_root` (the project's
    /// `.ara/cache/probes`).
    pub fn new(cache_root: PathBuf) -> Self {
        Self {
            cache_root,
            ..Default::default()
        }
    }

    /// Binds the scene this view renders: its canonical document ID and the project's
    /// probe cache folder. Requests for another document fail with `ARA_PROBE_MISSING`.
    pub fn set_scene(&mut self, doc: impl Into<String>, cache_root: PathBuf) {
        let doc = doc.into();
        if self.doc.as_deref() != Some(doc.as_str()) || self.cache_root != cache_root {
            self.doc = Some(doc);
            self.cache_root = cache_root;
            for probe in self.probes.values_mut() {
                probe.loaded = None;
            }
            self.version += 1;
        }
    }

    /// Captures probes without a valid published or cached capture automatically,
    /// as Play does (no host drives jobs there). Off by default: editors request.
    pub fn set_auto_capture(&mut self, auto: bool) {
        self.auto = auto;
    }

    /// Whether capture work is under way (a generation in flight, a cache read or
    /// write): a view that otherwise stops redrawing keeps preparing frames
    /// (`RasterView::needs_frame`) until it settles.
    pub fn is_busy(&self) -> bool {
        self.probes
            .values()
            .any(|p| p.flight.is_some() || p.loading)
    }

    /// The probe cache folder.
    pub fn cache_root(&self) -> &std::path::Path {
        &self.cache_root
    }

    /// Resolves and validates `request` against `frame` (the retained frame this
    /// view renders) and queues one job for its valid probes. Never waits on the GPU
    /// or allocates GPU resources; capture work runs in the view's frame path. Fails
    /// without a job when the scene is unbound, the renderer cannot shade probes, or
    /// no requested probe is valid. A probe already in flight for identical inputs
    /// joins that generation (coalesced); one in flight for older inputs is
    /// superseded by a new generation.
    pub fn recapture(
        &mut self,
        frame: &RenderFrame,
        request: &RecaptureRequest,
    ) -> Result<ProbeJob, Vec<Diagnostic>> {
        let Some(doc) = self.doc.clone() else {
            return Err(vec![Diagnostic::error(
                codes::PROBE_MISSING,
                "no scene is bound to this view (ProbeCaptures::set_scene)",
            )]);
        };
        if let Some(reason) = self.unsupported() {
            return Err(vec![Diagnostic::error(codes::PROBE_UNSUPPORTED, reason)]);
        }
        let mut failures = Vec::new();
        let mut valid = Vec::new();
        for key in request.probes {
            let problem = if key.doc != doc {
                Some((
                    codes::PROBE_MISSING,
                    format!("`{}` is not the scene this view renders (`{doc}`)", key.doc),
                ))
            } else if let Some(view) = frame
                .reflection_probes
                .iter()
                .find(|v| v.address == key.entity)
            {
                view.probe.invalid().map(|why| {
                    let code = if why.starts_with("captureResolution") {
                        codes::PROBE_RESOLUTION
                    } else {
                        codes::PROBE_INVALID
                    };
                    (code, why)
                })
            } else {
                Some((
                    codes::PROBE_MISSING,
                    format!("`{}` has no live ReflectionProbe in `{doc}`", key.entity),
                ))
            };
            match problem {
                Some((code, message)) => failures.push(ProbeFailure {
                    doc: key.doc.clone(),
                    entity: key.entity.clone(),
                    state: ProbeState::Failed,
                    diagnostics: vec![Diagnostic::error(code, message).file(key.doc.clone())],
                }),
                None if !valid.contains(&key.entity) => valid.push(key.entity.clone()),
                None => {}
            }
        }
        if valid.is_empty() {
            return Err(failures.into_iter().flat_map(|f| f.diagnostics).collect());
        }
        let job = ProbeJob(self.next_job);
        self.next_job += 1;
        let mut members = Vec::new();
        let mut superseded = Vec::new();
        for entity in valid {
            // A probe not observed yet (the request raced the frame) adopts its inputs
            // at its first tick (an empty source hash).
            let probe = self
                .probes
                .entry(entity.clone())
                .or_insert_with(|| Probe::pending(&entity));
            let coalesce = probe
                .flight
                .as_ref()
                .filter(|f| {
                    !f.cancel && (f.source_hash == probe.source_hash || f.source_hash.is_empty())
                })
                .map(|f| f.generation);
            let generation = match coalesce {
                Some(generation) => generation,
                None => {
                    if let Some(old) = &probe.flight {
                        superseded.push((entity.clone(), old.generation));
                    }
                    probe.generation += 1;
                    probe.ended = None;
                    probe.flight = Some(Flight::new(
                        probe.generation,
                        &probe.source_hash,
                        self.scene_revision,
                    ));
                    probe.generation
                }
            };
            members.push((entity, generation));
        }
        for (entity, generation) in superseded {
            self.end_superseded(&entity, generation);
        }
        self.jobs.insert(
            job,
            Job {
                members,
                why: request.why.unwrap_or("recapture").to_owned(),
                receipts: Vec::new(),
                failures,
            },
        );
        self.version += 1;
        Ok(job)
    }

    /// Requests cancellation of `job`'s in-flight members (a member shared with
    /// another job, coalesced, is cancelled for both); `None` for an unknown job.
    /// Publishing happens within one frame's work, so every in-flight member is
    /// cancellable between frames.
    pub fn cancel(&mut self, job: ProbeJob) -> Option<CancelOutcome> {
        let members = self.jobs.get(&job)?.members.clone();
        let mut outcome = CancelOutcome {
            cancel_requested: false,
            cancellable: false,
        };
        for (entity, generation) in members {
            if let Some(flight) = self
                .probes
                .get_mut(&entity)
                .and_then(|p| p.flight.as_mut())
                .filter(|f| f.generation == generation && !f.writing)
            {
                flight.cancel = true;
                outcome.cancel_requested = true;
                outcome.cancellable = true;
            }
        }
        if outcome.cancel_requested {
            self.version += 1;
        }
        Some(outcome)
    }

    /// `job`'s status; `None` for an unknown job.
    pub fn job(&self, job: ProbeJob) -> Option<ProbeJobStatus> {
        let record = self.jobs.get(&job)?;
        let (mut work, mut total, mut open, mut cancellable) = (0u32, 0u32, 0usize, false);
        for (entity, generation) in &record.members {
            let flight = self
                .probes
                .get(entity)
                .and_then(|p| p.flight.as_ref())
                .filter(|f| f.generation == *generation);
            if let Some(f) = flight {
                open += 1;
                work += f.completed;
                total += STEPS;
                cancellable |= !f.writing;
            } else {
                work += STEPS;
                total += STEPS;
            }
        }
        let done = record.receipts.len();
        let failed = record.failures.len();
        let finished = open == 0;
        Some(ProbeJobStatus {
            job,
            finished,
            progress: (total > 0).then(|| work as f32 / total as f32),
            message: if finished {
                format!("{}: {done} captured, {failed} not", record.why)
            } else {
                format!(
                    "{}: {open} of {} probes capturing",
                    record.why,
                    record.members.len()
                )
            },
            cancellable,
            receipts: record.receipts.clone(),
            failures: record.failures.clone(),
            diagnostics: Vec::new(),
        })
    }

    /// The rows matching `filter`, with the current capabilities. Rows are empty when
    /// `since` is at least the current version.
    pub fn snapshot(&self, filter: &ProbeFilter, since: Option<u64>) -> ProbeSnapshot {
        let mut capabilities = self.capabilities.clone();
        if let Some(reason) = self.unsupported() {
            capabilities.reflection_probes = false;
            capabilities
                .reasons
                .insert("reflectionProbes".into(), reason);
        }
        let unchanged = since.is_some_and(|s| s >= self.version);
        let doc = self.doc.clone().unwrap_or_default();
        let probes = if unchanged || filter.doc.is_some_and(|d| d != doc) {
            Vec::new()
        } else {
            self.probes
                .iter()
                .filter(|(entity, p)| {
                    p.entity != 0 && filter.entities.is_none_or(|e| e.contains(entity))
                })
                .map(|(entity, p)| p.row(&doc, entity, self.scene_revision))
                .collect()
        };
        ProbeSnapshot {
            version: self.version,
            scene_revision: self.scene_revision,
            capabilities,
            probes,
        }
    }

    /// Advances capture work from the view's frame path (`RasterView`): refreshes
    /// capabilities and probes from `frame`, drives the renderer's bakes for requested
    /// generations, discards stale ones, hands published images to the cache worker
    /// and restores cached captures. Bounded: no waits, no file IO on this thread.
    pub(crate) fn tick(
        &mut self,
        renderer: &mut ara_render::Renderer,
        frame: &RenderFrame,
        scene_revision: u64,
    ) {
        let capabilities = renderer.capabilities();
        let mut changed =
            capabilities != self.capabilities || scene_revision != self.scene_revision;
        self.capabilities = capabilities;
        self.scene_revision = scene_revision;
        let inputs = renderer.reflection_capture_inputs();
        // Probes of this frame, with their current input hashes.
        let mut seen = std::collections::BTreeSet::new();
        for view in &frame.reflection_probes {
            seen.insert(view.address.clone());
            let source_hash = source_hash(&inputs, view);
            let probe = self
                .probes
                .entry(view.address.clone())
                .or_insert_with(|| Probe::pending(&view.address));
            let invalid = view.probe.invalid();
            if probe.entity != view.entity
                || probe.enabled != view.probe.enabled
                || probe.invalid != invalid
                || probe.source_hash != source_hash
            {
                probe.entity = view.entity;
                probe.enabled = view.probe.enabled;
                probe.invalid = invalid;
                probe.source_hash = source_hash;
                changed = true;
            }
            if let Some(flight) = probe.flight.as_mut().filter(|f| f.source_hash.is_empty()) {
                flight.source_hash = probe.source_hash.clone();
            }
        }
        // Probes gone from the frame end their flights as stale.
        let gone: Vec<String> = self
            .probes
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        for entity in gone {
            if let Some(probe) = self.probes.remove(&entity)
                && let Some(flight) = probe.flight
            {
                self.end(
                    &entity,
                    flight.generation,
                    ProbeState::Cancelled,
                    codes::PROBE_STALE,
                    "the probe was deleted before its capture finished",
                );
            }
            changed = true;
        }
        let entities: Vec<String> = self.probes.keys().cloned().collect();
        for entity in entities {
            changed |= self.advance(renderer, &entity);
        }
        // Published images: hash and cache them off this thread.
        for image in renderer.take_reflection_capture_images() {
            let Some((entity, probe)) = self.probes.iter_mut().find(|(_, p)| p.entity == image.key)
            else {
                continue;
            };
            let Some(flight) = probe
                .flight
                .as_mut()
                .filter(|f| f.requested.is_some_and(|b| image.bake > b) && !f.writing)
            else {
                continue;
            };
            flight.writing = true;
            flight.state = ProbeState::Progress;
            flight.stage = Some(ProbeStage::Publish);
            flight.completed = STEPS;
            let (doc, entity, generation) = (
                self.doc.clone().unwrap_or_default(),
                entity.clone(),
                flight.generation,
            );
            let (root, source_hash, source_revision) = (
                self.cache_root.clone(),
                flight.source_hash.clone(),
                flight.source_revision,
            );
            let sender = self.io.clone();
            std::thread::spawn(move || {
                let receipt = write_cache(
                    &root,
                    &doc,
                    &entity,
                    generation,
                    &source_hash,
                    source_revision,
                    &image,
                );
                let _ = sender.send(Io::Written {
                    entity,
                    generation,
                    receipt,
                });
            });
            changed = true;
        }
        while let Ok(io) = self.landed.try_recv() {
            changed = true;
            match io {
                Io::Written {
                    entity,
                    generation,
                    receipt,
                } => self.written(&entity, generation, receipt),
                Io::Loaded {
                    entity,
                    source_hash,
                    found,
                } => self.restore(renderer, &entity, &source_hash, found),
            }
        }
        if changed {
            self.version += 1;
        }
    }

    /// Advances one probe's flight (or starts a cache load); true on any change.
    fn advance(&mut self, renderer: &mut ara_render::Renderer, entity: &str) -> bool {
        let auto = self.auto;
        let root = self.cache_root.clone();
        let doc = self.doc.clone();
        let Some(probe) = self.probes.get_mut(entity) else {
            return false;
        };
        let status = renderer.reflection_capture_status(probe.entity);
        // Restore a cached capture for the current inputs, once per input.
        if probe.flight.is_none()
            && doc.is_some()
            && probe.enabled
            && status.is_some_and(|s| !s.ready)
            && probe.loaded.as_deref() != Some(probe.source_hash.as_str())
        {
            probe.loaded = Some(probe.source_hash.clone());
            probe.loading = true;
            let (sender, entity, source_hash) = (
                self.io.clone(),
                entity.to_owned(),
                probe.source_hash.clone(),
            );
            std::thread::spawn(move || {
                let found = read_cache(&root, &source_hash);
                let _ = sender.send(Io::Loaded {
                    entity,
                    source_hash,
                    found,
                });
            });
            return true;
        }
        // Play captures what has no capture once its cache lookup came back empty.
        if auto
            && probe.flight.is_none()
            && probe.enabled
            && probe.receipt.is_none()
            && !probe.loading
            && probe.loaded.as_deref() == Some(probe.source_hash.as_str())
            && status.is_some_and(|s| !s.ready && !s.pending)
        {
            probe.generation += 1;
            probe.flight = Some(Flight::new(
                probe.generation,
                &probe.source_hash,
                self.scene_revision,
            ));
        }
        let Some(flight) = probe.flight.as_mut() else {
            return false;
        };
        if flight.writing {
            return false;
        }
        let key = probe.entity;
        let before = (flight.state, flight.stage, flight.completed);
        let (generation, stale) = (
            flight.generation,
            !flight.source_hash.is_empty() && flight.source_hash != probe.source_hash,
        );
        if flight.cancel || stale || !probe.enabled {
            renderer.cancel_reflection_capture(key);
            let (code, why) = if flight.cancel {
                (codes::PROBE_CANCELLED, "cancelled before it published")
            } else if stale {
                (
                    codes::PROBE_STALE,
                    "the probe's inputs changed before its capture finished",
                )
            } else {
                (
                    codes::PROBE_CANCELLED,
                    "the probe was disabled before its capture finished",
                )
            };
            self.end(entity, generation, ProbeState::Cancelled, code, why);
            return true;
        }
        match (flight.requested, status) {
            // The renderer refits its records while rendering: allow a few frames.
            (_, None) if flight.misses < RECORD_FRAMES => flight.misses += 1,
            (_, None) => {
                self.end(entity, generation, ProbeState::Failed, codes::PROBE_LIMIT, "the renderer holds no record for this probe (more probes than the preset captures)");
                return true;
            }
            (None, Some(s)) => {
                // A fresh bake: none started under older inputs continues into this one.
                renderer.cancel_reflection_capture(key);
                if renderer.request_reflection_capture(key) {
                    flight.requested = Some(s.bakes);
                    flight.state = ProbeState::Queued;
                }
            }
            (Some(_), Some(s)) => match s.step {
                Some(0) => {
                    flight.state = ProbeState::Capturing;
                    flight.stage = Some(ProbeStage::Faces);
                    flight.completed = 0;
                }
                Some(step) if step < 6 => {
                    flight.state = ProbeState::Progress;
                    flight.stage = Some(ProbeStage::Faces);
                    flight.completed = step;
                }
                Some(step) => {
                    flight.state = ProbeState::Progress;
                    flight.stage = Some(ProbeStage::Filter);
                    flight.completed = step;
                }
                None if s.pending => flight.state = ProbeState::Queued,
                None => {}
            },
        }
        before != (flight.state, flight.stage, flight.completed)
    }

    /// Ends `entity`'s flight `generation` without publishing.
    fn end(
        &mut self,
        entity: &str,
        generation: u64,
        state: ProbeState,
        code: ara_diag::Code,
        why: &str,
    ) {
        let doc = self.doc.clone().unwrap_or_default();
        let diagnostics = vec![Diagnostic::warning(code, why).file(doc.clone())];
        if let Some(probe) = self.probes.get_mut(entity)
            && probe
                .flight
                .as_ref()
                .is_some_and(|f| f.generation == generation)
        {
            probe.flight = None;
            probe.ended = Some((state, diagnostics.clone()));
        }
        for job in self.jobs.values_mut() {
            if job
                .members
                .iter()
                .any(|(e, g)| e == entity && *g == generation)
            {
                job.failures.push(ProbeFailure {
                    doc: doc.clone(),
                    entity: entity.to_owned(),
                    state,
                    diagnostics: diagnostics.clone(),
                });
            }
        }
    }

    /// Ends a generation a newer request superseded (`recapture`).
    fn end_superseded(&mut self, entity: &str, generation: u64) {
        let doc = self.doc.clone().unwrap_or_default();
        let diagnostics = vec![
            Diagnostic::warning(codes::PROBE_CANCELLED, "superseded by a newer request")
                .file(doc.clone()),
        ];
        for job in self.jobs.values_mut() {
            if job
                .members
                .iter()
                .any(|(e, g)| e == entity && *g == generation)
            {
                job.failures.push(ProbeFailure {
                    doc: doc.clone(),
                    entity: entity.to_owned(),
                    state: ProbeState::Cancelled,
                    diagnostics: diagnostics.clone(),
                });
            }
        }
    }

    /// A cache write finished: the generation's receipt, or its failure.
    fn written(&mut self, entity: &str, generation: u64, receipt: Result<ProbeReceipt, String>) {
        match receipt {
            Ok(receipt) => {
                if let Some(probe) = self.probes.get_mut(entity)
                    && probe
                        .flight
                        .as_ref()
                        .is_some_and(|f| f.generation == generation)
                {
                    probe.flight = None;
                    probe.ended = None;
                    probe.loaded = Some(receipt.source_hash.clone());
                    probe.receipt = Some(receipt.clone());
                }
                for job in self.jobs.values_mut() {
                    if job
                        .members
                        .iter()
                        .any(|(e, g)| e == entity && *g == generation)
                    {
                        job.receipts.push(receipt.clone());
                    }
                }
            }
            Err(why) => self.end(
                entity,
                generation,
                ProbeState::Failed,
                codes::PROBE_CACHE,
                &why,
            ),
        }
    }

    /// A cache read came back: restore its image and receipt if still current.
    fn restore(
        &mut self,
        renderer: &mut ara_render::Renderer,
        entity: &str,
        source_hash: &str,
        found: Result<Option<(ara_render::look::captures::CaptureImage, ProbeReceipt)>, String>,
    ) {
        let Some(probe) = self.probes.get_mut(entity) else {
            return;
        };
        probe.loading = false;
        if probe.source_hash != source_hash || probe.flight.is_some() {
            return;
        }
        match found {
            Ok(Some((mut image, receipt))) => {
                image.key = probe.entity;
                match renderer.load_reflection_capture(image) {
                    Ok(()) => probe.receipt = Some(receipt),
                    Err(e) => {
                        probe.ended = Some((
                            ProbeState::Failed,
                            vec![Diagnostic::warning(codes::PROBE_CACHE, format!("{e:#}"))],
                        ))
                    }
                }
            }
            Ok(None) => {}
            Err(why) => {
                probe.ended = Some((
                    ProbeState::Failed,
                    vec![Diagnostic::warning(codes::PROBE_CACHE, why)],
                ))
            }
        }
    }

    /// Retires this controller's work without rendering (a replaced scene): applies
    /// pending cancels to the renderer's bake planner (CPU state only), ends those
    /// generations as Cancelled, and drains finished cache IO. A generation already
    /// published but not yet read back cannot complete without the renderer's next
    /// primary frame: it ends Cancelled (ARA_PROBE_STALE). Never records GPU work,
    /// waits or touches files on this thread. True while a cache write is in flight.
    pub fn retire(&mut self, renderer: &mut ara_render::Renderer) -> bool {
        let mut changed = false;
        let entities: Vec<String> = self.probes.keys().cloned().collect();
        for entity in entities {
            let Some(probe) = self.probes.get(&entity) else {
                continue;
            };
            let Some(flight) = probe.flight.clone() else {
                continue;
            };
            if flight.writing {
                continue;
            }
            renderer.cancel_reflection_capture(probe.entity);
            let (code, why) = if flight.cancel {
                (codes::PROBE_CANCELLED, "cancelled before it published")
            } else {
                (
                    codes::PROBE_STALE,
                    "the scene was replaced before its capture was read back",
                )
            };
            self.end(&entity, flight.generation, ProbeState::Cancelled, code, why);
            changed = true;
        }
        while let Ok(io) = self.landed.try_recv() {
            changed = true;
            match io {
                Io::Written {
                    entity,
                    generation,
                    receipt,
                } => self.written(&entity, generation, receipt),
                Io::Loaded { .. } => {}
            }
        }
        if changed {
            self.version += 1;
        }
        self.probes
            .values()
            .any(|p| p.flight.as_ref().is_some_and(|f| f.writing))
    }

    /// Why captures cannot run now, or `None`.
    fn unsupported(&self) -> Option<String> {
        (!self.capabilities.reflection_probes).then(|| {
            self.capabilities
                .reasons
                .get("reflectionProbes")
                .cloned()
                .unwrap_or_else(|| "reflection probes are unavailable".to_owned())
        })
    }
}

/// Bake steps per capture (six faces, then the prefilter) and publishing.
const STEPS: u32 = 7;
/// Frames a requested probe may wait for the renderer to hold its record (specs
/// reach the bake planner during the next render) before `ARA_PROBE_LIMIT`.
const RECORD_FRAMES: u32 = 8;

impl Probe {
    fn pending(_entity: &str) -> Self {
        Self {
            entity: 0,
            enabled: true,
            invalid: None,
            source_hash: String::new(),
            generation: 0,
            flight: None,
            receipt: None,
            ended: None,
            loaded: None,
            loading: false,
        }
    }

    fn row(&self, doc: &str, entity: &str, scene_revision: u64) -> ProbeRow {
        let (state, stage, completed, cancellable) =
            match (&self.flight, &self.ended, &self.receipt) {
                (Some(f), _, _) => (f.state, f.stage, Some(f.completed), !f.writing),
                (None, Some((state, _)), _) => (*state, None, None, false),
                (None, None, Some(_)) => (ProbeState::Done, None, None, false),
                (None, None, None) => (ProbeState::Idle, None, None, false),
            };
        let mut diagnostics: Vec<Diagnostic> = self
            .invalid
            .iter()
            .map(|why| Diagnostic::error(codes::PROBE_INVALID, why.clone()).file(doc.to_owned()))
            .collect();
        if let Some((_, ended)) = &self.ended {
            diagnostics.extend(ended.iter().cloned());
        }
        ProbeRow {
            doc: doc.to_owned(),
            entity: entity.to_owned(),
            enabled: self.enabled,
            generation: self.generation,
            state,
            stage,
            progress: completed.map(|c| c as f32 / STEPS as f32),
            completed_work: completed,
            total_work: completed.map(|_| STEPS),
            cancellable,
            scene_revision,
            source_hash: self.source_hash.clone(),
            stale: self
                .receipt
                .as_ref()
                .is_some_and(|r| r.source_hash != self.source_hash),
            retained_previous: self.flight.is_some() && self.receipt.is_some(),
            last_receipt: self.receipt.clone(),
            diagnostics,
        }
    }
}

impl Flight {
    fn new(generation: u64, source_hash: &str, source_revision: u64) -> Self {
        Self {
            generation,
            source_hash: source_hash.to_owned(),
            source_revision,
            state: ProbeState::Queued,
            stage: None,
            completed: 0,
            requested: None,
            cancel: false,
            writing: false,
            misses: 0,
        }
    }
}

/// The portable hash of a probe's capture inputs: the renderer's scene inputs
/// (`Renderer::reflection_capture_inputs`) plus the probe's settings and pose.
fn source_hash(inputs: &str, view: &ara_runtime::extract::ProbeView) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(inputs.as_bytes());
    hash.update(view.address.as_bytes());
    hash.update(format!("{:?}", view.probe).as_bytes());
    for v in view
        .translation
        .iter()
        .chain(&view.rotation)
        .chain(&view.scale)
    {
        hash.update(v.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

/// Hashes a published image and writes it with its receipt under `root` as
/// `<sourceHash>.capture`: a JSON receipt line, then the image bytes.
fn write_cache(
    root: &std::path::Path,
    doc: &str,
    entity: &str,
    generation: u64,
    source_hash: &str,
    source_revision: u64,
    image: &ara_render::look::captures::CaptureImage,
) -> Result<ProbeReceipt, String> {
    use sha2::{Digest, Sha256};
    let capture_hash = format!("{:x}", Sha256::digest(&image.bytes));
    let file = format!("{source_hash}.capture");
    let receipt = ProbeReceipt {
        doc: doc.to_owned(),
        entity: entity.to_owned(),
        generation,
        source_revision,
        source_hash: source_hash.to_owned(),
        capture_hash,
        resolution: image.resolution,
        faces: 6,
        cache_path: format!(".ara/cache/probes/{file}"),
        published: true,
    };
    let header =
        serde_json::json!({"version": CACHE_VERSION, "extent": image.extent, "receipt": receipt});
    let mut bytes = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    bytes.extend_from_slice(&image.bytes);
    std::fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let path = root.join(&file);
    let partial = root.join(format!("{file}.partial"));
    std::fs::write(&partial, &bytes)
        .map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path)
        .map_err(|e| format!("cannot publish {}: {e}", path.display()))?;
    Ok(receipt)
}

/// Reads `<sourceHash>.capture` under `root`: `None` when absent.
fn read_cache(
    root: &std::path::Path,
    source_hash: &str,
) -> Result<Option<(ara_render::look::captures::CaptureImage, ProbeReceipt)>, String> {
    use sha2::{Digest, Sha256};
    let path = root.join(format!("{source_hash}.capture"));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let split = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| format!("{} has no header", path.display()))?;
    let header: serde_json::Value =
        serde_json::from_slice(&bytes[..split]).map_err(|e| format!("{}: {e}", path.display()))?;
    let receipt: ProbeReceipt = serde_json::from_value(header["receipt"].clone())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let extent: [u32; 2] = serde_json::from_value(header["extent"].clone())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let image = bytes[split + 1..].to_vec();
    if header["version"] != CACHE_VERSION
        || format!("{:x}", Sha256::digest(&image)) != receipt.capture_hash
    {
        return Err(format!(
            "{} is from another version or damaged",
            path.display()
        ));
    }
    Ok(Some((
        ara_render::look::captures::CaptureImage {
            key: 0,
            bake: 0,
            resolution: receipt.resolution,
            extent,
            bytes: image,
        },
        receipt,
    )))
}
