//! Read-only import state for presentation; players never schedule import work.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Owner notification describing registered sources whose latest import is pending.
/// Failed and unregistered sources are absent. Existing published generations stay
/// visible during reimports; pending bounds are only used before the first cook.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportState {
    /// Monotonic owner progress revision, independent of published generations.
    pub revision: u64,
    /// Stable asset IDs still importing or queued for import.
    pub pending: BTreeSet<String>,
}

/// Internal fault-isolation workload: block simulation while IO keeps heartbeating.
#[derive(Debug, Serialize, Deserialize)]
pub struct Stall {
    /// Wall-clock stall duration, limited by the player to thirty seconds.
    pub millis: u64,
}
