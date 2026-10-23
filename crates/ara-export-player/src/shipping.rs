//! Local crash capture installed before loading gameplay code.
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Explicit fault injection for a separately launched validation child.
#[derive(Debug, Clone, Copy)]
pub enum CrashProbe {
    /// Native exception/signal after loading the managed library.
    Native,
    /// Rust panic after loading the managed library.
    Panic,
    /// Unhandled managed background-thread exception from the generated hook.
    Managed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    format: String,
    build_id: String,
    settings: ara_crash::CrashSettings,
}

/// Starts a build-scoped crash monitor. Older template layouts without crash
/// configuration retain their previous behavior; malformed configuration fails.
pub fn start(root: &Path) -> Result<Option<ara_crash::Capture>, String> {
    let path = root.join("ara-crash.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let config: Config = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if config.format != "ara.crash.config/1"
        || config.build_id.len() != 64
        || !config.build_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("ARA-EXP-CRASH-001: invalid build crash configuration".into());
    }
    config.settings.validate().map_err(|e| e.to_string())?;
    let directory = directory(&config.build_id)?;
    ara_crash::Capture::start(&directory, &config.build_id)
        .map(Some)
        .map_err(|e| e.to_string())
}

fn directory(build_id: &str) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("ARA_CRASH_DIR") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("LOCALAPPDATA missing for crash storage")?;
    #[cfg(not(windows))]
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or("XDG_STATE_HOME/HOME missing for crash storage")?;
    Ok(root.join("Ara/crashes").join(build_id))
}
