//! [`Host`]: one project's engine state behind the Engine Protocol. The daemon
//! serves it over WebSocket/stdio; the offline CLI and tests call it in-process.
//!
//! Every call goes: route → session + scope check → params validation → service →
//! drain. The drain journals each committed transaction, starts review tracking for
//! agent edits, marks documents for the debounced flush and notifies subscribers.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ara_cmd::Registry;
use ara_diag::bus::{self, Meta, Origin};
use ara_diag::store::{Retention, Store as DiagStore};
use ara_diag::{Diagnostic, FixAction, codes, did_you_mean};
use ara_doc::{AssetId, IdGen};
use ara_ops::{Author, Store, Transaction, TxnId};
use ara_proto::jsonrpc::{self, Incoming};
use ara_proto::{ConnId, Endpoint, MethodInfo, RpcError, Scope};
use ara_schema::{SchemaDb, Validator};
use serde_json::{Value, json};

use crate::files::{Files, Synced};
use crate::jobs::Jobs;
use crate::journal::{self, Entry, Journal};
use crate::params::{ReviewStatus, Topic};
use crate::project::{Project, STATE_DIR};
use crate::review::Review;
use crate::service::{Cx, Method, Service};
use crate::session::{Approval, DefaultPolicy, Sessions};

/// Method families reserved for services that register later, with who provides
/// them. Calls get `ARA1806` until then.
pub const RESERVED: [(&str, &str); 4] = [
    ("run", "the C# script worker (code mode)"),
    ("capture", "the renderer's headless capture service"),
    ("play", "the simulation / play-mode service"),
    ("test", "the test runner"),
];

/// Host settings.
pub struct HostConfig {
    /// Quiet time after the last edit before a document is written to disk.
    pub debounce: Duration,
    /// How often loaded files are checked for external edits when OS change
    /// notifications are unavailable (with them, a full check runs every 30 s as a
    /// safety net). Checks run on the host's file thread; zero instead checks every
    /// file synchronously on each `tick` (tests).
    pub watch_every: Duration,
    /// How often the project is scanned for new document files when OS change
    /// notifications are unavailable.
    pub scan_every: Duration,
    /// Seed for host IDs (sessions, checkpoints, jobs) and the store; entropy if `None`.
    pub seed: Option<u64>,
    /// Decides which scopes new sessions get.
    pub approval: Box<dyn Approval>,
    /// How many diagnostics the bus store keeps per severity, and for how long.
    pub diagnostics: Retention,
    /// Recent-field overwrite notice window; defaults to ten minutes.
    pub overwrite_window: Duration,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            debounce: Duration::from_millis(200),
            watch_every: Duration::from_millis(250),
            scan_every: Duration::from_secs(2),
            seed: None,
            approval: Box::new(DefaultPolicy),
            diagnostics: Retention::default(),
            overwrite_window: Duration::from_secs(600),
        }
    }
}

/// Everything the services work on. Crate-private; services see it through [`Cx`].
pub(crate) struct State {
    pub(crate) profiler: ara_profile::Profiler,
    pub(crate) profile_attached: bool,
    pub(crate) root: PathBuf,
    pub(crate) name: String,
    /// The project's main scene (path), the default document of calls naming none.
    pub(crate) main: Option<String>,
    pub(crate) store: Store,
    pub(crate) registry: Registry,
    pub(crate) proto: SchemaDb,
    pub(crate) catalog: Vec<MethodInfo>,
    /// Commands published at runtime by services (C# `[Command]`s of a running play
    /// world): name to (source, ara-cmd `Info`-shaped entry). Listed and described
    /// with the registry; run through their source's method (`play.command`).
    pub(crate) external: BTreeMap<String, (String, Value)>,
    pub(crate) sessions: Sessions,
    pub(crate) review: Review,
    pub(crate) journal: Journal,
    pub(crate) files: Files,
    /// The diagnostics bus store: everything logged here or drained from the bus.
    pub(crate) diag: DiagStore,
    /// This process's bus consumer (drained every tick).
    pub(crate) bus: bus::Consumer,
    /// Live `diag.subscribe` streams.
    pub(crate) diag_subs: crate::diag::Subscriptions,
    /// The `ara why` explainers.
    pub(crate) why: ara_why::Why,
    /// The engine's runtime diagnostics on disk (`.ara/diagnostics.jsonl`).
    pub(crate) diag_file: crate::diag::Persist,
    /// Collaboration: participants, presence, claims, branch sessions (D21).
    pub(crate) collab: crate::collab::Collab,
    pub(crate) jobs: Jobs,
    pub(crate) gestures: crate::gesture::Gestures,
    pub(crate) subs: BTreeMap<ConnId, BTreeSet<Topic>>,
    pub(crate) outbox: Vec<(ConnId, Value)>,
    pub(crate) txn_session: HashMap<TxnId, String>,
    /// The session of the call in progress; its commits are attributed to it.
    pub(crate) calling: Option<String>,
    /// Set while committing compensating (review revert) transactions, which need no
    /// review of their own.
    pub(crate) untracked: bool,
    pub(crate) ids: IdGen,
    pub(crate) approval: Box<dyn Approval>,
    pub(crate) debounce: Duration,
    pub(crate) watch_every: Duration,
    pub(crate) stop: bool,
    killed: bool,
    ops: Receiver<Transaction>,
    _lock: File,
}

