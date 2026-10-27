//! Signed releases shared by the Hub, its service and release tooling.
pub mod bootstrap;
pub mod build;
pub use build::BuildIdentity;
mod download;
mod feed;
mod manifest;
mod redirect;
mod storage;
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used)]
#[path = "../tests/support/mod.rs"]
pub mod testing;
mod transfer;
pub mod updater;

pub use download::{
    Cancel, DownloadPhase, Progress, download, download_with_phase, extract, sha256_file,
};
pub use feed::{Feed, FeedConfig, FeedSnapshot, FeedStatus};
pub use manifest::{
    Asset, Channel, Kind, Manifest, VerifiedRelease, canonical_manifest, sign, trusted_keys,
    verify, version,
};
pub use storage::{DiskSpace, check_space};

/// GitHub repository used when neither saved settings nor an override selects one.
pub const DEFAULT_REPOSITORY: &str = "Ara-Engine/ara";

use ara_diag::{Code, Diagnostic};
use std::fmt;

/// Stable, catalogued release failure.
#[derive(Debug, Clone)]
pub struct Error {
    /// Diagnostic number; callers preserve it across RPC.
    pub code: Code,
    /// Actionable failure detail, never credentials.
    pub message: String,
}
impl Error {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    /// Convert to the engine's standard diagnostic.
    pub fn diagnostic(&self) -> Diagnostic {
        let message = if self.code == RATE_LIMIT && !self.message.starts_with("GitHub") {
            format!(
                "GitHub release rate limit reached; retry after {}",
                self.message
            )
        } else {
            self.message.clone()
        };
        Diagnostic::error(self.code, message)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.diagnostic().message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(IO, error.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::new(MANIFEST, error.to_string())
    }
}
/// Untrusted or unsigned manifest.
pub const SIGNATURE: Code = Code(1950);
/// Download checksum mismatch.
pub const HASH: Code = Code(1951);
/// Download or expanded archive exceeds limits.
pub const SIZE: Code = Code(1952);
/// Invalid manifest, tag, platform, version or archive.
pub const MANIFEST: Code = Code(1953);
/// Unsafe URL or redirect.
pub const URL: Code = Code(1954);
/// Feed unavailable without a usable cache.
pub const NETWORK: Code = Code(1955);
/// GitHub rate limit without a usable cache.
pub const RATE_LIMIT: Code = Code(1956);
/// Storage or process operation failed.
pub const IO: Code = Code(1957);
/// User cancelled a transfer.
pub const CANCELLED: Code = Code(1958);
/// Hub update failed and rollback was required.
pub const ROLLBACK: Code = Code(1959);

/// Canonical JSON replacement; temporary files stay beside their destination.
pub fn write_json(path: &std::path::Path, value: &impl serde::Serialize) -> Result<(), Error> {
    let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(value)?));
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
