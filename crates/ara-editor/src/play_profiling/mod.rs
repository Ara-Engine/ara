//! Profiling a running Play player: the typed port (`Backend`: its targets, Start, Stop,
//! Snapshot and Freeze requests, and their status) and its adapter over the Play
//! service's mailbox (`attach_play`), owned by the `Editor` (`Editor::play_profiling`).
mod play;
pub(crate) use play::attach as attach_play;

use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};

/// Exact live-world/player incarnation. Player number alone is never an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    /// Live Play world ID.
    pub world: String,
    /// Rendering player index in that world.
    pub player: u32,
    /// Opaque process/world producer incarnation.
    pub source: String,
    /// Monotonic binding within that producer slot.
    pub binding: u64,
}
impl Scope {
    /// The target in words: its world, player, source and binding.
    pub(crate) fn label(&self) -> String {
        format!(
            "{} · player {} · source {} · binding {}",
            self.world, self.player, self.source, self.binding
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub target: Scope,
    pub name: String,
    pub process_id: Option<u32>,
    pub recording: bool,
    pub available: bool,
    pub reason: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Start,
    Stop,
    Snapshot,
}
pub(crate) struct Request {
    pub session: String,
    pub target: Scope,
    pub action: Action,
    pub why: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ticket(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Completed,
    Unchanged,
    Failed,
    Refused,
}
pub(crate) struct Status {
    pub finished: bool,
    pub outcome: Option<Outcome>,
    pub recording: bool,
    pub profile: Option<ara_profile::Profile>,
    pub diagnostics: Vec<Diagnostic>,
}
/// All methods are bounded handle operations; implementations own any asynchronous transport.
/// No default/fake implementation is provided. None means unavailable player profiling.
pub(crate) trait Backend: Send {
    fn refresh_targets(&mut self) -> Result<(), Vec<Diagnostic>>;
    fn targets(&self) -> &[Row];
    fn request(&mut self, request: Request) -> Result<Ticket, Vec<Diagnostic>>;
    fn poll(&mut self, ticket: Ticket) -> Option<Status>;
}
