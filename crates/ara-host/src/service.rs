//! The service registry: how subsystems add Engine Protocol methods (and with them
//! MCP tools) to the host.
//!
//! A [`Service`] lists its [`Method`]s (each with a `#[derive(Schema)]` params type,
//! a scope and optionally the MCP tool it backs) and handles calls through a [`Cx`].
//! The host checks the session and scope and validates params against the schema
//! before `call` runs, so services never re-check them. The core methods are a
//! service too (`core`).
//!
//! ```ignore
//! /// Params of `play.step`.
//! #[derive(Schema, Deserialize)]
//! #[ara(params)]
//! #[serde(rename_all = "camelCase", deny_unknown_fields)]
//! struct StepParams {
//!     /// Session handle from `session.begin`.
//!     session: String,
//!     /// Ticks to advance.
//!     ticks: u32,
//! }
//!
//! impl Service for Play {
//!     fn name(&self) -> &'static str { "play" }
//!     fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
//!         vec![Method::new::<StepParams>(db, "play.step", "Advance the play world.")
//!             .scope(Scope::Play)
//!             .action("ara_play", "step")]
//!     }
//!     fn call(&mut self, cx: &mut Cx<'_>, method: &str, params: Value) -> Result<Value, Vec<Diagnostic>> {
//!         let p: StepParams = parse(params)?;
//!         ...
//!     }
//! }
//! host.register(Box::new(Play::new()))?;
//! ```

use std::path::Path;

use ara_cmd::Registry;
use ara_diag::{Diagnostic, codes};
use ara_ops::Store;
use ara_proto::{ConnId, MethodHints, Scope, ToolBinding};
use ara_schema::{Schema, SchemaDb};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::host::State;
use crate::params::{ApplyParams, Topic};
use crate::session::Session;

/// One method a service provides.
#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    /// Method name (`play.step`). Namespaced by the service.
    pub name: String,
    /// One line on what it does; also the docs and tool-search summary.
    pub summary: String,
    /// Name of the params type in the protocol Schema DB.
    pub params: String,
    /// Scope the session needs; `None` for methods callable without a session.
    pub scope: Option<Scope>,
    /// Behaviour hints (MCP annotations).
    pub hints: MethodHints,
    /// The MCP tool this method backs.
    pub tool: Option<ToolBinding>,
}

impl Method {
    /// A method whose params are `P` (registered in `db`). Defaults: scope `read`,
    /// no hints, no MCP tool.
    pub fn new<P: Schema>(db: &mut SchemaDb, name: &str, summary: &str) -> Method {
        db.register::<P>();
        Method {
            name: name.to_owned(),
            summary: summary.to_owned(),
            params: P::describe().name,
            scope: Some(Scope::Read),
            hints: MethodHints::default(),
            tool: None,
        }
    }

    /// Requires `scope`.
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Callable without a session.
    pub fn sessionless(mut self) -> Self {
        self.scope = None;
        self
    }

    /// Never changes state.
    pub fn read_only(mut self) -> Self {
        self.hints.read_only = true;
        self.hints.idempotent = true;
        self
    }

    /// Can remove data.
    pub fn destructive(mut self) -> Self {
        self.hints.destructive = true;
        self
    }

    /// The only method behind MCP tool `tool`.
    pub fn tool(mut self, tool: &str) -> Self {
        self.tool = Some(ToolBinding {
            tool: tool.to_owned(),
            action: None,
        });
        self
    }

    /// Behind MCP tool `tool`, selected by `"action": action`.
    pub fn action(mut self, tool: &str, action: &str) -> Self {
        self.tool = Some(ToolBinding {
            tool: tool.to_owned(),
            action: Some(action.to_owned()),
        });
        self
    }
}

/// A subsystem that serves Engine Protocol methods.
pub trait Service: 'static {
    /// Short name shown in the catalog (`core`, `play`, `capture`).
    fn name(&self) -> &'static str;
    /// The methods; register their params types in `db` (via [`Method::new`]).
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method>;
    /// Handles a call. The session and scope are checked and `params` validated
    /// against the method's params type before this runs.
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>>;
    /// Background work, called on every host tick.
    fn tick(&mut self, cx: &mut Cx<'_>) {
        let _ = cx;
    }
}

