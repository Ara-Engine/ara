//! Engine Protocol transports (decision A5): everything that moves JSON-RPC text
//! between processes. The engine side is a [`Handler`]; this crate knows nothing
//! about documents.
//!
//! - [`serve`]: one loop owning the handler. WebSocket connections and HTTP requests
//!   arrive on a `127.0.0.1` listener, stdio lines on stdin; every message is
//!   handled on the loop's thread, in arrival order. Idle loops call `tick`.
//! - Every connection must present the project token (`Authorization: Bearer` or
//!   `?token=`); browser origins other than localhost are refused.
//! - [`Client`]: a blocking WebSocket client implementing
//!   [`Endpoint`](ara_proto::Endpoint).
//! - [`DaemonInfo`]: `.ara/daemon.json` (pid, port, token), and the per-project
//!   token in `.ara/token`.

mod client;
mod daemon;
pub mod http;
mod server;
mod transport;

pub use client::Client;
pub use daemon::{DaemonInfo, project_token};
pub use server::{Handler, STDIO, ServeOptions, serve};