/// One project's engine host.
pub struct Host {
    st: State,
    services: Vec<Box<dyn Service>>,
    routes: BTreeMap<String, (usize, Method)>,
}

fn io_error(what: &str, path: &Path, e: std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::IO,
        format!("cannot {what} `{}`: {e}", path.display()),
    )
}

/// Takes the project lock (`.ara/lock`). `ARA1903` if another process holds it.
fn lock(state_dir: &Path) -> Result<File, Diagnostic> {
    let path = state_dir.join("lock");
    let file = File::create(&path).map_err(|e| io_error("create", &path, e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Diagnostic::error(
            codes::DAEMON_RUNNING,
            "another `ara` process (a daemon or a running command) holds this project's lock",
        )),
        Err(TryLockError::Error(e)) => Err(io_error("lock", &path, e)),
    }
}

impl Host {
    /// Opens a host over `project`: takes the project lock, loads every document,
    /// replays the journal (recovering edits a crash kept from disk) and registers
    /// the core and profiling services.
    pub fn open(project: Project, config: HostConfig) -> Result<Host, Diagnostic> {
        let state_dir = project.root.join(STATE_DIR);
        fs::create_dir_all(&state_dir).map_err(|e| io_error("create", &state_dir, e))?;
        let lock = lock(&state_dir)?;
        let (mut ws, per_file) = project.load(&[]);
        // Canonical import sidecars are editable documents too. Source-only legacy
        // identity stubs remain index entries until explicitly initialized.
        if project.db.kind("import").is_some() {
            for (id, path) in project.index.imports() {
                let sidecar = format!("{path}.import.json");
                if let Ok(text) = fs::read_to_string(project.root.join(&sidecar)) {
                    let result = ara_doc::read(&text, &sidecar, &project.db, Some(&project.index));
                    if let Some(doc) = result
                        .doc
                        .filter(|d| d.kind == "import" && d.id.as_str() == id)
                    {
                        ws.insert_with_asset_path(sidecar, doc, path);
                    }
                }
            }
        }
        let marker = fs::read_to_string(project.root.join(crate::project::MARKER))
            .map_err(|e| io_error("read", &project.root.join(crate::project::MARKER), e))?;
        let settings =
            crate::project_settings::read(&marker, crate::project::MARKER, &project.db, None);
        let settings = settings.doc.ok_or_else(|| {
            settings
                .diags
                .into_iter()
                .next()
                .expect("invalid settings has a diagnostic")
        })?;
        ws.insert(crate::project::MARKER, settings);
        let name = project.name();
        let main = project.main_scene();
        let Project {
            root,
            db,
            registry,
            diags,
            type_files,
            ..
        } = project;
        let consumer = bus::attach();
        let mut diag = DiagStore::new(config.diagnostics);
        let (diag_file, unreadable) = crate::diag::Persist::open(&state_dir, &mut diag);
        if unreadable > 0 {
            diag.push(
                Diagnostic::warning(
                    codes::DIAG_INVALID_REPORT,
                    format!("{unreadable} lines of `.ara/diagnostics.jsonl` could not be read and were skipped"),
                )
                .file(".ara/diagnostics.jsonl"),
                Meta::default(),
            );
        }
        for d in diags.into_iter().chain(per_file.into_values().flatten()) {
            diag.push(d, Meta::default());
        }
        let type_files = type_files
            .into_iter()
            .map(|p| {
                let m = fs::metadata(&p).and_then(|m| m.modified()).ok();
                (p, m)
            })
            .collect();
        let io = crate::file_io::FileThread::start(
            root.clone(),
            db.clone(),
            type_files,
            config.watch_every,
            config.scan_every,
        )
        .map_err(|e| io_error("start the file thread for", &root, e))?;
        let mut files = Files::new(io);
        for (path, doc) in ws.docs() {
            let abs = root.join(path);
            if let Ok(text) = fs::read_to_string(&abs) {
                files.io.track(path.to_owned(), crate::file_io::stamp(&abs));
                files.synced.insert(
                    doc.id.clone(),
                    Synced::new(path.to_owned(), text, doc.clone()),
                );
            }
        }
        let ids = config.seed.map_or_else(IdGen::from_entropy, IdGen::seeded);
        let store_ids = config
            .seed
            .map_or_else(IdGen::from_entropy, |s| IdGen::seeded(s ^ 0x5eed));
        let mut store = Store::with_ids(db, ws, store_ids);
        let (journal, entries, journal_diags) =
            Journal::open(&state_dir).map_err(|e| io_error("open", &state_dir, e))?;
        for d in journal_diags {
            diag.push(d, Meta::default());
        }
        let ops = store.subscribe();
        let mut st = State {
            profiler: ara_profile::Profiler::new(ara_profile::Config::default())
                .map_err(|e| Diagnostic::error(codes::INVALID_PARAMS, e))?,
            profile_attached: false,
            root,
            name,
            main,
            store,
            registry,
            proto: SchemaDb::new(),
            catalog: Vec::new(),
            external: BTreeMap::new(),
            sessions: Sessions::default(),
            review: Review::default(),
            journal,
            files,
            diag,
            bus: consumer,
            diag_subs: BTreeMap::new(),
            why: ara_why::Why::builtin(),
            diag_file,
            collab: crate::collab::Collab::new(state_dir.clone()),
            jobs: Jobs::default(),
            gestures: crate::gesture::Gestures::default(),
            subs: BTreeMap::new(),
            outbox: Vec::new(),
            txn_session: HashMap::new(),
            calling: None,
            untracked: false,
            ids,
            approval: config.approval,
            debounce: config.debounce,
            watch_every: config.watch_every,
            stop: false,
            killed: false,
            ops,
            _lock: lock,
        };
        st.recover(&entries);
        st.refresh_project_settings();
        st.collab_open(config.seed);
        st.collab.hub.set_overwrite_window(
            config
                .overwrite_window
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        drop(entries);
        st.flush(true);
        let mut host = Host {
            st,
            services: Vec::new(),
            routes: BTreeMap::new(),
        };
        host.register(Box::new(crate::core::Core))?;
        host.register(Box::new(crate::gesture::GestureService))?;
        host.register(Box::new(crate::profile::ProfileService))?;
        host.register(Box::new(crate::diag::DiagService))?;
        host.register(Box::new(crate::why::WhyService))?;
        host.register(Box::new(crate::collab::CollabService))?;
        Ok(host)
    }

    /// Adds a service. Its methods appear in `rpc.discover` (and as MCP tools)
    /// immediately. Fails if a method name is taken or a params type is invalid.
    pub fn register(&mut self, service: Box<dyn Service>) -> Result<(), Diagnostic> {
        let methods = service.methods(&mut self.st.proto);
        if let Some(d) = self
            .st
            .proto
            .verify()
            .into_iter()
            .find(Diagnostic::is_error)
        {
            return Err(d);
        }
        if let Some(m) = methods.iter().find(|m| self.routes.contains_key(&m.name)) {
            return Err(Diagnostic::error(
                codes::UNKNOWN_METHOD,
                format!("method `{}` is already registered", m.name),
            ));
        }
        let index = self.services.len();
        let name = service.name();
        self.services.push(service);
        for m in methods {
            let mut params = self
                .st
                .proto
                .get(&m.params)
                .map(|t| self.st.proto.type_json_schema(t))
                .unwrap_or(Value::Null);
            if m.name == "cmd.apply" {
                crate::core::list_commands(&mut params, &self.st.registry, self.st.store.schema());
            }
            self.st.catalog.push(MethodInfo {
                name: m.name.clone(),
                summary: m.summary.clone(),
                scope: m.scope,
                hints: m.hints,
                tool: m.tool.clone(),
                params,
                service: name.to_owned(),
            });
            self.routes.insert(m.name.clone(), (index, m));
        }
        Ok(())
    }

    /// The `ara why` explainer registry: subsystems register their explainers and
    /// add collectors to the built-in ones here.
    pub fn why(&mut self) -> &mut ara_why::Why {
        &mut self.st.why
    }

    /// Replaces the connection-approval hook (the editor asks the human).
    pub fn set_approval(&mut self, approval: Box<dyn Approval>) {
        self.st.approval = approval;
    }

    /// The method catalog, in registration order (deterministic).
    pub fn catalog(&self) -> &[MethodInfo] {
        &self.st.catalog
    }

    /// Documents and history.
    pub fn store(&self) -> &Store {
        &self.st.store
    }
    /// Per-entity review state for one transaction, keyed by `doc/entity`.
    /// Editor overlays read the same state that `review.list/accept/revert` use.
    pub fn review_entities(&self, txn: &TxnId) -> Option<&BTreeMap<String, ReviewStatus>> {
        self.st.review.entities(txn)
    }

    /// The project root.
    pub fn root(&self) -> &Path {
        &self.st.root
    }

    /// True after `daemon.shutdown`.
    pub fn stopping(&self) -> bool {
        self.st.stop
    }

    /// Calls a method for connection `conn`.
    pub fn call(&mut self, conn: ConnId, method: &str, params: Value) -> Result<Value, RpcError> {
        let result = self.dispatch(conn, method, params);
        self.st.drain();
        self.st.release_gestures();
        if let Err(diags) = &result {
            for d in diags.iter().filter(|d| d.is_error()) {
                self.st.log(d.clone());
            }
        }
        self.st.calling = None;
        self.st.store.set_origin(None);
        result.map_err(RpcError::from_diagnostics)
    }

    fn dispatch(
        &mut self,
        conn: ConnId,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        let Some((index, def)) = self.routes.get(method).cloned() else {
            return Err(vec![unknown_method(method, self.routes.keys())]);
        };
        let session = match def.scope {
            None => None,
            Some(scope) => Some(self.st.check_session(&params, scope, method)?),
        };
        let Some(ty) = self.st.proto.get(&def.params) else {
            return Err(vec![Diagnostic::error(
                codes::UNKNOWN_TYPE,
                format!("params type `{}` is not registered", def.params),
            )]);
        };
        let mut problems = Vec::new();
        match params.as_object() {
            Some(obj) => Validator::new(&self.st.proto, &mut problems).fields(ty, obj, "params"),
            None => problems.push(Diagnostic::error(
                codes::INVALID_REQUEST,
                "params must be an object",
            )),
        }
        problems.retain(Diagnostic::is_error);
        if !problems.is_empty() {
            return Err(problems);
        }
        self.st.calling = session.clone();
        // Origins preserve the participant identity of attributed transactions.
        self.st.store.set_origin(session.clone());
        // Subsystems that emit onto the bus while serving the call are attributed to
        // the caller.
        let _origin = self.st.calling_origin().map(bus::scope);
        let mut cx = Cx {
            st: &mut self.st,
            conn,
            session,
        };
        self.services[index].call(&mut cx, method, params)
    }

    /// Handles one JSON-RPC message text from `conn`; returns the response text
    /// (none for notifications and stray responses).
    pub fn handle_text(&mut self, conn: ConnId, text: &str) -> Option<String> {
        let reply = match Incoming::parse(text) {
            Err(error) => Some(error),
            Ok(Incoming::Request { id, method, params }) => {
                Some(jsonrpc::reply(id, self.call(conn, &method, params)))
            }
            Ok(Incoming::Notification { method, params }) => {
                let _ = self.call(conn, &method, params);
                None
            }
            Ok(Incoming::Response { .. }) => None,
        };
        reply.map(|r| r.to_string())
    }

    /// Periodic work: external-edit checks, debounced flushes, service ticks.
    pub fn tick(&mut self) {
        let profiling = self.st.profiler.is_recording() && self.st.profiler.begin_frame().is_ok();
        {
            ara_profile::scope!("host.tick");
            self.st.pump_bus();
            self.st.persist_diag(false);
            self.st.release_gestures();
            self.st.watch();
            self.st.drain();
            self.st.flush(false);
            for service in &mut self.services {
                let mut cx = Cx {
                    st: &mut self.st,
                    conn: ConnId::LOCAL,
                    session: None,
                };
                service.tick(&mut cx);
            }
            self.st.drain();
        }
        if profiling {
            let _ = self
                .st
                .profiler
                .end_frame(Default::default(), Default::default());
        }
    }

    /// Frame-inspector integration: register render/simulation/worker endpoints,
    /// begin/end presentation frames, and attach delayed GPU results by frame ID.
    /// Host ticks make their own frames only when no presentation frame is open.
    pub fn profiler(&mut self) -> &mut ara_profile::Profiler {
        &mut self.st.profiler
    }

    /// Whether any document has changes not yet written (they save after a short debounce).
    pub fn unsaved(&self) -> bool {
        !self.st.files.changed.is_empty()
    }

    /// Writes every document with unsaved changes now.
    pub fn flush(&mut self) {
        self.st.drain();
        self.st.flush(true);
        self.st.pump_bus();
        self.st.persist_diag(true);
    }

    /// Forgets a closed connection's subscriptions.
    pub fn closed(&mut self, conn: ConnId) {
        self.st.subs.remove(&conn);
        self.st.diag_subs.retain(|_, s| s.conn != conn);
        self.st.collab_closed(conn);
        self.st.close_gestures(conn);
    }

    /// Notifications waiting for their connections.
    pub fn drain_outbox(&mut self) -> Vec<(ConnId, Value)> {
        std::mem::take(&mut self.st.outbox)
    }
}

impl Host {
    /// Drops the host without writing pending documents, as a crash would. The
    /// journal already holds every commit; the next open replays them.
    pub fn kill(mut self) {
        self.st.files.changed.clear();
        self.st.killed = true;
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if self.st.profile_attached {
            ara_profile::ThreadRegistration::detach();
        }
        if !self.st.killed {
            self.flush();
        }
    }
}

impl Endpoint for Host {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        Host::call(self, ConnId::LOCAL, method, params)
    }
}

