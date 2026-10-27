//! The Engine Protocol (decision A5) wire format, shared by the host, transports, the
//! MCP adapter and future clients (the editor).
//!
//! - [`jsonrpc`]: JSON-RPC 2.0 requests, responses, notifications and [`RpcError`],
//!   whose `data.diagnostics` always carries `ara-diag` diagnostics.
//! - [`Scope`]: the capability vocabulary sessions are granted.
//! - [`MethodInfo`]: one catalog entry (`rpc.discover`), with its params JSON Schema
//!   and the MCP tool it backs.
//! - [`Endpoint`]: anything that answers method calls (the in-process host, a
//!   WebSocket client).
//! - [`lines`]: newline-delimited JSON framing for stdio.
//!
//! No IO beyond `std::io` traits; transports live in `ara-rpc`.

mod catalog;
pub mod jsonrpc;
pub mod lines;
mod scope;

pub use catalog::{MethodHints, MethodInfo, ToolBinding};
pub use jsonrpc::{Incoming, RpcError};
pub use scope::Scope;

use serde_json::Value;

/// Protocol name and version, reported by `rpc.discover` and `daemon.json`.
pub const PROTOCOL: &str = "ara-ep/1";

/// A connection to the host, assigned by the transport. Notifications are routed by
/// it. [`ConnId::LOCAL`] is the in-process caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(pub u64);

impl ConnId {
    /// The in-process caller (offline CLI, tests).
    pub const LOCAL: ConnId = ConnId(0);
}

/// Something that answers Engine Protocol calls: the in-process host or a client
/// connected to a daemon. The MCP adapter and the CLI work against this trait.
pub trait Endpoint {
    /// Calls `method` with `params` (an object) and waits for the result.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError>;
    /// Advance local asynchronous work while a caller awaits a preparation job.
    /// Remote endpoints are advanced by their server and keep the default no-op.
    fn tick(&mut self) {}
}
