//! `.ara/daemon.json` (how clients find a running daemon) and `.ara/token` (the
//! per-project secret every connection presents). Both live in the gitignored
//! `.ara/` state directory.

use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A running daemon, as written to `.ara/daemon.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// Process ID.
    pub pid: u32,
    /// Port on `127.0.0.1`.
    pub port: u16,
    /// The project token to present.
    pub token: String,
    /// Protocol version (`ara-ep/1`).
    pub protocol: String,
}

fn state(root: &Path) -> PathBuf {
    root.join(".ara")
}

impl DaemonInfo {
    /// `<root>/.ara/daemon.json`.
    pub fn path(root: &Path) -> PathBuf {
        state(root).join("daemon.json")
    }

    /// The daemon file, if present and readable.
    pub fn read(root: &Path) -> Option<DaemonInfo> {
        let text = fs::read_to_string(Self::path(root)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes the daemon file.
    pub fn write(&self, root: &Path) -> io::Result<()> {
        fs::create_dir_all(state(root))?;
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(Self::path(root), text + "\n")
    }

    /// Removes the daemon file if it still names this process.
    pub fn remove(root: &Path) {
        if Self::read(root).is_some_and(|d| d.pid == std::process::id()) {
            let _ = fs::remove_file(Self::path(root));
        }
    }

    /// The WebSocket URL.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/", self.port)
    }
}

/// The project's token (`.ara/token`), created on first use: 128 random bits as hex.
pub fn project_token(root: &Path) -> io::Result<String> {
    let path = state(root).join("token");
    if let Ok(t) = fs::read_to_string(&path) {
        let t = t.trim();
        if t.len() >= 16 {
            return Ok(t.to_owned());
        }
    }
    let seed = (
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    // RandomState is keyed from OS randomness per instance.
    let token = format!(
        "{:016x}{:016x}",
        RandomState::new().hash_one(seed),
        RandomState::new().hash_one(seed)
    );
    fs::create_dir_all(state(root))?;
    fs::write(&path, &token)?;
    Ok(token)
}