fn unknown_method<'a>(method: &str, known: impl Iterator<Item = &'a String>) -> Diagnostic {
    let family = method.split('.').next().unwrap_or(method);
    if let Some((_, who)) = RESERVED.iter().find(|(f, _)| *f == family) {
        return Diagnostic::error(
            codes::SERVICE_UNAVAILABLE,
            format!(
                "`{method}` is not available yet: it is provided by {who}, which is not registered in this engine build"
            ),
        );
    }
    let known: Vec<&String> = known.collect();
    let mut msg = format!("unknown method `{method}`");
    if let Some(near) = did_you_mean(method, known.iter().map(|s| s.as_str())) {
        msg.push_str(&format!("; did you mean `{near}`?"));
    }
    Diagnostic::error(codes::UNKNOWN_METHOD, msg)
}

/// `ARA1803` with the fix: begin a session.
pub(crate) fn session_not_found(id: &str) -> Diagnostic {
    Diagnostic::error(
        codes::SESSION_NOT_FOUND,
        format!("no live session `{id}`; begin one with `session.begin` and pass its handle"),
    )
    .with_fix(
        "Begin a session",
        FixAction::Call {
            method: "session.begin".into(),
            params: json!({ "client": "<your client name>" }),
        },
    )
}

impl State {
    /// The live session named by `params.session`, if it holds `scope`.
    pub(crate) fn check_session(
        &self,
        params: &Value,
        scope: Scope,
        method: &str,
    ) -> Result<String, Vec<Diagnostic>> {
        let Some(id) = params.get("session").and_then(Value::as_str) else {
            return Err(vec![session_not_found("").path("params.session")]);
        };
        let Some(s) = self.sessions.live(id) else {
            return Err(vec![session_not_found(id).path("params.session")]);
        };
        if !s.allows(scope) {
            let held: Vec<&str> = s.granted.iter().map(|g| g.as_str()).collect();
            return Err(vec![Diagnostic::error(
                codes::MISSING_SCOPE,
                format!(
                    "`{method}` needs the `{scope}` scope; session `{id}` holds [{}]",
                    held.join(", ")
                ),
            )]);
        }
        Ok(id.to_owned())
    }

