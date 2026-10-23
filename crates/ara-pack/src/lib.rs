//! Deterministic content packs: canonical manifests, zstd chunks and store deltas.
//!
//! Version 2 splits files into bounded chunks, deduplicates by cooked BLAKE3,
//! and aligns chunks to 4 KiB. Cook keys include platform, architecture, profile,
//! importer version and settings. Encryption deters casual extraction only;
//! a shipped runtime necessarily has the decryption key.

#![warn(missing_docs)]
mod model;
mod read;
#[cfg(feature = "write")]
mod write;

pub use model::*;
pub use read::{MappedPack, Pack};
#[cfg(feature = "write")]
pub use write::{BuiltPack, Input, build, build_directory};

/// An invalid pack, unsupported format or failed I/O operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Malformed or corrupt input, including invalid paths and size limits.
    #[error("ARA-EXP-PACK-001: {0}")]
    Invalid(String),
    /// Encryption was requested without the matching key.
    #[error("ARA-EXP-PACK-002: missing or incorrect pack encryption key")]
    Key,
    /// Filesystem failure.
    #[error("ARA-EXP-PACK-003: {0}")]
    Io(#[from] std::io::Error),
    /// Manifest could not be serialized or decoded.
    #[error("ARA-EXP-PACK-004: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<&Error> for ara_diag::Diagnostic {
    /// The error as a bus diagnostic under its `ARA-EXP-PACK-*` named code.
    fn from(e: &Error) -> ara_diag::Diagnostic {
        let name = match e {
            Error::Invalid(_) => "ARA-EXP-PACK-001",
            Error::Key => "ARA-EXP-PACK-002",
            Error::Io(_) => "ARA-EXP-PACK-003",
            Error::Json(_) => "ARA-EXP-PACK-004",
        };
        let code = ara_diag::Code::parse(name).unwrap_or(ara_diag::codes::UNKNOWN_CODE);
        let text = e.to_string();
        let message = text.split_once(": ").map_or(text.as_str(), |(_, m)| m).to_owned();
        ara_diag::Diagnostic::error(code, message)
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

pub(crate) fn canonical<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    Ok(
        ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(value)?))
            .into_bytes(),
    )
}

/// Validates a portable relative content path (never an OS path or `@` reference).
pub fn validate_path(path: &str) -> Result<(), Error> {
    if path.is_empty()
        || path.contains(['\\', ':', '\0'])
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || path.starts_with('@')
    {
        return Err(invalid(format!("invalid content path {path:?}")));
    }
    Ok(())
}
