//! Params of the core Engine Protocol methods. Each derives `Schema`, so its doc
//! comments become the method's JSON Schema (`rpc.discover`), the MCP tool
//! descriptions and the generated docs. Field names are camelCase on the wire.

use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Params of `rpc.discover`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(deny_unknown_fields)]
pub struct DiscoverParams {}

/// Params of `session.begin`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionBeginParams {
    /// Who is connecting: `claude-code`, `codex`, `editor`, `ara-cli`.
    pub client: String,
    /// Transaction author: `human` or `agent:<name>`. Defaults to `agent:<client>`.
    pub author: Option<String>,
    /// Scopes to request: `read`, `edit:scene`, `edit:assets`, `edit:code`, `play`,
    /// `run`, `import`, `bake`, `build`, `generate:$`, `fs:outside-project`, `net`.
    /// Defaults to everything the approval policy allows. The policy may grant fewer.
    pub scopes: Option<Vec<String>>,
    /// Short description of the work, shown in review.
    pub label: Option<String>,
}

/// Params of methods that only need the session (`session.status`, `session.end`,
/// `history.redo`, `jobs.list`, `daemon.shutdown`).
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionParams {
    /// Session handle from `session.begin`.
    pub session: String,
}

/// Params of `session.diff`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDiffParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Show changes since this checkpoint (`cp_...`); default: the session's start.
    pub checkpoint: Option<String>,
}

/// Params of `session.checkpoint`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// What the checkpoint marks.
    pub label: Option<String>,
}

/// Params of `session.rollback`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollbackParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Checkpoint (`cp_...`) to return to. Every later transaction of this session is
    /// undone, all or nothing. Agent sessions get one checkpoint per turn.
    pub to: String,
}

/// What `schema.list` and `schema.search` look through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SchemaKind {
    /// Entity components.
    Component,
    /// Document record types.
    Record,
    /// Value structs nested in components.
    Struct,
    /// Enums.
    Enum,
    /// Command params types.
    Params,
    /// Commands (edits).
    Command,
    /// Queries (reads).
    Query,
    /// Jobs (long-running work).
    Job,
    /// Engine Protocol methods.
    Method,
}

/// Params of `schema.list`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaListParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only this kind of entry.
    pub kind: Option<SchemaKind>,
    /// Only types of this module (`core`, `render`).
    pub module: Option<String>,
}

/// Params of `schema.describe`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaDescribeParams {
    /// Type, command or method names (`Transform`, `entity.create`, `cmd.apply`).
    pub names: Vec<String>,
    /// Session handle from `session.begin`.
    pub session: String,
}

/// Params of `schema.search`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaSearchParams {
    /// Words to look for in names and docs (`light intensity`).
    pub query: String,
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only this kind of entry.
    pub kind: Option<SchemaKind>,
    /// Maximum results (default 10).
    pub limit: Option<u32>,
}

/// A field comparison in a query filter.
#[derive(Debug, Clone, PartialEq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Compare {
    /// Field path on the entity (`Light.intensity`, `Transform.position.y`, `name`).
    pub path: String,
    /// Comparison operator.
    pub op: CompareOp,
    /// Value to compare the effective (default-filled) field value with.
    pub value: Value,
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompareOp {
    /// Equal (numbers compare numerically).
    Eq,
    /// Not equal.
    Ne,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
}

/// The JSON form of a query: every set condition must hold. The compact `q` syntax
/// parses into this.
#[derive(Debug, Clone, Default, PartialEq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Filter {
    /// Components the entity must have (`["Light"]`).
    pub has: Vec<String>,
    /// Components the entity must not have.
    pub lacks: Vec<String>,
    /// Name glob, case-insensitive (`Crate*`).
    pub name: Option<String>,
    /// Only direct children of this entity.
    pub parent: Option<String>,
    /// Only descendants of this entity.
    pub under: Option<String>,
    /// Only prefab instances whose prefab path contains this text.
    pub prefab: Option<String>,
    /// Only entities at most this deep (roots are depth 0).
    pub max_depth: Option<u32>,
    /// Only these entity IDs or addresses.
    pub ids: Vec<String>,
    /// Field comparisons.
    #[ara(rename = "where")]
    #[serde(rename = "where")]
    pub compare: Vec<Compare>,
}

