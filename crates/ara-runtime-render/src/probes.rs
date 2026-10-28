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

/// Whether recapture jobs run in this build. Until they do, capabilities report
/// `reflectionProbes: false` with this reason and requests fail with
/// `ARA_PROBE_UNSUPPORTED`, so no host shows a job that cannot run.
const JOBS: Option<&str> = Some("probe recapture jobs are not in this build yet");

/// The retained probe capture state of one view (see the module docs).
#[derive(Debug, Default)]
pub struct ProbeCaptures {
    doc: Option<String>,
    cache_root: PathBuf,
    capabilities: RenderCapabilities,
    probes: BTreeMap<String, Probe>,
    scene_revision: u64,
    version: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct Probe {
    enabled: bool,
    invalid: Option<String>,
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
            self.version += 1;
        }
    }

    /// The probe cache folder.
    pub fn cache_root(&self) -> &std::path::Path {
        &self.cache_root
    }

    /// Resolves and validates `request` against `frame` (the retained frame this
    /// view renders) and queues one job for all of its probes. Never waits on the GPU
    /// or allocates; capture work runs in the view's frame path. Fails without a job
    /// when the scene is unbound, the renderer cannot shade probes, or no requested
    /// probe is valid; invalid members of a valid request become job failures.
    pub fn recapture(
        &mut self,
        frame: &RenderFrame,
        request: &RecaptureRequest,
    ) -> Result<ProbeJob, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let Some(doc) = self.doc.clone() else {
            return Err(vec![Diagnostic::error(
                codes::PROBE_MISSING,
                "no scene is bound to this view (ProbeCaptures::set_scene)",
            )]);
        };
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
            if let Some((code, message)) = problem {
                diagnostics.push(Diagnostic::error(code, message).file(key.doc.clone()));
            }
        }
        if let Some(reason) = self.unsupported() {
            diagnostics.push(Diagnostic::error(codes::PROBE_UNSUPPORTED, reason));
        }
        Err(diagnostics)
    }

    /// Requests cancellation of `job`'s in-flight members; `None` for an unknown job.
    pub fn cancel(&mut self, _job: ProbeJob) -> Option<CancelOutcome> {
        None
    }

    /// `job`'s status; `None` for an unknown job.
    pub fn job(&self, _job: ProbeJob) -> Option<ProbeJobStatus> {
        None
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
                .filter(|(entity, _)| filter.entities.is_none_or(|e| e.contains(entity)))
                .map(|(entity, probe)| ProbeRow {
                    doc: doc.clone(),
                    entity: entity.clone(),
                    enabled: probe.enabled,
                    generation: 0,
                    state: ProbeState::Idle,
                    stage: None,
                    progress: None,
                    completed_work: None,
                    total_work: None,
                    cancellable: false,
                    scene_revision: self.scene_revision,
                    source_hash: String::new(),
                    stale: false,
                    retained_previous: false,
                    last_receipt: None,
                    diagnostics: probe
                        .invalid
                        .iter()
                        .map(|why| {
                            Diagnostic::error(codes::PROBE_INVALID, why.clone()).file(doc.clone())
                        })
                        .collect(),
                })
                .collect()
        };
        ProbeSnapshot {
            version: self.version,
            scene_revision: self.scene_revision,
            capabilities,
            probes,
        }
    }

    /// Refreshes capabilities and the probe list from the frame the view rendered
    /// (`RasterView`'s frame path), bumping the version on any change.
    pub(crate) fn observe(
        &mut self,
        capabilities: RenderCapabilities,
        frame: &RenderFrame,
        scene_revision: u64,
    ) {
        let probes: BTreeMap<String, Probe> = frame
            .reflection_probes
            .iter()
            .map(|v| {
                (
                    v.address.clone(),
                    Probe {
                        enabled: v.probe.enabled,
                        invalid: v.probe.invalid(),
                    },
                )
            })
            .collect();
        if capabilities != self.capabilities
            || probes != self.probes
            || scene_revision != self.scene_revision
        {
            self.capabilities = capabilities;
            self.probes = probes;
            self.scene_revision = scene_revision;
            self.version += 1;
        }
    }

    /// Why captures cannot run now, or `None`.
    fn unsupported(&self) -> Option<String> {
        if let Some(reason) = JOBS {
            return Some(reason.to_owned());
        }
        (!self.capabilities.reflection_probes).then(|| {
            self.capabilities
                .reasons
                .get("reflectionProbes")
                .cloned()
                .unwrap_or_else(|| "reflection probes are unavailable".to_owned())
        })
    }
}
