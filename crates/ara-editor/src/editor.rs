//! In-process authority shared by native input and network clients.
use crate::{
    actions::{Control, EditorService},
    capture::{CaptureCompletion, CaptureService, CaptureTask},
    state::{EditorState, Navigation},
    world::{DocumentView, gesture::Gesture},
};
use ara_host::{Host, HostConfig};
use ara_proto::{ConnId, Endpoint, RpcError};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::mpsc::{self, Receiver},
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

/// One editor process owns the project lock, authority, navigation and agent activity.
pub struct Editor {
    asset_updates: Receiver<()>,
    /// Published model generations retained across document and transform edits.
    pub(crate) models: crate::world::gltf::ModelCache,
    /// Shared engine host. UI content mutations go through this protocol endpoint.
    pub host: Host,
    /// Current native view snapshot, updated by editor action notifications.
    pub state: EditorState,
    /// Human session owned by this editor window.
    pub session: String,
    /// Active provisional spatial edit.
    pub gesture: Option<Gesture>,
    active_fields: BTreeSet<(String, String, String)>,
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
    pub(crate) command_schemas: BTreeMap<String, Value>,
    pub(crate) view_revision: u64,
    /// Bumped when navigation changes what the Scene view renders (`scene_changed`);
    /// other navigation changes only redraw the UI over the last Scene image.
    pub(crate) scene_revision: u64,
    pub(crate) panels_revision: u64,
    /// Bumps when agent activity changes what is tinted (D24 attribution; the
    /// viewport rebuilds its tints on change). Named before D24's just-apply model.
    pub(crate) review_revision: u64,
    pub(crate) extraction_revision: u64,
    /// Registry commands requested over MCP, run by the shell at the next frame boundary.
    pub(crate) queued_commands: Vec<(String, crate::shell::commands::Target)>,
    /// Agent activity seen by this window: glows, notices, overwrite reports (D24).
    pub activity: crate::activity::Activity,
    /// Embedded Play surfaces for the Game view (taken from the play service).
    pub(crate) play_surfaces: Option<ara_play_host::embed::Surfaces>,
    /// The Play world this window started (`play:3`).
    pub(crate) play_world: Option<String>,
    /// The Game view's size in physical pixels: the size embedded Play starts at.
    pub(crate) game_size: [u32; 2],
    /// Model sources already checked on disk, present or reported missing
    /// (`report_missing_sources`); each is checked once per session.
    sources_checked: BTreeSet<String>,
    /// Each scene's last Scene view camera, saved as a user preference.
    cameras: crate::view_camera::SavedCameras,
}
impl Drop for Editor {
    fn drop(&mut self) {
        self.cameras.observe(&self.state, true);
    }
}
impl Editor {
    /// Open the project with in-memory preferences (nothing is written outside it).
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        Self::open_with(root, None)
    }
    /// Open the project, register editor commands/services and create the window's human
    /// session. `user_settings` is the preferences file; `None` keeps them in memory.
    pub fn open_with(
        root: &Path,
        user_settings: Option<std::path::PathBuf>,
    ) -> anyhow::Result<Self> {
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
        host.why().register(Box::new(ara_ai::why::NpcExplainer));
        ara_world::service::register(&mut host).map_err(|d| anyhow::anyhow!(d.to_string()))?;
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
        // Scenes reopen with the camera they were left with (a user preference).
        let cameras = crate::view_camera::SavedCameras::open(user_settings.as_deref(), root);
        let settings = crate::settings::Settings::open(root, user_settings)?;
        let activity = crate::activity::Activity::new(host.store().revision());
        let mut state = EditorState {
            camera: cameras.saved.get(&doc).copied().unwrap_or_default(),
            scene_cameras: cameras.saved.clone(),
            doc,
            ..Default::default()
        };
        crate::actions::sync_settings(&mut state, &settings);
        let navigation = Navigation::new(root, state)?;
        let state = navigation.state.clone();
        let (changes, updates) = mpsc::channel();
        let (control_send, controls) = mpsc::channel();
        host.register(Box::new(EditorService {
            navigation,
            settings,
            changes,
            controls: control_send,
            resolver: Default::default(),
        }))
        .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        // Play runs in pooled ara-player processes; the toolbar binds play.* methods.
        let mut play = ara_play_host::PlayConfig::for_project(root);
        play.pool.headless = 1;
        let mut play = ara_play_host::PlayService::new(root, play);
        // The Game view shows embedded players through shared GPU surfaces (D19).
        let play_surfaces = Some(play.surfaces());
        host.register(Box::new(play))
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
            "collab.join",
            json!({"session":session,"name":"Editor"}),
        )
        .map_err(EditorError::from)?;
        host.call(
            ConnId::LOCAL,
            "subscribe",
            json!({"session":session,"topics":["diagnostics","history","jobs"]}),
        )
        .map_err(EditorError::from)?;
        let mut editor = Self {
            asset_updates,
            models: crate::world::gltf::ModelCache::new(root)?,
            host,
            state,
            session,
            gesture: None,
            active_fields: BTreeSet::new(),
            profile_current: None,
            profile_baseline: None,
            captures,
            completed,
            updates,
            controls,
            command_schemas,
            view_revision: 0,
            scene_revision: 0,
            panels_revision: 0,
            review_revision: 0,
            extraction_revision: 0,
            queued_commands: Vec::new(),
            activity,
            play_surfaces,
            play_world: None,
            game_size: [1280, 720],
            sources_checked: BTreeSet::new(),
            cameras,
        };
        // A scene without a saved camera gets its opening view, like `editor.scene.open`.
        if !editor.state.scene_cameras.contains_key(&editor.state.doc) {
            editor.action("editor.frame", json!({"opening": true}))?;
        }
        Ok(editor)
    }
    /// Push native human activity into the non-disruption boundary.
    pub fn input_active(&mut self) {}
    /// A reflected editor action, using the same validation as an external client.
    pub fn action(&mut self, method: &str, mut params: Value) -> Result<Value, EditorError> {
        params["session"] = self.session.clone().into();
        let result = self.call_for(ConnId::LOCAL, method, params)?;
        if method == "editor.play" {
            // Multi-player Play (palette): the service resolved the `play.start` params.
            let mut start = result;
            start["background"] = true.into();
            self.embed_play(&mut start);
            return self.action("play.start", start);
        }
        if method == "play.start"
            && let Some(world) = result["world"].as_str()
        {
            self.play_world = Some(world.to_owned());
        } else if method == "play.stop" {
            self.play_world = None;
        }
        Ok(result)
    }
    /// Play in the Game view (setting `play.inGameView`, on by default): no player
    /// window, an embedded surface at the Game view's size instead.
    pub(crate) fn embed_play(&self, params: &mut Value) {
        if self
            .state
            .settings
            .get("play.inGameView")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            let [w, h] = self.game_size;
            params["window"] = false.into();
            params["embed"] = json!({"width": w.clamp(64, 8192), "height": h.clamp(64, 8192)});
        }
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
        while let Ok(mut state) = self.updates.try_recv() {
            // GPU detection belongs to this window, not to navigation history: keep
            // it and re-derive `quality` from the incoming settings before comparing.
            // Otherwise every update (each camera step of an orbit, each commit) looked
            // like a panel change and rebuilt the whole shell.
            state.detected_quality = self.state.detected_quality;
            state.apply_settings();
            // The theme is not here: it only tints deletion ghosts (none since D24),
            // and re-extracting a large scene for it took seconds.
            if self.state.doc != state.doc || self.state.hidden != state.hidden {
                self.extraction_revision += 1;
            }
            if self.state.panels_changed(&state) {
                self.panels_revision += 1;
            }
            if self.state.scene_changed(&state) {
                self.scene_revision += 1;
            }
            self.state = state;
            self.view_revision += 1;
            changed = true;
        }
        if changed {
            let now = std::time::Instant::now();
            let store = self.host.store();
            let describe = |doc: &str, selection: &[String]| {
                let scene = store
                    .workspace()
                    .path_of(doc)
                    .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())
                    .unwrap_or_else(|| doc.to_owned());
                let entity = selection.first().and_then(|id| {
                    crate::world::resolve(store, doc).ok().and_then(|r| {
                        r.get(id)
                            .and_then(|e| e.record.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                });
                match entity {
                    Some(name) => format!("{scene} · {name}"),
                    None => scene,
                }
            };
            self.activity.observe_presence(&self.state, describe, now);
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
                Control::Command(id, target) => self.queued_commands.push((id, target)),
            }
            self.panels_revision += 1;
        }
        changed
    }
    /// A host notification for this window (`diagnostics`, `history`, `jobs` and
    /// collaboration): the panels showing them rebuild, and history that changes
    /// review state (an agent's turn, an undo or redo, accept or reject) re-tints the
    /// Scene. A window's own edit changes neither: the store revision it advanced
    /// already rebuilt the panels and patched the Scene, so its plain human history
    /// and its own collaboration echoes (`collab.op`, `collab.presence`) are skipped.
    /// Bumping for them rebuilt the shell again and re-prepared the whole Scene a
    /// frame after every commit.
    pub(crate) fn notified(&mut self, value: &Value) {
        let params = &value["params"];
        if params["participant"] == self.session.as_str() {
            return;
        }
        if value["method"] == "history" {
            if params["author"] == "human" && params["reverts"].is_null() {
                return;
            }
            self.review_revision += 1;
        }
        self.panels_revision += 1;
    }
    /// Advance file watching and job completion at a frame boundary.
    pub fn tick(&mut self) {
        self.host.tick();
        self.sync();
        self.cameras.observe(&self.state, false);
        let now = std::time::Instant::now();
        let mut activity = self.activity.observe_history(self.host.store(), now);
        if let Some(since) = self.activity.log_poll(now) {
            let logs = self
                .action(
                    "logs",
                    json!({"since": since, "codes": ["ARA2503"], "limit": 20}),
                )
                .unwrap_or(Value::Null);
            if let Some(cursor) = logs["cursor"].as_u64() {
                let messages = logs["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e["diagnostic"]["message"].as_str().map(str::to_owned))
                    .collect();
                self.activity.logs_read(cursor, messages, now);
            }
        }
        activity |= self.activity.expire(now);
        if activity {
            self.review_revision += 1;
            self.panels_revision += 1;
        }
    }
    /// Report each model source the open scene references but the disk lacks, once
    /// per path (Problems names it; the entity draws a placeholder). Only paths not
    /// seen before touch the disk, so edits cost no file checks; a source deleted
    /// later surfaces as a failed import instead.
    pub fn report_missing_sources(&mut self, view: &DocumentView) {
        let root = self.host.root().to_owned();
        for (path, missing) in view.missing_sources(&root, &self.sources_checked) {
            self.sources_checked.insert(path.clone());
            if let Some(entity) = missing {
                self.report(
                    "missing",
                    &format!(
                        "`{path}` is missing; {entity} draws a placeholder. Restore the file or fix the reference"
                    ),
                    Some(&format!("asset:@{path}")),
                );
            }
        }
    }
    /// Read-only current edit-world adapter.
    pub fn view(&self) -> anyhow::Result<DocumentView> {
        let view = DocumentView::new(self.host.store(), &self.state.doc).map_err(|d| {
            anyhow::anyhow!(
                d.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })?;
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
        let field = match self.state.tool {
            crate::state::Tool::Translate => "Transform.position",
            crate::state::Tool::Rotate => "Transform.rotation",
            crate::state::Tool::Scale => "Transform.scale",
        };
        let entities: Vec<_> = self
            .gesture
            .as_ref()
            .into_iter()
            .flat_map(|g| g.initial.keys().cloned())
            .collect();
        for entity in entities {
            self.field_gesture(&entity, field, true)?;
        }
        Ok(())
    }
    /// Mark a human field edit for host-side gesture protection.
    pub fn field_gesture(
        &mut self,
        entity: &str,
        path: &str,
        active: bool,
    ) -> Result<(), EditorError> {
        self.action(
            "editor.gesture",
            json!({"doc":self.state.doc,"entity":entity,"path":path,"active":active}),
        )?;
        let key = (self.state.doc.clone(), entity.to_owned(), path.to_owned());
        if active {
            self.active_fields.insert(key);
        } else {
            self.active_fields.remove(&key);
        }
        Ok(())
    }
    /// Release field edits after their human commit, or on cancellation.
    pub fn end_field_gestures(&mut self) -> Result<(), EditorError> {
        for (doc, entity, path) in std::mem::take(&mut self.active_fields) {
            self.action(
                "editor.gesture",
                json!({"doc":doc,"entity":entity,"path":path,"active":false}),
            )?;
        }
        Ok(())
    }
    /// Finish the preview as one content transaction, or discard it with no document change.
    pub fn finish_gesture(&mut self, cancel: bool) -> Result<Option<Value>, EditorError> {
        let Some(gesture) = self.gesture.take() else {
            return Ok(None);
        };
        let commands = gesture.commands();
        let result = if cancel || commands.is_empty() {
            Ok(None)
        } else {
            self.apply(commands, "Transform gesture", Some(gesture.base))
                .map(Some)
        };
        self.end_field_gestures()?;
        result
    }
    /// Incoming calls apply through the shared host authority.
    pub fn call_for(
        &mut self,
        conn: ConnId,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        let result = self.host.call(conn, method, params);
        self.sync();
        result
    }
    /// Log a failed UI action: an engine error keeps its own diagnostics (code,
    /// location, fixes, so Problems can explain it); anything else is `ARA4500`.
    pub(crate) fn report_failure(&mut self, error: &(dyn std::error::Error + 'static)) {
        if let Some(EditorError(rpc)) = error.downcast_ref::<EditorError>()
            && let Some(diagnostics) = rpc
                .data
                .as_ref()
                .and_then(|d| d.get("diagnostics"))
                .and_then(Value::as_array)
                .filter(|d| !d.is_empty())
        {
            let diagnostics = diagnostics.clone();
            if self
                .action("diag.report", json!({"diagnostics": diagnostics}))
                .is_ok()
            {
                return;
            }
        }
        self.report_error(&error.to_string());
    }
    /// Log an editor action failure to the diagnostics bus (Console, Problems, `ara logs`).
    pub(crate) fn report_error(&mut self, message: &str) {
        self.report("action", message, None);
    }
    /// Log an editor failure of `kind` (see `editor.report`) about an optional subject.
    pub(crate) fn report(&mut self, kind: &str, message: &str, subject: Option<&str>) {
        // The editor's own problems come from the engine, not the person at the
        // keyboard: report them as the engine (`origin: engine` on the bus).
        let diagnostic = crate::actions::report_diagnostic(kind, message.into(), subject);
        if let Ok(value) = serde_json::to_value(&diagnostic) {
            let _ = self.action(
                "diag.report",
                json!({"engine": true, "diagnostics": [value]}),
            );
        }
    }
    /// Fields agents changed within the activity glow, with their previous values.
    pub fn agent_fields(&self) -> BTreeMap<(String, String), Value> {
        let mut fields = BTreeMap::new();
        for op in self
            .activity
            .recent_ops(self.host.store(), std::time::Instant::now())
        {
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
    /// Viewport/hierarchy tints of records agents changed in the open scene just now.
    pub fn agent_tints(&self) -> BTreeMap<u64, &'static str> {
        self.activity.tints(
            self.host.store(),
            &self.state.doc,
            std::time::Instant::now(),
        )
    }
}
impl Endpoint for Editor {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_for(ConnId::LOCAL, method, params)
    }
}