/// What a query returns per item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Detail {
    /// Address, name, components and position (the default).
    #[default]
    Summary,
    /// Only handles.
    Handles,
    /// Everything stored on the entity.
    Full,
}

/// What `query` searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QueryKind {
    /// Entities (prefab instances expanded).
    #[default]
    Entities,
    /// Documents and imported assets of the project.
    Documents,
}

/// Params of `query`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Compact query: space-separated terms that must all hold. `Light` (has the
    /// component), `!Light` (lacks it), `name:Crate*`, `parent:e_1c0d`, `under:e_1c0d`,
    /// `prefab:crate`, `depth<=1`, `e_8b21` (an ID), field comparisons such as
    /// `Light.intensity>500`, `Light.kind=spot`, `Transform.position.y<=0`.
    pub q: Option<String>,
    /// The same conditions as JSON; combined with `q`.
    pub filter: Option<Filter>,
    /// Document (asset ID, path or `@path#id`); default: every scene and prefab.
    pub doc: Option<String>,
    /// Search entities (default) or documents.
    pub kind: Option<QueryKind>,
    /// Field paths to return per entity (`Light.intensity`), effective values.
    pub fields: Option<Vec<String>>,
    /// Per-item detail: `summary` (default), `handles` or `full`.
    pub detail: Option<Detail>,
    /// Page size (default 50, max 500).
    pub limit: Option<u32>,
    /// `next` from the previous page.
    pub cursor: Option<String>,
}

/// Output format of `inspect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    /// Token-tuned text (the default).
    #[default]
    Text,
    /// Structured JSON.
    Json,
}

/// Params of `inspect`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectParams {
    /// Document (asset ID, path or `@path#id`).
    pub doc: String,
    /// Session handle from `session.begin`.
    pub session: String,
    /// Entities to show in full (IDs or addresses such as `e_4f9a/e_m4tz`). Absent:
    /// a summary of the document's hierarchy.
    pub entities: Option<Vec<String>>,
    /// Hierarchy depth of the summary (default 2).
    pub depth: Option<u32>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
    /// Summary page size (default 100).
    pub limit: Option<u32>,
    /// `next` from the previous summary page.
    pub cursor: Option<String>,
}

/// One call in an `ara_apply` / `cmd.apply` batch.
#[derive(Debug, Clone, Schema, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallSpec {
    /// Command name (`entity.create`); find them with `schema.search`.
    pub cmd: String,
    /// Command arguments, per the command's params schema (`schema.describe`).
    pub args: Option<Value>,
    /// Symbolic ID (`$crate`) for the record this call creates; later calls may use
    /// it anywhere an entity is expected. The result maps it to the real ID.
    #[ara(rename = "as")]
    #[serde(rename = "as")]
    pub symbol: Option<String>,
}

/// Params of `cmd.apply`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyParams {
    /// Calls, run in order as one atomic transaction.
    pub commands: Vec<CallSpec>,
    /// Document to edit (asset ID, path or `@path#id`).
    pub doc: String,
    /// Session handle from `session.begin`.
    pub session: String,
    /// Why, in a sentence; shown in review next to the change.
    pub why: Option<String>,
    /// Validate and return the diff without applying anything.
    pub dry_run: Option<bool>,
    /// Revision the batch was planned against; fails if later edits touched the same
    /// fields.
    pub base: Option<u64>,
    /// Retrying with the same key returns the first result instead of applying twice.
    pub idempotency_key: Option<String>,
}

