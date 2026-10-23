use crate::{
    DecodedSave, Limits, Result, SaveGame, SaveSchema, SlotMetadata, StateHash, codes, decode,
    encode, error,
};
use ara_diag::{Diagnostic, FixAction};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Trusted host topology; never deserialize authority from an agent/client request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// Single-player/local authority world.
    Local,
    /// Listen/dedicated server's authoritative world.
    Server,
    /// Replica/prediction world; authoritative save mutations are forbidden.
    Client,
}
impl Authority {
    /// Validates permission before capturing/writing/restoring authoritative state.
    pub fn require_write(self) -> Result<()> {
        if self == Self::Client {
            Err(error(
                codes::AUTHORITY,
                "clients cannot write or restore authoritative saves; request it from the server",
            ))
        } else {
            Ok(())
        }
    }
}

/// Slot listing with content identity for optimistic cloud conflict detection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotInfo {
    /// Metadata from the verified envelope, never an unchecked sidecar.
    pub metadata: SlotMetadata,
    /// Whole-file BLAKE3 used as a cloud compare-and-swap token.
    pub file_hash: StateHash,
    /// Persistent projection hash after schema migration.
    pub state_hash: StateHash,
    /// True when the primary was absent/corrupt and the backup supplied this slot.
    pub recovered: bool,
}

/// Loaded and validated slot; restoring it remains a separate tick-boundary operation.
#[derive(Debug)]
pub struct LoadedSlot {
    /// Current-schema game and migration information.
    pub save: DecodedSave,
    /// True when a valid backup replaced an unreadable/corrupt primary logically.
    pub recovered: bool,
    /// Original primary error plus an ARA7505 recovery diagnostic, if applicable.
    pub diagnostics: Vec<Diagnostic>,
}
impl LoadedSlot {
    /// Cloud/listing metadata for this verified generation.
    pub fn info(&self) -> SlotInfo {
        SlotInfo {
            metadata: self.save.game.metadata.clone(),
            file_hash: self.save.file_hash,
            state_hash: self.save.game.state_hash,
            recovered: self.recovered,
        }
    }
}

/// Filesystem boundary for slots. Methods that touch disk run on host workers.
/// Temp files and backups share the destination filesystem; process locks serialize
/// writers and crash-safe replacement preserves at least the last good generation.
pub struct SlotStore {
    root: PathBuf,
    authority: Authority,
    limits: Limits,
}
impl SlotStore {
    /// Configures an isolated save directory; no filesystem work until a slot operation.
    pub fn new(root: impl Into<PathBuf>, authority: Authority, limits: Limits) -> Self {
        Self {
            root: root.into(),
            authority,
            limits,
        }
    }
    /// Trusted host role, independent of agent-supplied command params.
    pub fn authority(&self) -> Authority {
        self.authority
    }
    /// Bounded format options shared by this store's read/write operations.
    pub fn limits(&self) -> Limits {
        self.limits
    }
    /// Save directory; individual slot names cannot escape it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Writes and syncs a temporary envelope, atomically publishes a verified backup,
    /// then atomically replaces the primary. `expected` detects stale cloud/local writes.
    pub fn write(
        &self,
        game: &SaveGame,
        schema: &SaveSchema,
        expected: Option<StateHash>,
    ) -> Result<SlotInfo> {
        self.authority.require_write()?;
        let slot = &game.metadata.slot;
        validate_slot(slot)?;
        schema.validate(&game.state)?;
        if game.versions != schema.versions() {
            return Err(error(
                codes::SCHEMA,
                "only current-schema saves can be written",
            ));
        }
        let bytes = encode(game, self.limits)?;
        fs::create_dir_all(&self.root).map_err(io_error)?;
        let _lock = self.lock(slot)?;
        let previous = self.load(slot, schema);
        if let Err(diagnostic) = &previous
            && diagnostic.code != codes::CORRUPT
            && diagnostic.code != codes::IO
        {
            return Err(diagnostic.clone());
        }
        if let Some(expected) = expected
            && previous.as_ref().map(|s| s.save.file_hash).ok() != Some(expected)
        {
            return Err(error(
                codes::IO,
                "slot changed since read; resolve the cloud/local conflict before retrying",
            ));
        }
        if let Ok(previous) = previous {
            // Never rotate corrupted primary bytes over a healthy backup.
            let path = self.path(slot, previous.recovered);
            let previous_bytes = self.read_bytes(&path)?;
            atomic_replace(&self.root, &self.path(slot, true), &previous_bytes)?;
        }
        atomic_replace(&self.root, &self.path(slot, false), &bytes)?;
        Ok(SlotInfo {
            metadata: game.metadata.clone(),
            file_hash: StateHash::of_bytes(&bytes),
            state_hash: game.state_hash,
            recovered: false,
        })
    }

