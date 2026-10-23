//! Authenticated dev-build observability over WebSocket/TCP, including adb forwards.
//! Publish owned telemetry batches without blocking the frame or tick thread.

#![warn(missing_docs)]
mod device;
mod protocol;
mod server;

pub use device::{Adb, Device, DeviceClient};
pub use protocol::*;
pub use server::{Publisher, Server};

/// Transport, protocol or runtime integration failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// TCP or process failure.
    #[error("ARA-EXP-BRG-001: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed or closed WebSocket.
    #[error("ARA-EXP-BRG-002: {0}")]
    WebSocket(#[from] tungstenite::Error),
    /// Invalid JSON payload.
    #[error("ARA-EXP-BRG-003: {0}")]
    Json(#[from] serde_json::Error),
    /// Typed runtime or device diagnostic.
    #[error("{0:?}")]
    Diagnostic(BridgeError),
}