    /// The origin of the call in progress (its session's author).
    pub(crate) fn calling_origin(&self) -> Option<Origin> {
        let session = self.sessions.get(self.calling.as_deref()?)?;
        Some(Origin::of_author(&session.author.to_string()))
    }

    /// Records a diagnostic, attributed to the calling session's author, and
    /// notifies subscribers.
    pub(crate) fn log(&mut self, d: Diagnostic) {
        let meta = Meta {
            origin: self.calling_origin(),
            ..Meta::default()
        };
        let seq = self.diag.push(d, meta).seq;
        self.published(&[seq]);
    }

    /// Appends changed engine diagnostics to `.ara/diagnostics.jsonl`.
    pub(crate) fn persist_diag(&mut self, now: bool) {
        if let Err(e) = self.diag_file.flush(&self.diag, now) {
            let d = Diagnostic::warning(
                codes::IO,
                format!("cannot write `.ara/diagnostics.jsonl`: {e}"),
            );
            self.diag.push(d, Meta::default());
        }
    }

    /// Moves everything other threads emitted onto the bus into the store.
    pub(crate) fn pump_bus(&mut self) {
        let mut events = Vec::new();
        let lost = self.bus.drain(&mut events);
        let mut touched = self.diag.ingest(events);
        touched.extend(self.diag.absorb(&lost));
        self.diag.expire();
        self.keep(&touched);
        self.published(&touched);
    }

