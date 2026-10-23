//! Params of the collaboration methods (`collab.*`, `branch.*`). Doc comments become
//! the JSON Schemas, the MCP descriptions and the generated docs.

use ara_collab::{Activity, Camera};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Params of `collab.join`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Display name (`Mia`); default: the author's name.
    pub name: Option<String>,
    /// Avatar colour `#rrggbb`; default: the next palette colour.
    pub color: Option<String>,
    /// Also return a snapshot of every document (late join of a replica).
    pub snapshot: Option<bool>,
    /// Notification methods to receive; absent receives all collaboration events.
    pub events: Option<Vec<String>>,
}

/// Params of `collab.presence`: the whole presence state (absent fields clear).
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresenceParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// The open document (project path).
    pub doc: Option<String>,
    /// The viewport camera.
    pub camera: Option<Camera>,
    /// Selected entity addresses.
    pub selection: Option<Vec<String>>,
    /// World point under the pointer.
    pub pointer: Option<[f32; 3]>,
    /// The current command or turn and its why.
    pub activity: Option<Activity>,
    /// Session handle of the participant whose camera this one follows.
    pub following: Option<String>,
}

/// Params of `collab.claim`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// What to claim: entity IDs or addresses (`e_door01`), `name:Gate Door`, a
    /// component or field of one (`e_door01.Light`), a document path
    /// (`levels/outpost.scene`), or another project file (`@scripts/Gate.cs`).
    pub subjects: Vec<String>,
    /// Document the entities are in; default: the one document holding them.
    pub doc: Option<String>,
    /// Why, shown to everyone (`tuning the gate`).
    pub why: Option<String>,
    /// Lifetime without heartbeats in ms (default 60000; presence updates and
    /// `collab.heartbeat` extend it).
    pub ttl_ms: Option<u64>,
}

/// Params of `collab.release`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Claim IDs (`cl_…`) to release; default: all of yours.
    pub claims: Option<Vec<String>>,
}

/// Params of `collab.submit`: a replica's optimistic change.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// The replica's sequence number (per session, from 1).
    pub seq: u64,
    /// The revision the replica had applied when it made the change.
    pub base: u64,
    /// Edits (`{op: set|create|delete|move, doc, id, …}`), applied as one
    /// transaction.
    pub edits: Vec<Value>,
    /// Why, if given.
    pub why: Option<String>,
    /// Short label (`transform.set`).
    pub label: Option<String>,
}

/// Params of `collab.sync`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Return the op events after this revision.
    pub since: Option<u64>,
    /// Return a snapshot of every document instead.
    pub snapshot: Option<bool>,
}

/// Params of `collab.conflicts`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictsParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Only the conflicts this session lost (default true).
    pub mine: Option<bool>,
}

/// Params of `branch.open`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchOpenParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Why: what the branch is for; it becomes the merge's why.
    pub why: Option<String>,
}

/// Params of `branch.status`, `branch.discard`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Branch ID (`br_…`).
    pub branch: String,
}

/// Params of `branch.submit`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchSubmitParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Branch ID (`br_…`).
    pub branch: String,
    /// Why, if it changed since `branch.open`.
    pub why: Option<String>,
}

/// Which side wins fields both changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreferSide {
    /// Keep the live value (default).
    Live,
    /// Take the branch's value.
    Branch,
}

/// Params of `branch.merge`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchMergeParams {
    /// Session handle from `session.begin` (a human).
    pub session: String,
    /// Branch ID (`br_…`).
    pub branch: String,
    /// Which side wins fields both changed (default `live`).
    pub prefer: Option<PreferSide>,
    /// Legacy compatibility flag; claims never block a merge.
    pub force: Option<bool>,
}
