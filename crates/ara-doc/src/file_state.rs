//! Immutable file states in transactions. Bytes and filesystem I/O belong to the host.
use ara_diag::{Diagnostic, codes};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    let hash = FileState::bytes(path.as_bytes()).hash;
    crate::RecordId::parse(&format!("f_{}", &hash[..16]))
        .expect("hexadecimal file key is a valid record ID")
}