/// Deserializes validated params into the method's params type.
pub fn parse<P: DeserializeOwned>(params: Value) -> Result<P, Vec<Diagnostic>> {
    serde_json::from_value(params)
        .map_err(|e| vec![Diagnostic::error(codes::INVALID_PARAMS, e.to_string())])
}

/// What a service sees during a call or tick.
pub struct Cx<'a> {
    pub(crate) st: &'a mut State,
    pub(crate) conn: ConnId,
    pub(crate) session: Option<String>,
}

impl Cx<'_> {
    /// Commit a typed batch with a complete inventory revalidated by its owner on a
    /// preparation worker. Metadata is visible only during this batch. This avoids
    /// re-entering source preparation when a source/asset owner already prepared it.
    pub fn apply_prepared(
        &mut self,
        params: ApplyParams,
        inventory: ara_doc::file_state::PreparedInventory,
        prepared_at: u64,
    ) -> Result<Value, Vec<Diagnostic>> {
        if self.st.store.revision() != prepared_at {
            return Err(vec![Diagnostic::error(
                codes::STALE_BASE,
                "authority changed during file preparation; revalidate and retry",
            )]);
        }
        self.st
            .store
            .prepare_files(inventory)
            .map_err(|d| vec![d])?;
        let previous_calling = std::mem::replace(&mut self.st.calling, self.session.clone());
        let previous_origin = self.st.store.origin().map(str::to_owned);
        self.st.store.set_origin(self.session.clone());
        let previous = std::mem::replace(&mut self.st.file_jobs.applying, true);
        let result = self.apply(params);
        self.st.drain();
        self.st.file_jobs.applying = previous;
        self.st.store.clear_prepared_files();
        self.st.calling = previous_calling;
        self.st.store.set_origin(previous_origin);
        result
    }
    /// Publish a worker's prepared file inventory before applying typed source/asset
    /// commands. Tokens contain no mutable bytes and this never writes to disk.
    pub fn observe_files(
        &mut self,
        entries: impl IntoIterator<Item = (String, Option<ara_doc::file_state::FileState>)>,
    ) {
        for (path, state) in entries {
            self.st.store.observe_file(path, state);
        }
    }
    /// Complete background work under its initiating live session, retaining author
    /// attribution and permissions rather than borrowing the editor's human session.
    pub fn resume_session<R>(
        &mut self,
        session: &str,
        complete: impl FnOnce(&mut Self) -> R,
    ) -> Result<R, Vec<Diagnostic>> {
        self.st.sessions.live(session).ok_or_else(|| {
            vec![Diagnostic::error(
                codes::SESSION_NOT_FOUND,
                "background command session expired",
            )]
        })?;
        let previous = self.session.replace(session.to_owned());
        let result = complete(self);
        self.session = previous;
        Ok(result)
    }
    /// The calling session (`None` for sessionless methods and ticks).
    pub fn session(&self) -> Option<&Session> {
        self.session
            .as_deref()
            .and_then(|s| self.st.sessions.get(s))
    }

    /// The connection the call came in on.
    pub fn conn(&self) -> ConnId {
        self.conn
    }

    /// Documents, schema and history. Read-only: edits go through [`Cx::apply`].
    pub fn store(&self) -> &Store {
        &self.st.store
    }

    /// The command, query and job registry.
    pub fn registry(&self) -> &Registry {
        &self.st.registry
    }

    /// The project root.
    pub fn root(&self) -> &Path {
        &self.st.root
    }

    /// Current startup scene, including command edits that have not flushed to disk yet.
    pub fn main_scene(&self) -> Option<&str> {
        self.st.main.as_deref()
    }

    /// Create a document through the shared typed command, including attribution and undo.
    pub fn create_document(&mut self, path: &str, kind: &str) -> Result<String, Diagnostic> {
        let session = self.session.clone().ok_or_else(|| {
            Diagnostic::error(
                codes::SESSION_NOT_FOUND,
                "document creation needs a session",
            )
        })?;
        let params = parse(serde_json::json!({
            "session": session, "why": format!("Create {kind} document"),
            "commands": [{"cmd":"doc.create","args":{"path":path,"kind":kind}}]
        }))
        .map_err(|d| {
            d.into_iter()
                .next()
                .expect("invalid params reports a diagnostic")
        })?;
        let result = self.apply(params).map_err(|d| {
            d.into_iter()
                .next()
                .expect("failed command reports a diagnostic")
        })?;
        Ok(result["outputs"][0]["id"]
            .as_str()
            .expect("doc.create returns its asset ID")
            .to_owned())
    }
    /// Runs a batch of commands for the calling session, exactly like `cmd.apply`
    /// (one transaction, review state, a turn checkpoint for agents).
    pub fn apply(&mut self, params: ApplyParams) -> Result<Value, Vec<Diagnostic>> {
        let session = self.session.clone().ok_or_else(|| {
            vec![Diagnostic::error(
                codes::SESSION_NOT_FOUND,
                "edits need a session",
            )]
        })?;
        self.st.apply(&session, params)
    }

    /// Runs `f` on the mutable kernel store for a service that commits its own typed
    /// transactions (several documents at once: world cell moves). `meta` carries the
    /// calling session's author and `why`. Like `cmd.apply`, the host then journals,
    /// reviews and publishes every commit, and an agent's commit gets a turn checkpoint.
    pub fn transact<R>(
        &mut self,
        why: Option<String>,
        f: impl FnOnce(&mut Store, ara_ops::TxnMeta) -> R,
    ) -> Result<R, Vec<Diagnostic>> {
        let session = self.session.clone().ok_or_else(|| {
            vec![Diagnostic::error(
                codes::SESSION_NOT_FOUND,
                "edits need a session",
            )]
        })?;
        let author = self
            .st
            .sessions
            .live(&session)
            .map(|s| s.author.clone())
            .ok_or_else(|| {
                vec![Diagnostic::error(
                    codes::SESSION_NOT_FOUND,
                    format!("no live session `{session}`"),
                )]
            })?;
        let agent = matches!(author, ara_ops::Author::Agent(_));
        let mut meta = ara_ops::TxnMeta::by(author);
        meta.why = why.clone();
        let before = self.st.store.revision();
        let result = f(&mut self.st.store, meta);
        if agent && self.st.store.revision() != before {
            crate::core::checkpoint_at(self.st, &session, before, why, true);
        }
        Ok(result)
    }

    /// Publishes the commands `source` currently provides (an ara-cmd catalog object:
    /// name to `Info`-shaped entry), replacing its previous set. `schema.list` and
    /// `schema.describe` show them with the registry's commands, so agents and the
    /// editor see one catalog (C# play commands: `source` is `play`).
    pub fn publish_commands(&mut self, source: &str, catalog: &Value) {
        self.st.external.retain(|_, (from, _)| from != source);
        if let Some(entries) = catalog.as_object() {
            for (name, entry) in entries {
                self.st
                    .external
                    .insert(name.clone(), (source.to_owned(), entry.clone()));
            }
        }
    }

    /// Records a diagnostic on the bus (`logs`, the `diagnostics` topic).
    pub fn log(&mut self, d: Diagnostic) {
        self.st.log(d);
    }

    /// Sends a notification to every connection subscribed to `topic`.
    pub fn notify(&mut self, topic: Topic, payload: Value) {
        self.st.notify(topic, payload);
    }

    /// Starts a job owned by the caller; returns its ID.
    pub fn start_job(&mut self, name: &str) -> String {
        self.st.start_job(name, self.session.clone())
    }

    /// Reports a running job's progress (`[0, 1]`).
    pub fn job_progress(&mut self, job: &str, fraction: f32, message: &str) {
        self.st.job_progress(job, fraction, message);
    }

    /// Finishes a running job.
    pub fn finish_job(&mut self, job: &str, result: Result<Value, Vec<Diagnostic>>) {
        self.st.finish_job(job, result);
    }

    /// True if the job was cancelled; the service should stop working on it.
    pub fn job_cancelled(&self, job: &str) -> bool {
        self.st
            .jobs
            .get(job)
            .is_some_and(|j| j.status == crate::jobs::JobStatus::Cancelled)
    }
}
