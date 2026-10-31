use crate::{STORAGE, Settings};
use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    settings: Settings,
}

/// A per-game user settings file. No filesystem work occurs until load/save.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    future: bool,
}

impl Store {
    /// An explicit isolated path; useful for tools and tests.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            future: false,
        }
    }

    /// Resolve the engine user config root and an opaque stable game identity.
    /// User/project text can never escape the per-game directory.
    pub fn for_game(game: &str) -> Result<Self, Diagnostic> {
        let root = std::env::var_os("ARA_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(user_root)
            .ok_or_else(|| {
                Diagnostic::error(STORAGE, "cannot locate user config; set ARA_CONFIG_DIR")
            })?;
        Ok(Self::at(
            root.join("games")
                .join(blake3::hash(game.as_bytes()).to_hex().as_str())
                .join("graphics.json"),
        ))
    }

    /// The resolved file, for diagnostics and tests.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load strict bounded JSON. Missing files keep project defaults. Unknown future
    /// versions are preserved and disable Save until a newer engine reads them.
    pub fn load(&mut self, defaults: &Settings) -> (Settings, Vec<Diagnostic>) {
        let result = (|| {
            let file = match std::fs::File::open(&self.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                result => result.map_err(|e| e.to_string())?,
            };
            let mut text = String::new();
            file.take(MAX_BYTES + 1)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            if text.len() as u64 > MAX_BYTES {
                return Err("settings file exceeds 64 KiB".to_owned());
            }
            let value = ara_json::parse(&text).map_err(|e| e.to_string())?;
            let version = value
                .get("version")
                .and_then(|v| v.as_u64())
                .ok_or("missing settings version")?;
            if version != u64::from(VERSION) {
                self.future = version > u64::from(VERSION);
                return Err(format!(
                    "unsupported graphics settings version {version}; file retained"
                ));
            }
            let file: File = serde_json::from_value(value).map_err(|e| e.to_string())?;
            Ok(Some(file.settings))
        })();
        match result {
            Ok(Some(mut settings)) => {
                let diagnostics = settings.validate();
                (settings, diagnostics)
            }
            Ok(None) => (defaults.clone(), Vec::new()),
            Err(error) => (defaults.clone(), vec![self.error(error)]),
        }
    }

    /// Atomically replace the file after syncing the new contents. A failed write
    /// leaves the last good file intact. Never overwrite a future file version.
    pub fn save(&self, settings: &Settings) -> Result<(), Diagnostic> {
        if self.future {
            return Err(self.error("a newer engine owns this file; refusing to overwrite"));
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let parent = self
                .path
                .parent()
                .ok_or("settings file has no parent directory")?;
            std::fs::create_dir_all(parent)?;
            let value = ara_json::to_value(&File {
                version: VERSION,
                settings: settings.clone(),
            })?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(ara_json::to_canonical_string(&value).as_bytes())?;
            file.as_file().sync_all()?;
            file.persist(&self.path)?;
            Ok(())
        })();
        result.map_err(|e| self.error(e.to_string()))
    }

    fn error(&self, message: impl std::fmt::Display) -> Diagnostic {
        Diagnostic::error(STORAGE, format!("graphics settings: {message}"))
            .file(self.path.to_string_lossy())
    }
}

fn user_root() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Ara"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support/Ara"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .map(|p| p.join("ara"))
    }
}
