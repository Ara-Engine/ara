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

    /// Establish an empty document identity for a subsystem's create command.
    /// Semantic records must subsequently be added through `apply`, so initialization
    /// participates in attribution, review and undo like every later edit.
    pub fn create_document(&mut self, path: &str, kind: &str) -> Result<String, Diagnostic> {
        use std::path::Component;
        let relative = Path::new(path);
        if path.contains(':')
            || path.contains('\\')
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "document path must stay inside the project",
            ));
        }
        let prefix = self
            .st
            .store
            .schema()
            .kinds()
            .find(|k| k.name == kind)
            .map(|k| k.id_prefix.clone())
            .ok_or_else(|| Diagnostic::error(codes::INVALID_PARAMS, "unknown document kind"))?;
        let target = self.st.root.join(relative);
        if target.exists() || self.st.store.workspace().find(path).is_some() {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "document already exists",
            ));
        }
        let parent = target
            .parent()
            .ok_or_else(|| Diagnostic::error(codes::INVALID_PARAMS, "document has no parent"))?;
        let ancestor = parent.ancestors().find(|p| p.exists()).ok_or_else(|| {
            Diagnostic::error(codes::INVALID_PARAMS, "document has no existing ancestor")
        })?;
        let ancestor = ancestor
            .canonicalize()
            .map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?;
        if !ancestor.starts_with(&self.st.root) {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "document parent escapes the project",
            ));
        }
        std::fs::create_dir_all(parent).map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?;
        let resolved = parent
            .canonicalize()
            .map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?;
        if !resolved.starts_with(&self.st.root) {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "document parent escapes the project",
            ));
        }
        let id = self.st.store.ids().asset_id(&prefix);
        let doc = ara_doc::Document::new(kind, id);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?;
        file.write_all(ara_doc::write(&doc, self.st.store.schema()).as_bytes())
            .map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?;
        let id = doc.id.to_string();
        let source = crate::project::asset_path(path, &doc.kind);
        self.st.store.load_doc_with_asset_path(path, doc, source);
        Ok(id)
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