/// Params of `history.list`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryListParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only transactions touching this document.
    pub doc: Option<String>,
    /// Only this author (`human`, `agent:claude`).
    pub author: Option<String>,
    /// Only transactions after this revision.
    pub since: Option<u64>,
    /// Maximum transactions, newest first (default 20).
    pub limit: Option<u32>,
}

/// Params of `history.undo`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Transaction to undo (`tx_...`); default: the session author's latest.
    pub txn: Option<String>,
}

/// Review state of a transaction or entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewStatus {
    /// Applied live, not reviewed yet.
    Applied,
    /// Kept by a reviewer.
    Accepted,
    /// Undone.
    Reverted,
}

/// Params of `review.list`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewListParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only transactions with this status (default: `applied`).
    pub status: Option<ReviewStatus>,
    /// Maximum transactions, newest first (default 50).
    pub limit: Option<u32>,
}

/// Params of `review.accept` and `review.revert`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Transaction (`tx_...`). With `entity`, only that entity's changes in it.
    pub txn: Option<String>,
    /// Entity (`e_8b21`, or `sc_arena001/e_8b21` to name the document). Without
    /// `txn`: its changes in every unreviewed transaction.
    pub entity: Option<String>,
    /// Every unreviewed transaction.
    pub all: Option<bool>,
}

/// A diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SeverityParam {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warning,
    /// Everything.
    Info,
}

/// Params of `logs`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogsParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only entries newer than this cursor (`cursor` of a previous call).
    pub since: Option<u64>,
    /// Only these codes (`ARA1101`).
    pub codes: Option<Vec<String>>,
    /// Minimum severity (default `info`).
    pub severity: Option<SeverityParam>,
    /// Only entries about this entity (record ID or address prefix).
    pub entity: Option<String>,
    /// Maximum entries (default 50).
    pub limit: Option<u32>,
}

/// Params of `docs.search`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocsSearchParams {
    /// Words to look for (`prefab override`).
    pub query: String,
    /// Session handle from `session.begin`.
    pub session: String,
    /// Maximum results (default 5).
    pub limit: Option<u32>,
    /// Return each page's full markdown, not just a snippet.
    pub full: Option<bool>,
}

/// Params of `jobs.start`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobStartParams {
    /// Document the job works on.
    pub doc: String,
    /// Job name from the registry (`schema.list {kind: "job"}`).
    pub job: String,
    /// Session handle from `session.begin`.
    pub session: String,
    /// Job arguments, per its params schema.
    pub args: Option<Value>,
    /// Why, shown in review.
    pub why: Option<String>,
}

/// Params of `jobs.status` and `jobs.cancel`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobParams {
    /// Job ID (`jb_...`).
    pub job: String,
    /// Session handle from `session.begin`.
    pub session: String,
}

/// A notification stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Topic {
    /// Every committed transaction with its ops (`ops`).
    Ops,
    /// New and repeated log entries (`diagnostics`).
    Diagnostics,
    /// Transaction summaries and review status changes (`history`).
    History,
    /// Job progress (`jobs`).
    Jobs,
}

impl Topic {
    /// The notification method name.
    pub fn method(self) -> &'static str {
        match self {
            Topic::Ops => "ops",
            Topic::Diagnostics => "diagnostics",
            Topic::History => "history",
            Topic::Jobs => "jobs",
        }
    }
}

/// Params of `subscribe` and `unsubscribe`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscribeParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Streams; notifications arrive on this connection as methods of the same name.
    pub topics: Vec<Topic>,
}

/// Which side wins in `doc.sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Keep {
    /// Write the engine's state over the file.
    Memory,
    /// Load the file, as an edit on top of the engine's state.
    Disk,
}

/// Params of `doc.sync`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocSyncParams {
    /// Document whose file diverged (asset ID or path).
    pub doc: String,
    /// Which side wins.
    pub keep: Keep,
    /// Session handle from `session.begin`.
    pub session: String,
}
