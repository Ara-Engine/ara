//! The `core` service: every method the host itself provides. It is registered
//! through the same [`Service`] trait as any other subsystem.
//!
//! | Methods | MCP tool |
//! |---|---|
//! | `rpc.discover` | (adapter metadata) |
//! | `session.begin/status/diff/checkpoint/rollback/end`, `history.list/undo/redo`, `review.list/revert` | `ara_session` (`action`) |
//! | `review.accept` (humans only), `subscribe`, `unsubscribe`, `doc.sync`, `daemon.shutdown` | none |
//! | `schema.list/describe/search` | `ara_schema` (`action`) |
//! | `query` | `ara_query` |
//! | `inspect` | `ara_inspect` |
//! | `cmd.apply` | `ara_apply` |
//! | `logs` | `ara_logs` |
//! | `docs.search` | `ara_docs` |
//! | `jobs.list/status/start/cancel` | `ara_jobs` (`action`) |

mod edit;
mod pick;
mod read;
mod session;

use ara_diag::Diagnostic;
use ara_proto::Scope;
use ara_schema::SchemaDb;
use serde_json::Value;

pub(crate) use edit::list_commands;

use crate::params::*;
use crate::service::{Cx, Method, Service, parse};

/// The host's own methods.
pub(crate) struct Core;

/// The core method table. Also used offline (`ara docs --llms`) through
/// [`crate::core_methods`].
pub(crate) fn methods(db: &mut SchemaDb) -> Vec<Method> {
    let s = "ara_session";
    vec![
        Method::new::<DiscoverParams>(db, "rpc.discover", "Protocol version, scopes and every method with its params JSON Schema.")
            .sessionless()
            .read_only(),
        Method::new::<SessionBeginParams>(db, "session.begin", "Begin a session: returns the handle every other call takes, and the scopes granted.")
            .sessionless()
            .action(s, "begin"),
        Method::new::<SessionParams>(db, "session.status", "The session's scopes, turns, unreviewed changes, checkpoints and unsaved documents.")
            .read_only()
            .action(s, "status"),
        Method::new::<SessionDiffParams>(db, "session.diff", "What the session changed since a checkpoint, per entity, with was → now per field.")
            .read_only()
            .action(s, "diff"),
        Method::new::<CheckpointParams>(db, "session.checkpoint", "Mark the current revision to roll back to. Agent turns get one automatically.")
            .action(s, "checkpoint"),
        Method::new::<RollbackParams>(db, "session.rollback", "Undo every transaction of this session after a checkpoint, all or nothing.")
            .scope(Scope::EditScene)
            .destructive()
            .action(s, "rollback"),
        Method::new::<SessionParams>(db, "session.end", "End the session; its handle stops working.")
            .action(s, "end"),
        Method::new::<HistoryListParams>(db, "history.list", "Transactions newest first: author, why, revision, review status and touched entities.")
            .read_only()
            .action(s, "history"),
        Method::new::<UndoParams>(db, "history.undo", "Undo the author's latest transaction, or a given one, as a new transaction.")
            .scope(Scope::EditScene)
            .action(s, "undo"),
        Method::new::<SessionParams>(db, "history.redo", "Redo the author's most recently undone transaction.")
            .scope(Scope::EditScene)
            .action(s, "redo"),
        Method::new::<ReviewListParams>(db, "review.list", "Agent transactions by review status (default: unreviewed), with per-entity status.")
            .read_only()
            .action(s, "review"),
        Method::new::<ReviewParams>(db, "review.accept", "Accept agent changes per transaction, per entity or all. Human sessions only.")
            .scope(Scope::EditScene),
        Method::new::<ReviewParams>(db, "review.revert", "Revert agent changes per transaction or per entity (inverse ops, conflict-checked).")
            .scope(Scope::EditScene)
            .destructive()
            .action(s, "revert"),
        Method::new::<SchemaListParams>(db, "schema.list", "List components, types, commands, queries, jobs and methods (names and one-line summaries).")
            .read_only()
            .action("ara_schema", "list"),
        Method::new::<SchemaDescribeParams>(db, "schema.describe", "Full description of types, commands or methods, with JSON Schemas and examples.")
            .read_only()
            .action("ara_schema", "describe"),
        Method::new::<SchemaSearchParams>(db, "schema.search", "Search components, fields, commands and methods by words in their names and docs.")
            .read_only()
            .action("ara_schema", "search"),
        Method::new::<QueryParams>(db, "query", "Find entities (or documents) with the compact query syntax or its JSON form; paginated, returns handles.")
            .read_only()
            .tool("ara_query"),
        Method::new::<InspectParams>(db, "inspect", "A document's hierarchy as scene-to-text, or entities' full component state.")
            .read_only()
            .tool("ara_inspect"),
        Method::new::<ApplyParams>(db, "cmd.apply", "Run a batch of commands as one atomic, reviewable transaction; dryRun returns the diff.")
            .tool("ara_apply"),
        Method::new::<LogsParams>(db, "logs", "Diagnostics from every subsystem, grouped by code with counts, subjects and fixes; filter by code, severity, subject or origin, and read new ones with `since`.")
            .read_only()
            .tool("ara_logs"),
        Method::new::<DocsSearchParams>(db, "docs.search", "Search this engine version's generated docs (the same pages as llms.txt).")
            .read_only()
            .tool("ara_docs"),
        Method::new::<SessionParams>(db, "jobs.list", "Every job of this host run with status and progress.")
            .read_only()
            .action("ara_jobs", "list"),
        Method::new::<JobParams>(db, "jobs.status", "One job's status, progress, result or diagnostics.")
            .read_only()
            .action("ara_jobs", "status"),
        Method::new::<JobStartParams>(db, "jobs.start", "Start a registered job on a document; progress streams on the `jobs` topic.")
            .action("ara_jobs", "start"),
        Method::new::<JobParams>(db, "jobs.cancel", "Cancel a running job.")
            .action("ara_jobs", "cancel"),
        Method::new::<SubscribeParams>(db, "subscribe", "Stream notifications on this connection: ops, diagnostics, history, jobs."),
        Method::new::<SubscribeParams>(db, "unsubscribe", "Stop streaming topics on this connection."),
        Method::new::<DocSyncParams>(db, "doc.sync", "Resolve a document whose file diverged: keep memory (overwrite the file) or disk (load it).")
            .scope(Scope::EditScene),
        Method::new::<SessionParams>(db, "daemon.shutdown", "Flush everything and stop the daemon. Human sessions only."),
    ]
}

