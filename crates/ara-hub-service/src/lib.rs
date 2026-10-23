//! User-level Ara Hub backend. The native UI and global MCP share this JSON-RPC API.
mod registry;
mod service;
mod templates;

pub use registry::{EngineBuild, ProjectEntry, Registry};
pub use service::{Hub, HubConfig, connect_or_start, serve};
pub use templates::{DirectoryTemplates, Template, TemplateSource};

use ara_diag::{Diagnostic, codes};
use ara_proto::RpcError;
use std::{
    io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// Engine pin used by the first alpha distribution.
pub const ALPHA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// User config directory; `ARA_CONFIG_DIR` isolates tests and portable installations.
pub fn config_dir() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("ARA_CONFIG_DIR") {
        return Ok(path.into());
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(path).join("Ara"));
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(path).join("Library/Application Support/Ara"));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("ara"));
    }
    if let Some(path) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(path).join(".config/ara"));
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "cannot find the user config directory; set ARA_CONFIG_DIR",
    ))
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX))
}

pub(crate) fn invalid(message: impl Into<String>) -> RpcError {
    Diagnostic::error(codes::INVALID_PARAMS, message).into()
}

pub(crate) fn io_error(error: impl std::fmt::Display) -> RpcError {
    Diagnostic::error(codes::IO, error.to_string()).into()
}