    /// Marks records the engine reported (bus events, `diag.report`) for
    /// `.ara/diagnostics.jsonl`.
    pub(crate) fn keep(&mut self, seqs: &[u64]) {
        for seq in seqs {
            if let Some(r) = self.diag.get(*seq) {
                self.diag_file.mark(r);
            }
        }
    }

    /// Notifies `diagnostics` topic subscribers and matching `diag.subscribe`
    /// streams of new or updated records.
    pub(crate) fn published(&mut self, seqs: &[u64]) {
        let mut out = Vec::new();
        for seq in seqs {
            let Some(record) = self.diag.get(*seq) else {
                continue;
            };
            let entry = serde_json::to_value(record).unwrap_or(Value::Null);
            for (id, sub) in &self.diag_subs {
                if sub.filter.accepts(record) {
                    let params = json!({ "subscription": id, "record": entry });
                    out.push((sub.conn, jsonrpc::notification("diag.event", params)));
                }
            }
            self.notify(Topic::Diagnostics, entry);
        }
        self.outbox.extend(out);
    }

    /// Queues a notification for every connection subscribed to `topic`.
    pub(crate) fn notify(&mut self, topic: Topic, payload: Value) {
        let msg = jsonrpc::notification(topic.method(), payload);
        for (conn, topics) in &self.subs {
            if topics.contains(&topic) {
                self.outbox.push((*conn, msg.clone()));
            }
        }
    }

