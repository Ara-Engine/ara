//! Optional multiplayer. Local applications depend on ara-sim alone.
//!
//! Gameplay implements [`ara_sim::SimulationFeature`] and declares Ara schemas;
//! backend registration, encryption, prediction and replication remain private.
mod bounds;
mod conditions;
mod intent;
mod launch;
mod lifecycle;
mod policy;
mod replay;
mod runtime;
mod service;

pub use conditions::{Conditions, DirectionStats, Link};
pub use intent::{CueLedger, Expose, Intent, IntentGate};
pub use launch::{LaunchPlan, PlayerPool, PlayerRole, RunningPlayers, player_worker};
pub use lifecycle::{ConnectionView, DeferredReferences, Interest, NetId, Spawn, WorldIds};
pub use policy::{FieldBytes, FieldId, PolicyRegistry, Projection};
pub use replay::{Divergence, Trace};
pub use runtime::{
    AdapterRef, Authoritative, Client, ClientConfig, ClientStatus, JoinToken, Server, ServerConfig,
};
pub use service::{NetControl, NetService, WorldControl, register};

/// A rejected network configuration, packet, or session operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetError(
    /// Readable diagnostic reason.
    pub String,
);
impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NetError {}
impl From<&NetError> for ara_diag::Diagnostic {
    /// The rejection as a bus diagnostic (`ARA2401`).
    fn from(e: &NetError) -> ara_diag::Diagnostic {
        ara_diag::Diagnostic::error(ara_diag::codes::NET_REJECTED, e.0.clone())
    }
}
impl From<ara_sim::SimError> for NetError {
    fn from(value: ara_sim::SimError) -> Self {
        Self(value.0)
    }
}
impl From<std::io::Error> for NetError {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}
impl From<serde_json::Error> for NetError {
    fn from(value: serde_json::Error) -> Self {
        Self(value.to_string())
    }
}
impl From<postcard::Error> for NetError {
    fn from(value: postcard::Error) -> Self {
        Self(value.to_string())
    }
}
/// Fallible networking result.
pub type Result<T> = std::result::Result<T, NetError>;
pub(crate) fn error(value: impl std::fmt::Display) -> NetError {
    NetError(value.to_string())
}
