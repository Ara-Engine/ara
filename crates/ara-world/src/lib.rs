//! Canonical partitioned worlds, worker-side cell cooking and bounded activation.
#![deny(missing_docs)]

mod author;
mod cook;
mod index;
mod model;
mod runtime;
mod source;
mod tools;
mod worker;

pub use author::{MoveToCell, SetLayer, WorldCommand};
pub use cook::{CellBlob, CellEntity, HlodCooker, HlodProxy, cook_cell};
pub use index::{IndexedEntity, TextIndex};
pub use model::{Cell, CellKey, Layer, Manifest, WorldSettings, register};
pub use runtime::{
    ActivationBudget, FrameStats, Interest, StreamConfig, Streaming, StreamingSource,
};
pub use source::{CellSource, CookedSource, EditorSource, PackSource};
pub use tools::{CellsList, SpatialQuery, WorldTools};
pub use worker::WorkerPool;

/// A malformed world, rejected transaction or worker failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid authoring data, cooked content or runtime configuration.
    #[error("world: {0}")]
    Invalid(String),
    /// File transport failure.
    #[error("world I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid structured data.
    #[error("world JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The simulation rejected a boundary operation.
    #[error(transparent)]
    Sim(#[from] ara_sim::SimError),
    /// Asset cache rejected a result.
    #[error(transparent)]
    Asset(#[from] ara_asset::Error),
    /// Authoring transaction diagnostics, preserving codes and locations.
    #[error("world transaction rejected: {0:?}")]
    Transaction(Vec<ara_diag::Diagnostic>),
}

/// World subsystem result.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