    /// Appends to the journal, logging (not failing) on IO errors.
    pub(crate) fn journal(&mut self, entry: &Entry) {
        if let Err(e) = self.journal.append(entry) {
            self.log(Diagnostic::error(
                codes::IO,
                format!("cannot append to `.ara/journal`: {e}"),
            ));
        }
    }

    /// A new host ID (`ses_…`, `cp_…`, `jb_…`).
    pub(crate) fn token(&mut self, prefix: &str) -> String {
        self.ids.token(prefix, 8)
    }

    /// Processes committed transactions: journal, review, flush scheduling, history
    /// and op-stream notifications.
    pub(crate) fn drain(&mut self) {
        while let Ok(txn) = self.ops.try_recv() {
            let session = self
                .collab
                .hub
                .owner(&txn.id)
                .map(|p| p.0.clone())
                .or_else(|| self.calling.clone());
            self.journal(&Entry::Txn {
                session: session.clone(),
                txn: txn.clone(),
            });
            if let Some(s) = session {
                self.txn_session.insert(txn.id.clone(), s);
            }
            let now = Instant::now();
            for op in &txn.ops {
                self.files.changed.insert(op.doc().clone(), now);
                if let ara_doc::Op::Document {
                    doc, path, kind, ..
                } = op
                {
                    if kind.is_none() {
                        self.files.deleted.insert(doc.clone(), path.clone());
                    } else {
                        self.files.deleted.remove(doc);
                    }
                }
            }
            self.refresh_project_settings();
            self.track_review(&txn);
            let summary = self.txn_summary(&txn);
            self.notify(Topic::History, summary);
            self.notify(Topic::Ops, json!({ "txn": txn }));
        }
        self.collab_after();
    }

