//! Deterministic asset jobs, immutable caches and boundary-controlled hot reload.
#![deny(missing_docs)]

mod cache;
mod graph;
mod hot_reload;
mod importer;
mod metrics;
mod preset;
mod sidecar;
mod source;
mod worker;

pub use cache::{CacheBackend, CachedImport, FileCache, GcStats};
pub use graph::DependencyGraph;
pub use hot_reload::{ApplyPolicy, AssetChanged, AssetSlot, FileWatcher, HotReload, ReloadEvent};
pub use importer::{
    Artifact, ArtifactKind, Cancellation, Digest, ImportContext, ImportInput, ImportOutput,
    Importer, ImporterRegistry, Progress, TargetPlatform,
};
pub use metrics::{ImportMetrics, ImportStage, StageTimer, StageTimes};
pub use preset::{CookPreset, DEFAULT_THREAD_BUDGET};
pub use sidecar::{ImportSettings, Sidecar, SidecarCommand, register_schema};
pub use source::{AssetPipeline, SourceAsset, snapshot};
pub use worker::{ImportJob, JobEvent, ProcessWorker, serve_worker};

/// Asset errors preserve the last published generation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// File or process transport failed.
    #[error("asset I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Structured data could not be decoded.
    #[error("asset JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Source content, settings, or cooked data are invalid.
    #[error("{0}")]
    Invalid(String),
    /// The job was cancelled before publication.
    #[error("import cancelled")]
    Cancelled,
    /// A child process failed or violated the worker protocol.
    #[error("import worker: {0}")]
    Worker(String),
}
impl From<ara_asset_format::FormatError> for Error {
    fn from(e: ara_asset_format::FormatError) -> Self {
        Self::Invalid(e.to_string())
    }
}
impl Error {
    /// The error as a bus diagnostic about `asset` (a path or `@path#id`).
    pub fn diagnostic(&self, asset: &str) -> ara_diag::Diagnostic {
        use ara_diag::{Diagnostic, Subject, codes};
        let reference = if asset.starts_with('@') {
            asset.to_owned()
        } else {
            format!("@{asset}")
        };
        let d = match self {
            Error::Cancelled => Diagnostic::info(
                codes::ASSET_IMPORT_CANCELLED,
                format!("import of `{asset}` was cancelled or superseded"),
            ),
            Error::Worker(m) => Diagnostic::error(
                codes::ASSET_WORKER_FAILED,
                format!("import worker failed on `{asset}`: {m}"),
            ),
            other => Diagnostic::error(
                codes::ASSET_IMPORT_FAILED,
                format!("`{asset}` did not import: {other}"),
            ),
        };
        d.subject(Subject::asset(reference))
    }
}

/// Result shared by pipeline and plugin importers.
pub type Result<T> = std::result::Result<T, Error>;
