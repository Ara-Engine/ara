use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};

/// Explicit recovery of one player's scripts, or all players of a live world.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRecoveryRequest {
    /// Requesting editor session.
    pub session: String,
    /// Selected live Play world.
    pub world: String,
    /// Player index; omitted selects every current player.
    pub player: Option<u32>,
    /// Optional explanation for the control action.
    pub why: Option<String>,
}
/// Handle-local query token; unknown tickets never imply success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScriptRecoveryTicket(pub u64);
/// Authoritative result of activation at the player's boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptRecoveryOutcome {
    /// A verified image was activated, clearing its transient faults.
    Reloaded,
    /// No activation changed.
    Unchanged,
    /// Compilation or activation failed, retaining last-good code and fault state.
    Failed,
    /// The requested incarnation is no longer eligible.
    Refused,
}
/// Recovery result attributed to the actual selected player incarnation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRecoveryPlayerResult {
    /// World-local player index.
    pub player: u32,
    /// J10 process incarnation source.
    pub source: String,
    /// J10 Play binding generation.
    pub binding: u64,
    /// Activation result; authoritative when the enclosing status is finished.
    pub outcome: ScriptRecoveryOutcome,
    /// Modules included in the verified activation.
    pub modules: Vec<String>,
    /// Original scoped diagnostics for this attempt.
    pub diagnostics: Vec<Diagnostic>,
}
/// Owned progress snapshot; polling never waits for compilation or a player.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRecoveryStatus {
    /// Request query token.
    pub ticket: ScriptRecoveryTicket,
    /// Selected world.
    pub world: String,
    /// Every selected incarnation has reached a terminal result.
    pub finished: bool,
    /// Per-incarnation results.
    pub players: Vec<ScriptRecoveryPlayerResult>,
    /// Request-level refusal or transport diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}