impl Service for Core {
    fn name(&self) -> &'static str {
        "core"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        methods(db)
    }

    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        let st = &mut *cx.st;
        let sid = cx.session.clone().unwrap_or_default();
        match method {
            "rpc.discover" => Ok(read::discover(st)),
            "session.begin" => session::begin(st, parse(params)?),
            "session.status" => session::status(st, &sid),
            "session.diff" => session::diff(st, &sid, parse(params)?),
            "session.checkpoint" => {
                let p: CheckpointParams = parse(params)?;
                Ok(session::checkpoint(st, &sid, p.label, false))
            }
            "session.rollback" => session::rollback(st, &sid, parse(params)?),
            "session.end" => session::end(st, &sid),
            "history.list" => session::history(st, parse(params)?),
            "history.undo" => session::undo(st, &sid, parse(params)?),
            "history.redo" => session::redo(st, &sid),
            "review.list" => session::review_list(st, parse(params)?),
            "review.accept" => session::accept(st, &sid, parse(params)?),
            "review.revert" => session::revert(st, &sid, parse(params)?),
            "schema.list" => read::schema_list(st, parse(params)?),
            "schema.describe" => read::schema_describe(st, parse(params)?),
            "schema.search" => read::schema_search(st, parse(params)?),
            "query" => read::query(st, parse(params)?),
            "inspect" => read::inspect(st, parse(params)?),
            "logs" => crate::diag::logs(st, parse(params)?),
            "docs.search" => read::docs_search(st, parse(params)?),
            "cmd.apply" => st.apply(&sid, parse(params)?),
            "jobs.list" => Ok(edit::jobs_list(st)),
            "jobs.status" => edit::job_status(st, parse(params)?),
            "jobs.start" => edit::job_start(st, &sid, parse(params)?),
            "jobs.cancel" => edit::job_cancel(st, parse(params)?),
            "subscribe" => Ok(edit::subscribe(st, cx.conn, parse(params)?, true)),
            "unsubscribe" => Ok(edit::subscribe(st, cx.conn, parse(params)?, false)),
            "doc.sync" => edit::doc_sync(st, parse(params)?),
            "daemon.shutdown" => edit::shutdown(st, &sid),
            other => Err(vec![Diagnostic::error(
                ara_diag::codes::UNKNOWN_METHOD,
                format!("core does not handle `{other}`"),
            )]),
        }
    }
}
