//! User-level Ara Hub backend. The native UI and global MCP share this JSON-RPC API.
mod isolation;
mod registry;
mod service;
pub mod startup;
mod templates;

pub use isolation::validate_development_config;
pub use registry::{EngineBuild, ProjectEntry, Registry};
pub use service::{
    ConnectedService, EngineRegistration, Hub, HubConfig, UpdateConfig, connect_checked,
    connect_or_start, serve, update_installed_engine,
};
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
    let directory = std::env::var_os("ARA_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(user_config_dir)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot find the user config directory; set ARA_CONFIG_DIR",
            )
        })?;
    validate_development_config(&directory)?;
    Ok(directory)
}

fn user_config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(path).join("Ara"));
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return Some(PathBuf::from(path).join("Library/Application Support/Ara"));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(path).join("ara"));
    }
    if let Some(path) = std::env::var_os("HOME") {
        return Some(PathBuf::from(path).join(".config/ara"));
    }
    None
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
