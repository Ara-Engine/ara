//! In-process authority and the frame-boundary policy shared by native input and network clients.
use crate::{
    actions::{Control, EditorService},
    capture::{CaptureCompletion, CaptureService, CaptureTask},
    state::{ApplyPolicy, EditorState, Navigation},
    world::{DocumentView, gesture::Gesture},
};
use ara_host::{Host, HostConfig};
use ara_proto::{ConnId, Endpoint, RpcError};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

/// A structured protocol failure displayed by the native editor.
#[derive(Debug)]
pub struct EditorError(pub RpcError);
impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for EditorError {}
impl From<RpcError> for EditorError {
    fn from(error: RpcError) -> Self {
        Self(error)
    }
}

/// One editor process owns the project lock, authority, navigation and review policy.
pub struct Editor {
    asset_updates: Receiver<()>,
    /// Shared engine host. UI content mutations go through this protocol endpoint.
    pub host: Host,
    /// Current native view snapshot, updated by editor action notifications.
    pub state: EditorState,
    /// Human session owned by this editor window.
    pub session: String,
    /// Active provisional spatial edit.
    pub gesture: Option<Gesture>,
    /// Frozen trace selected for frame inspection; immutable and shared only with IO workers.
    pub profile_current: Option<std::sync::Arc<ara_profile::Profile>>,
    /// Independent baseline trace for the compare view.
    pub profile_baseline: Option<std::sync::Arc<ara_profile::Profile>>,
    /// Deferred capture jobs, consumed by the GPU/window owner.
    pub(crate) captures: Receiver<CaptureTask>,
    /// GPU job results returned to the capture service.
    pub(crate) completed: mpsc::Sender<CaptureCompletion>,
    updates: Receiver<EditorState>,
    controls: Receiver<Control>,
    pending: VecDeque<Pending>,
    last_input: Instant,
    pending_sequence: u64,
    pub(crate) command_schemas: BTreeMap<String, Value>,
    pub(crate) view_revision: u64,
    pub(crate) panels_revision: u64,
    pub(crate) review_revision: u64,
    pub(crate) extraction_revision: u64,
}
struct Pending {
    id: String,
    conn: ConnId,
    params: Value,
    approved: bool,
    method: String,
}
impl Editor {
    /// Open the project, register editor commands/services and create the window's human session.
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        // One schema world: the editor validates exactly what the player runs.
        let mut project =
            ara_play_host::open_project(root).map_err(|d| anyhow::anyhow!(d.to_string()))?;
        let main_scene = project.main_scene();
        crate::asset_tools::prepare(&mut project)?;
        crate::authoring::register(&mut project.registry, &mut project.db);
        crate::commands::register(&mut project.registry, &mut project.db);
        let command_schemas = project
            .registry
            .infos()
            .filter_map(|info| {
                project
                    .db
                    .get(&info.params)
                    .map(|ty| (info.name.to_owned(), project.db.type_json_schema(ty)))
            })
            .collect();
        if let Some(d) = project.db.verify().into_iter().find(|d| d.is_error()) {
            return Err(anyhow::anyhow!(d.to_string()));
        }
        let mut host = Host::open(project, HostConfig::default())
            .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        let (asset_changed, asset_updates) = mpsc::channel();
        host.register(Box::new(crate::asset_tools::AssetService::new(
            asset_changed,
        )))
        .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        host.register(Box::new(crate::authoring::AuthoringService))
            .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        *host.profiler() = ara_profile::Profiler::new(ara_profile::Config {
            budget_ns: 6_944_444,
            ..Default::default()
        })
        .map_err(anyhow::Error::msg)?;
        ara_audio_why::register(host.why());
        let workspace = host.store().workspace();
        let doc = main_scene
            .as_deref()
            .and_then(|path| workspace.find(path))
            .and_then(|id| workspace.doc(id.as_str()))
            .filter(|d| d.kind == "scene")
            .or_else(|| {
                workspace
                    .docs()
                    .find(|(_, d)| d.kind == "scene")
                    .map(|(_, d)| d)
            })
            .map(|d| d.id.to_string())
            .ok_or_else(|| anyhow::anyhow!("project has no scene; run ara init first"))?;
        let state = EditorState {
            doc,
            ..Default::default()
        };
        let navigation = Navigation::new(root, state)?;
        let state = navigation.state.clone();
        let (changes, updates) = mpsc::channel();
        let (control_send, controls) = mpsc::channel();
        host.register(Box::new(EditorService {
            navigation,
            changes,
            controls: control_send,
        }))
        .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        // Play runs in pooled ara-player processes; the toolbar binds play.* methods.
        let mut play = ara_play_host::PlayConfig::for_project(root);
        play.pool.headless = 1;
        host.register(Box::new(ara_play_host::PlayService::new(root, play)))
            .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        let (tasks, captures) = mpsc::channel();
        let (completed, results) = mpsc::channel();
        host.register(Box::new(CaptureService::new(tasks, results)))
            .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        let session = host
            .call(
                ConnId::LOCAL,
                "session.begin",
                json!({"client":"ara-editor","author":"human","label":"Editor"}),
            )
            .map_err(EditorError::from)?["session"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("session.begin returned no session"))?
            .to_owned();
        host.call(
            ConnId::LOCAL,
            "subscribe",
            json!({"session":session,"topics":["diagnostics","history","jobs"]}),
        )
        .map_err(EditorError::from)?;
        Ok(Self {
            asset_updates,
            host,
            state,
            session,
            gesture: None,
            profile_current: None,
            profile_baseline: None,
            captures,
            completed,
            updates,
            controls,
            pending: VecDeque::new(),
            last_input: Instant::now(),
            pending_sequence: 0,
            command_schemas,
            view_revision: 0,
            panels_revision: 0,
            review_revision: 0,
            extraction_revision: 0,
        })
    }
    /// Push native human activity into the non-disruption boundary.
    pub fn input_active(&mut self) {
        self.last_input = Instant::now();
    }
    /// A reflected editor action, using the same validation as an external client.
    pub fn action(&mut self, method: &str, mut params: Value) -> Result<Value, EditorError> {
        params["session"] = self.session.clone().into();
        self.call_for(ConnId::LOCAL, method, params)
            .map_err(Into::into)
    }
    /// Commit an atomic list of content commands as the window's human author.
    pub fn apply(
        &mut self,
        commands: Vec<Value>,
        why: &str,
        base: Option<u64>,
    ) -> Result<Value, EditorError> {
        self.call_for(ConnId::LOCAL,"cmd.apply",json!({"session":self.session,"doc":self.state.doc,"commands":commands,"why":why,"base":base})).map_err(Into::into)
    }
    /// Apply a diagnostic's commands to its document without changing the open scene.
    pub fn apply_to(
        &mut self,
        doc: &str,
        commands: Vec<Value>,
        why: &str,
    ) -> Result<Value, EditorError> {
        self.call_for(
            ConnId::LOCAL,
            "cmd.apply",
            json!({"session":self.session,"doc":doc,"commands":commands,"why":why}),
        )
        .map_err(Into::into)
    }
    /// Refresh navigation snapshots without cloning unchanged frame state.
    pub fn sync(&mut self) -> bool {
        let mut changed = false;
        for _ in self.asset_updates.try_iter().take(32) {
            self.extraction_revision += 1;
            changed = true;
        }
        while let Ok(state) = self.updates.try_recv() {
            if self.state.doc != state.doc
                || self.state.hidden != state.hidden
                || self.state.light != state.light
            {
                self.extraction_revision += 1;
            }
            if self.state.panels_changed(&state) {
                self.panels_revision += 1;
            }
            self.state = state;
            self.view_revision += 1;
            changed = true;
        }
        while let Ok(control) = self.controls.try_recv() {
            match control {
                Control::ProfileFreeze { baseline, job } => {
                    self.host.profiler().collect();
                    let mut captured = self.host.profiler().profile().clone();
                    captured.frames.retain(|frame| frame.duration_ns > 0);
                    let profile = std::sync::Arc::new(captured);
                    if baseline {
                        self.profile_baseline = Some(std::sync::Arc::clone(&profile));
                    } else {
                        self.profile_current = Some(std::sync::Arc::clone(&profile));
                    }
                    let output = self
                        .host
                        .root()
                        .join(".ara/cache/profiles")
                        .join(format!("{job}.araprofile"));
                    let completed = self.completed.clone();
                    std::thread::spawn(move || {
                        let result = (|| -> anyhow::Result<Value> {
                            std::fs::create_dir_all(
                                output
                                    .parent()
                                    .ok_or_else(|| anyhow::anyhow!("profile cache path"))?,
                            )?;
                            ara_profile::write_capture(&output, &profile)?;
                            Ok(json!({"path":output}))
                        })()
                        .map_err(|e| vec![crate::invalid(e.to_string())]);
                        let _ = completed.send(CaptureCompletion { job, result });
                    });
                }
                Control::Approve(ticket) => {
                    if let Some(p) = self.pending.iter_mut().find(|p| p.id == ticket) {
                        p.approved = true;
                    }
                }
                Control::Reject(ticket) => self.pending.retain(|p| p.id != ticket),
            }
            self.panels_revision += 1;
        }
        changed
    }
    /// Advance file watching, job completion and deferred agent batches at a frame boundary.
    pub fn tick(&mut self) {
        self.host.tick();
        self.sync();
        let idle =
            self.gesture.is_none() && self.last_input.elapsed() >= Duration::from_millis(350);
        let policy = self.state.policy;
        let count = self.pending.len();
        for _ in 0..count.min(8) {
            let Some(p) = self.pending.pop_front() else {
                break;
            };
            if p.approved
                || policy == ApplyPolicy::Immediately
                || (policy == ApplyPolicy::WhenIdle && idle)
            {
                if let Err(e) = self.host.call(p.conn, &p.method, p.params) {
                    eprintln!("deferred batch {}: {e}", p.id);
                }
            } else {
                self.pending.push_back(p);
            }
        }
    }
    /// Read-only current edit-world adapter.
    pub fn view(&self) -> anyhow::Result<DocumentView> {
        let mut view = DocumentView::new(self.host.store(), &self.state.doc).map_err(|d| {
            anyhow::anyhow!(
                d.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })?;
        let document = self
            .host
            .store()
            .workspace()
            .doc(&self.state.doc)
            .ok_or_else(|| anyhow::anyhow!("scene disappeared"))?;
        let mut deleted = ara_doc::Document::new("scene", document.id.clone());
        for op in self.unreviewed_ops() {
            if let ara_doc::Op::Delete { doc, id, snapshot } = op
                && doc.as_str() == self.state.doc
            {
                deleted.insert_unchecked(id.clone(), snapshot.clone());
            }
        }
        if !deleted.records().is_empty() {
            let mut diagnostics = Vec::new();
            let ghosts = ara_doc::prefab::resolve(
                &deleted,
                self.host.store().schema(),
                self.host.store().workspace(),
                &Default::default(),
                &mut diagnostics,
            );
            for (id, entity) in ghosts {
                let mut record = entity.record;
                if let Some(parent) = entity.parent {
                    record.insert("parent".into(), parent.into());
                }
                view.ghost(id, record);
            }
        }
        Ok(view)
    }
    /// Start a spatial gesture at the current revision.
    pub fn begin_gesture(&mut self) -> anyhow::Result<()> {
        let resolved = crate::world::resolve(self.host.store(), &self.state.doc)
            .map_err(|d| anyhow::anyhow!(format!("{d:?}")))?;
        self.gesture = Some(Gesture::begin(
            &resolved,
            &self.state,
            self.host.store().revision(),
        )?);
        Ok(())
    }
    /// Finish the preview as one content transaction, or discard it with no document change.
    pub fn finish_gesture(&mut self, cancel: bool) -> Result<Option<Value>, EditorError> {
        let Some(gesture) = self.gesture.take() else {
            return Ok(None);
        };
        if cancel {
            return Ok(None);
        }
        let commands = gesture.commands();
        if commands.is_empty() {
            return Ok(None);
        }
        self.apply(commands, "Transform gesture", Some(gesture.base))
            .map(Some)
    }
    /// Incoming calls pass through D20 before reaching the authority.
    pub fn call_for(
        &mut self,
        conn: ConnId,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if matches!(
            method,
            "cmd.apply"
                | "editor.resource.create"
                | "editor.asset.import"
                | "editor.animation.layout"
                | "history.undo"
                | "history.redo"
                | "review.revert"
                | "session.rollback"
                | "doc.sync"
                | "jobs.start"
                // Agent changes to a running game: live patches, C# commands, kept
                // changes and C# source writes (hot reloads) follow the same policy.
                | "play.patch"
                | "play.command"
                | "play.keep"
                | "script.write"
        ) && !params["dryRun"].as_bool().unwrap_or(false)
            && self.state.policy != ApplyPolicy::Immediately
        {
            let session = params["session"]
                .as_str()
                .ok_or_else(|| RpcError::from(crate::invalid("session required")))?;
            let status = self
                .host
                .call(conn, "session.status", json!({"session":session}))?;
            if status["session"]["author"]
                .as_str()
                .is_some_and(|a| a.starts_with("agent:"))
                && (self.state.policy == ApplyPolicy::OnApproval
                    || self.last_input.elapsed() < Duration::from_millis(350)
                    || self.gesture.is_some())
            {
                // Validate the entire batch now. Approval still uses its original base and attribution.
                if method == "cmd.apply" {
                    let mut dry = params.clone();
                    dry["dryRun"] = true.into();
                    self.host.call(conn, "cmd.apply", dry)?;
                }
                if let Some(key) = params["idempotencyKey"].as_str()
                    && let Some(p) = self.pending.iter().find(|p| {
                        p.method == method
                            && p.params["session"] == params["session"]
                            && p.params["idempotencyKey"].as_str() == Some(key)
                    })
                {
                    return Ok(json!({"queued":true,"ticket":p.id}));
                }
                self.pending_sequence += 1;
                let id = format!("pending-{}", self.pending_sequence);
                self.pending.push_back(Pending {
                    id: id.clone(),
                    conn,
                    params,
                    approved: false,
                    method: method.into(),
                });
                return Ok(json!({"queued":true,"ticket":id,"policy":self.state.policy}));
            }
        }
        if method.starts_with("editor.pending.")
            && !self
                .pending
                .iter()
                .any(|p| Some(p.id.as_str()) == params["ticket"].as_str())
        {
            return Err(RpcError::from(crate::invalid("pending batch not found")));
        }
        let result = self.host.call(conn, method, params);
        self.sync();
        result
    }
    /// Queued agent changes shown in the Agents panel, including their command list and reason.
    pub fn pending(&self) -> Value {
        json!(
            self.pending
                .iter()
                .map(
                    |p| json!({"ticket":p.id,"commands":p.params["commands"],"why":p.params["why"]})
                )
                .collect::<Vec<_>>()
        )
    }
    /// Human approval releases the original attributed batch at the next frame boundary.
    pub fn approve(&mut self, ticket: &str) -> Result<(), EditorError> {
        self.action("editor.pending.apply", json!({"ticket":ticket}))?;
        Ok(())
    }
    /// Reject a pending batch before it touches any document.
    pub fn reject(&mut self, ticket: &str) -> Result<(), EditorError> {
        self.action("editor.pending.reject", json!({"ticket":ticket}))?;
        Ok(())
    }
    pub(crate) fn report_error(&mut self, message: &str) {
        let _ = self.action("editor.report", json!({"message":message}));
    }
    /// Applied agent field preimages for provenance and review overlays.
    pub fn agent_fields(&self) -> BTreeMap<(String, String), Value> {
        let mut fields = BTreeMap::new();
        for op in self.unreviewed_ops() {
            if op.doc().as_str() != self.state.doc {
                continue;
            }
            match op {
                ara_doc::Op::Set { id, path, prev, .. } => {
                    let (entity, path) = match path.segments() {
                        [
                            ara_schema::Segment::Field(head),
                            ara_schema::Segment::Key(key),
                        ] if head == "overrides" => {
                            let (inner, path) = key.rsplit_once('/').unwrap_or(("", key));
                            (
                                if inner.is_empty() {
                                    id.to_string()
                                } else {
                                    format!("{id}/{inner}")
                                },
                                path.to_owned(),
                            )
                        }
                        _ => (id.to_string(), path.to_string()),
                    };
                    fields.insert((entity, path), prev.clone().unwrap_or(Value::Null));
                }
                ara_doc::Op::Create { id, record, .. } => {
                    for component in record.keys().filter(|k| ara_doc::is_component_key(k)) {
                        fields.insert((id.to_string(), component.clone()), Value::Null);
                    }
                }
                _ => {}
            }
        }
        fields
    }
    pub(crate) fn unreviewed_ops(&self) -> impl Iterator<Item = &ara_doc::Op> {
        self.host
            .store()
            .history()
            .iter()
            .filter_map(|t| self.host.review_entities(&t.id).map(|review| (t, review)))
            .flat_map(|(t, review)| {
                t.ops.iter().filter(move |op| {
                    review.get(&ara_host::review::entity_key(
                        op.doc().as_str(),
                        op.id().as_str(),
                    )) == Some(&ara_host::params::ReviewStatus::Applied)
                })
            })
    }
    pub(crate) fn review_tints(&self) -> BTreeMap<u64, &'static str> {
        self.unreviewed_ops()
            .filter(|op| op.doc().as_str() == self.state.doc)
            .map(|op| {
                (
                    crate::world::render_id(op.id().as_str()),
                    match op {
                        ara_doc::Op::Create { .. } => "success-foreground",
                        ara_doc::Op::Delete { .. } => "destructive-foreground",
                        _ => "warning-foreground",
                    },
                )
            })
            .collect()
    }
}
impl Endpoint for Editor {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_for(ConnId::LOCAL, method, params)
    }
}
