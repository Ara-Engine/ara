use serde::{Deserialize, Serialize};

/// Engine-private receipt for one source.replace job's exact byte revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceClaim {
    /// Stable source-write job identity; retirement affects only this edit.
    pub edit: String,
    /// SHA-256 of the replacement's exact UTF-8 bytes, including any BOM.
    pub hash: String,
    /// Preparation or terminal materialization state.
    pub phase: SourceClaimPhase,
}

/// Private source materialization lifecycle, separate from compilation/reload state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceClaimPhase {
    /// Register expected bytes before any filesystem mutation.
    Preparing,
    /// The file worker finished writing this revision.
    Materialized,
    /// Preparation or materialization failed or was cancelled.
    Cancelled,
}
