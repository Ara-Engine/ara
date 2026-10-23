//! Tiled heightfields, non-destructive authoring and bounded cache publication.
//! Authoring state lives in kernel documents; tiles are immutable derived media.
#![warn(missing_docs)]

pub mod cache;
pub mod commands;
mod evaluate;
pub mod job;
pub mod lod;
mod model;
mod physics;
pub mod stream;
mod tile;

pub use model::*;
pub use tile::*;

/// Terrain validation failure (local diagnostic range pending central allocation).
pub fn invalid(message: impl Into<String>) -> ara_diag::Diagnostic {
    ara_diag::Diagnostic::error(ara_diag::Code(3600), message)
}
