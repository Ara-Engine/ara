//! Immutable file states in transactions. Bytes and filesystem I/O belong to the host.
use ara_diag::{Diagnostic, codes};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Worker-prepared facts for a file-bearing command batch. The authority revalidates
/// these facts before commit. The opaque metadata belongs to the preparing subsystem.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedInventory {
    /// Initial file/directory states and explicitly observed destination absence.
    pub entries: BTreeMap<String, Option<FileState>>,
    /// Complete direct-child storage paths of inspected directories, sorted ordinally.
    /// The empty key denotes the project root; protected engine/Git entries are excluded.
    pub directories: BTreeMap<String, Vec<String>>,
    /// Owner-defined immutable worker/policy/URI/reference mapping metadata.
    pub metadata: serde_json::Value,
}

impl PreparedInventory {
    /// Validate the pure shape before publishing an inventory to commands.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        for (path, state) in &self.entries {
            check_path(path)?;
            if let Some(state) = state {
                state.validate()?;
            }
        }
        for (directory, children) in &self.directories {
            if !directory.is_empty() {
                check_path(directory)?;
            }
            for child in children {
                check_path(child)?;
                if child.rsplit_once('/').map_or("", |(p, _)| p) != directory {
                    return Err(Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "directory inventory must contain direct children only",
                    ));
                }
            }
            if children.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "directory inventory must be sorted and unique",
                ));
            }
        }
        Ok(())
    }

    /// Whether an initial state (including absence) was prepared for this path.
    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
            || (cfg!(windows) && self.entries.keys().any(|p| same_path(p, path)))
    }
}

/// Storage-path equality used by file state transitions on this platform.
pub fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Stable conflict/preflight key; op paths retain their original casing.
pub fn path_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_owned()
    }
}

/// The kind of a filesystem entry; directories retain empty folders and path casing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    /// A regular file whose bytes are stored under its hash.
    #[default]
    File,
    /// An ordinary directory (never a symbolic link).
    Directory,
}

/// Content-addressed snapshot token. `None` in a file op means the path is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileState {
    /// Lowercase SHA-256 of the exact bytes, or the directory marker.
    pub hash: String,
    /// Byte length (zero for a directory).
    pub len: u64,
    /// File or directory; directory contents are separate entries.
    #[serde(default)]
    pub kind: FileKind,
}

impl FileState {
    /// Hash bytes without touching the filesystem.
    pub fn bytes(bytes: &[u8]) -> Self {
        Self {
            hash: format!("{:x}", Sha256::digest(bytes)),
            len: bytes.len() as u64,
            kind: FileKind::File,
        }
    }

    /// The token for an existing directory; descendants have their own states.
    pub fn directory() -> Self {
        Self {
            kind: FileKind::Directory,
            len: 0,
            ..Self::bytes(b"ara:file-history:directory:v1")
        }
    }

    /// Reject malformed tokens before they become content-store paths.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if self.hash.len() != 64
            || !self
                .hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || (self.kind == FileKind::Directory && *self != Self::directory())
        {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "invalid file snapshot token",
            ));
        }
        Ok(())
    }
}

/// Validate canonical project-relative paths used by file ops and snapshot adapters.
pub fn check_path(path: &str) -> Result<(), Diagnostic> {
    if path.is_empty()
        || path.contains([':', '\\'])
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || path
            .split('/')
            .next()
            .is_some_and(|p| p.eq_ignore_ascii_case(".ara") || p.eq_ignore_ascii_case(".git"))
    {
        return Err(Diagnostic::error(
            codes::INVALID_PARAMS,
            "file path must stay inside project content",
        ));
    }
    Ok(())
}

/// Stable pseudo-record key keeps file touches separate in conflict checks and attribution.
pub fn record_id(path: &str) -> crate::RecordId {
    let hash = FileState::bytes(path_key(path).as_bytes()).hash;
    crate::RecordId::parse(&format!("f_{}", &hash[..16]))
        .expect("hexadecimal file key is a valid record ID")
}