    /// Validates the primary; if corrupt/missing, returns a verified backup with an
    /// explicit recovery diagnostic. This read never rewrites disk or simulation state.
    pub fn load(&self, slot: &str, schema: &SaveSchema) -> Result<LoadedSlot> {
        validate_slot(slot)?;
        match self.read_game(slot, false, schema) {
            Ok(save) => Ok(LoadedSlot {
                save,
                recovered: false,
                diagnostics: Vec::new(),
            }),
            Err(primary) if primary.code != codes::CORRUPT && primary.code != codes::IO => {
                Err(primary)
            }
            Err(primary) => {
                let save = self.read_game(slot, true, schema).map_err(|backup| {
                    error(
                        codes::CORRUPT,
                        format!("primary: {}; backup: {}", primary.message, backup.message),
                    )
                })?;
                let recovered = Diagnostic::warning(
                    codes::BACKUP,
                    format!("slot {slot} recovered from verified backup"),
                )
                .with_fix(
                    "Restore the verified backup atomically",
                    FixAction::Command {
                        command: "save.restore_backup".into(),
                        params: serde_json::json!({"slot":slot}),
                    },
                );
                Ok(LoadedSlot {
                    save,
                    recovered: true,
                    diagnostics: vec![primary, recovered],
                })
            }
        }
    }

    /// Atomically repairs a corrupt/missing primary from its verified backup while
    /// retaining the backup. Requires server/local authority like ordinary writes.
    pub fn restore_backup(&self, slot: &str, schema: &SaveSchema) -> Result<SlotInfo> {
        self.authority.require_write()?;
        validate_slot(slot)?;
        let _lock = self.lock(slot)?;
        let save = self.read_game(slot, true, schema)?;
        let bytes = self.read_bytes(&self.path(slot, true))?;
        atomic_replace(&self.root, &self.path(slot, false), &bytes)?;
        Ok(SlotInfo {
            metadata: save.game.metadata,
            file_hash: save.file_hash,
            state_hash: save.game.state_hash,
            recovered: true,
        })
    }

    /// Lists valid primary/backup-only slots in stable name order. Invalid slots
    /// carry diagnostics in the output instead of silently disappearing.
    pub fn list(&self, schema: &SaveSchema) -> Result<SlotListing> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SlotListing {
                    slots: Vec::new(),
                    diagnostics: Vec::new(),
                });
            }
            Err(e) => return Err(io_error(e)),
        };
        let mut names = std::collections::BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            if let Some(name) = entry.file_name().to_str().and_then(|s| {
                s.strip_suffix(".ara-save")
                    .or_else(|| s.strip_suffix(".ara-save.bak"))
            }) && validate_slot(name).is_ok()
            {
                names.insert(name.to_string());
            }
        }
        let mut listing = SlotListing {
            slots: Vec::new(),
            diagnostics: Vec::new(),
        };
        for name in names {
            match self.load(&name, schema) {
                Ok(loaded) => {
                    listing.slots.push(loaded.info());
                    listing.diagnostics.extend(loaded.diagnostics);
                }
                Err(d) => listing.diagnostics.push(d.path(name)),
            }
        }
        Ok(listing)
    }

    fn path(&self, slot: &str, backup: bool) -> PathBuf {
        self.root.join(format!(
            "{slot}.ara-save{}",
            if backup { ".bak" } else { "" }
        ))
    }
    fn read_game(&self, slot: &str, backup: bool, schema: &SaveSchema) -> Result<DecodedSave> {
        let save = decode(
            &self.read_bytes(&self.path(slot, backup))?,
            schema,
            self.limits,
        )?;
        if save.game.metadata.slot != slot {
            return Err(error(
                codes::CORRUPT,
                "save metadata slot does not match its filename",
            ));
        }
        Ok(save)
    }
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>> {
        let file = File::open(path).map_err(io_error)?;
        if file.metadata().map_err(io_error)?.len() > self.limits.file_bytes as u64 {
            return Err(error(codes::CORRUPT, "save exceeds file read limit"));
        }
        let mut bytes = Vec::new();
        file.take(self.limits.file_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > self.limits.file_bytes {
            return Err(error(codes::CORRUPT, "save grew beyond read limit"));
        }
        Ok(bytes)
    }
    fn lock(&self, slot: &str) -> Result<File> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(format!("{slot}.lock")))
            .map_err(io_error)?;
        lock.try_lock()
            .map_err(|e| error(codes::IO, format!("slot has another active writer: {e}")))?;
        Ok(lock)
    }
}

/// Structured slot listing including unreadable/corrupted slot diagnostics.
#[derive(Debug, Serialize)]
pub struct SlotListing {
    /// Verified metadata in slot-name order.
    pub slots: Vec<SlotInfo>,
    /// Invalid/recovered slots, so agents can act on corruption explicitly.
    pub diagnostics: Vec<Diagnostic>,
}
fn validate_slot(slot: &str) -> Result<()> {
    let reserved = slot
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if slot.is_empty()
        || slot.len() > 64
        || !slot
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (reserved.len() == 4
            && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
            && reserved.as_bytes()[3].is_ascii_digit())
    {
        return Err(error(codes::IO, "invalid portable slot name"));
    }
    Ok(())
}
fn io_error(error_value: std::io::Error) -> Diagnostic {
    error(codes::IO, error_value.to_string())
}
fn atomic_replace(root: &Path, destination: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(root).map_err(io_error)?;
    temporary.write_all(bytes).map_err(io_error)?;
    temporary.as_file().sync_all().map_err(io_error)?;
    temporary
        .persist(destination)
        .map_err(|e| io_error(e.error))?;
    // On Unix sync the directory entry as well as file contents. Windows tempfile
    // uses MoveFileExW with replacement; the previously synced backup is independent.
    #[cfg(unix)]
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)?;
    Ok(())
}