    fn track_review(&mut self, txn: &Transaction) {
        match &txn.reverts {
            Some(target) if self.review.is_tracked(target) => {
                self.review.set(target, None, ReviewStatus::Reverted);
            }
            Some(target) => {
                // Redo restores the original immediate acceptance as well as its ops.
                let original = self.store.txn(target).and_then(|t| t.reverts.clone());
                if let Some(original) = original.filter(|o| self.review.is_tracked(o)) {
                    self.review.set(&original, None, ReviewStatus::Accepted);
                }
            }
            None if matches!(txn.author, Author::Agent(_)) && !self.untracked => {
                self.review.track(txn);
            }
            _ => {}
        }
    }

    /// A transaction as `history.list` shows it.
    pub(crate) fn txn_summary(&self, t: &Transaction) -> Value {
        let status = self.review.status(&t.id).unwrap_or(if t.reverts.is_some() {
            ReviewStatus::Accepted
        } else if self.store.reverted_by(&t.id).is_some() {
            ReviewStatus::Reverted
        } else {
            ReviewStatus::Accepted
        });
        json!({
            "txn": t.id,
            "author": t.author,
            "why": t.why,
            "label": crate::gesture::turn(t.label.as_deref()).map(|(_,label)|label).or(t.label.as_deref()),
            "turn": crate::gesture::turn(t.label.as_deref()).map(|(group,_)|group),
            "revision": t.result,
            "base": t.base,
            "reverts": t.reverts,
            "revertedBy": self.store.reverted_by(&t.id),
            "status": status,
            "entities": crate::review::touched(t),
            "session": self.txn_session.get(&t.id),
        })
    }

