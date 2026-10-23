#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Ordered, typed VFX authoring and deterministic CPU simulation.
pub mod commands;
pub mod library;
mod model;
mod modules;
mod projection;
pub mod samples;
mod simulate;
mod slang;
mod validate;

pub use model::*;
pub use modules::*;
pub use projection::{VfxAdapter, registry};
pub use simulate::*;
pub use slang::generate;

use ara_diag::{Code, Diagnostic};

/// VFX diagnostic code, local until the shared catalog assigns this track a range.
pub const INVALID: Code = Code(3600);
/// Quality budget diagnostic; dropped work is visible to agents and the editor.
pub const QUALITY: Code = Code(3601);
pub(crate) fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(INVALID, message)
}
pub(crate) fn encode(v: &impl serde::Serialize) -> Result<serde_json::Value, Diagnostic> {
    serde_json::to_value(v).map_err(|e| error(e.to_string()))
}
pub(crate) fn decode<T: serde::de::DeserializeOwned>(
    v: serde_json::Value,
) -> Result<T, Diagnostic> {
    serde_json::from_value(v).map_err(|e| error(e.to_string()))
}
