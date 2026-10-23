//! Deterministic tiled foliage extraction and undoable manual painting.
//! Extracted instances are presentation data; authoring truth is a kernel document.
#![warn(missing_docs)]
pub mod commands;
mod model;
mod scatter;
pub use model::*;
pub use scatter::*;
