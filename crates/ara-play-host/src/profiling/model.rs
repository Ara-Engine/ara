use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};

/// Exact live J10 incarnation selected for player profiling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayProfileTarget {
    /// Play world ID.
    pub world: String,
    /// World-local player index.
    pub player: u32,
    /// Trusted process-incarnation source.
    pub source: String,
    /// Trusted Play binding generation.
    pub binding: u64,
}
/// Owned target inventory, retaining unavailable incarnations after exit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayProfileTargetRow {
    /// Exact selected incarnation.
    pub target: PlayProfileTarget,
    /// Display name including the actual scene and role.
    pub name: String,
    /// Actual player's OS process ID.
    pub process_id: u32,
    /// Authoritative recorder state from this player.
    pub recording: bool,
    /// Whether this incarnation can accept profiling controls.
    pub available: bool,
    /// Explicit unavailable reason, including exited incarnations.
    pub reason: Option<String>,
}
/// Private player recorder controls; none mutate simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayProfileAction {
    /// Start retaining CPU frames and available GPU samples.
    Start,
    /// Stop retaining new frames.
    Stop,
    /// Copy completed frames and keep the current recording state.
    Snapshot,
    /// Copy completed frames and stop recording.
    Freeze,
}
/// Scoped native-editor profiling request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayProfileRequest {
    /// Requesting editor session.
    pub session: String,
    /// Exact selected incarnation; stale selections are refused.
    pub target: PlayProfileTarget,
    /// Recorder control action.
    pub action: PlayProfileAction,
    /// Optional explanation.
    pub why: Option<String>,
}
/// Owned query token; unknown tickets never imply completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlayProfileTicket(pub u64);
/// Authoritative private control result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayProfileOutcome {
    /// The selected action completed.
    Completed,
    /// Start was already recording, or Stop was already idle.
    Unchanged,
    /// The recorder or transport failed.
    Failed,
    /// The selected incarnation is unavailable or stale.
    Refused,
}
/// Owned nonblocking result snapshot from the selected player's recorder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayProfileStatus {
    /// Request query token.
    pub ticket: PlayProfileTicket,
    /// Original selected incarnation; never silently retargeted.
    pub target: PlayProfileTarget,
    /// Whether a terminal result has been published.
    pub finished: bool,
    /// Terminal outcome; absent while pending.
    pub outcome: Option<PlayProfileOutcome>,
    /// Actual selected recorder's state.
    pub recording: bool,
    /// Completed frames only, using the normal capture/summary schema.
    pub profile: Option<ara_profile::Profile>,
    /// Backend-published diagnostics; UI displays without re-logging.
    pub diagnostics: Vec<Diagnostic>,
}