    /// Rebuilds history, sessions and review from the journal, re-applying
    /// transactions newer than each document's file.
    fn recover(&mut self, entries: &[Entry]) {
        let mut base = 0;
        let mut flushes: BTreeMap<&str, Vec<(u64, &str)>> = BTreeMap::new();
        for e in entries {
            match e {
                Entry::Base { revision, .. } => base = *revision,
                Entry::Flush { doc, rev, hash } => {
                    flushes.entry(doc).or_default().push((*rev, hash));
                }
                _ => {}
            }
        }
        // The revision each document's file holds; `None` if it changed while no
        // host was running (it is then the truth, and nothing is replayed onto it).
        let mut on_disk: BTreeMap<AssetId, Option<u64>> = BTreeMap::new();
        for (id, synced) in &self.files.synced {
            let h = journal::hash(&synced.text);
            let rev = match flushes.get(id.as_str()) {
                None => Some(base),
                Some(list) => list.iter().rev().find(|(_, fh)| *fh == h).map(|(r, _)| *r),
            };
            on_disk.insert(id.clone(), rev);
        }
        let (mut replayed, mut stopped, mut skipped) = (0usize, false, BTreeSet::new());
        for entry in entries {
            if let Entry::Txn { txn, .. } = entry {
                for op in &txn.ops {
                    if let ara_doc::Op::Document { doc, path, .. } = op
                        && !self.root.join(path).exists()
                    {
                        let rev = flushes
                            .get(doc.as_str())
                            .and_then(|list| {
                                list.iter()
                                    .rev()
                                    .find(|(_, hash)| *hash == journal::hash("deleted"))
                            })
                            .map_or(base, |(rev, _)| *rev);
                        on_disk.entry(doc.clone()).or_insert(Some(rev));
                    }
                }
            }
        }
        for e in entries {
            match e {
                Entry::Txn { session, txn } => {
                    if stopped {
                        continue;
                    }
                    let needs: Vec<Option<bool>> = txn
                        .ops
                        .iter()
                        .map(|op| match on_disk.get(op.doc()) {
                            Some(Some(rev)) => Some(txn.result > *rev),
                            _ => None,
                        })
                        .collect();
                    let apply: BTreeSet<_> = txn
                        .ops
                        .iter()
                        .zip(&needs)
                        .filter(|(_, needed)| **needed == Some(true))
                        .map(|(op, _)| op.doc().clone())
                        .collect();
                    for op in &txn.ops {
                        if matches!(on_disk.get(op.doc()), Some(None))
                            && flushes
                                .get(op.doc().as_str())
                                .and_then(|l| l.last())
                                .is_some_and(|(r, _)| txn.result > *r)
                        {
                            skipped.insert(op.doc().clone());
                        }
                    }
                    match self.store.restore_documents(txn.clone(), &apply) {
                        Ok(()) => {
                            if !apply.is_empty() {
                                replayed += 1;
                                for op in txn.ops.iter().filter(|op| apply.contains(op.doc())) {
                                    self.files.changed.insert(op.doc().clone(), Instant::now());
                                    if let ara_doc::Op::Document {
                                        doc, path, kind, ..
                                    } = op
                                    {
                                        if kind.is_none() {
                                            self.files.deleted.insert(doc.clone(), path.clone());
                                        } else {
                                            self.files.deleted.remove(doc);
                                        }
                                    }
                                }
                            }
                            if let Some(s) = session {
                                self.txn_session.insert(txn.id.clone(), s.clone());
                            }
                            self.track_review(txn);
                        }
                        Err(diags) => {
                            stopped = true;
                            let why = diags.first().map(|d| d.message.clone()).unwrap_or_default();
                            self.diag.push(Diagnostic::error(
                                codes::JOURNAL_CORRUPT,
                                format!("cannot replay `{}` from the journal ({why}); later entries were ignored", txn.id),
                            ).file(".ara/journal"), Meta::default());
                        }
                    }
                }
                Entry::Session { session } => self.sessions.insert(session.clone()),
                Entry::Ended { session } => {
                    if let Some(s) = self.sessions.get_mut(session) {
                        s.ended = true;
                    }
                }
                Entry::Checkpoint {
                    session,
                    checkpoint,
                } => {
                    if let Some(s) = self.sessions.get_mut(session) {
                        s.checkpoints.push(checkpoint.clone());
                    }
                }
                Entry::Review {
                    txn,
                    entity,
                    status,
                } => self.review.set(
                    &TxnId(txn.clone()),
                    entity.as_deref(),
                    match status {
                        ReviewStatus::Applied => ReviewStatus::Accepted,
                        status => *status,
                    },
                ),
                Entry::Base { .. } | Entry::Flush { .. } => {}
            }
        }
        if replayed > 0 {
            self.diag.push(
                Diagnostic::info(
                    codes::JOURNAL_REPLAYED,
                    format!("recovered {replayed} transactions from the journal that had not reached disk"),
                )
                .file(".ara/journal"),
                Meta::default(),
            );
        }
        for (doc, rev) in &on_disk {
            if rev.is_some() {
                continue;
            }
            let path = self
                .store
                .workspace()
                .path_of(doc.as_str())
                .unwrap_or_default()
                .to_owned();
            let d = if skipped.contains(doc) {
                Diagnostic::warning(
                    codes::EXTERNAL_EDIT,
                    format!(
                        "`{path}` changed on disk while no host was running; loaded as is, and journaled edits newer than its last save were not replayed onto it"
                    ),
                )
            } else {
                Diagnostic::info(
                    codes::EXTERNAL_EDIT,
                    format!("`{path}` changed on disk while no host was running; loaded as is"),
                )
            };
            self.diag.push(d.file(path), Meta::default());
        }
    }

    /// Rewrites the journal once it grows past its bound. Only safe when every
    /// document is flushed, so their flush entries cover all dropped transactions.
    pub(crate) fn maybe_compact(&mut self) {
        if self.journal.len() <= journal::MAX_ENTRIES
            || !self.files.changed.is_empty()
            || self.files.io.pending() > 0
        {
            return;
        }
        let result = self.journal.read_all().and_then(|entries| {
            self.journal
                .rewrite(&journal::compact(&entries, journal::KEEP_TXNS))
        });
        if let Err(e) = result {
            self.log(Diagnostic::warning(
                codes::IO,
                format!("cannot compact `.ara/journal`: {e}"),
            ));
        }
    }
}
