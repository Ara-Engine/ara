//! Production export driver: pinned templates, cooked packs, C# lanes and reports.
//! The CLI owner can wire these APIs without linking a command-line parser.

#![warn(missing_docs)]
pub mod android;
pub mod apple;
/// Platform-scoped host prerequisites and runnable fixes.
pub mod doctor;
/// Versioned NDJSON progress/artifact/diagnostic stream.
pub mod event;
mod files;
mod native;
mod pipeline;
/// Schema-registered project export documents.
pub mod preset;
/// Hash-pinned template cache.
pub mod template;
/// Tool discovery, bounded child processes and metadata writing.
pub mod tools;
pub mod verify;

pub use pipeline::{ExportRequest, ExportSummary, export};
pub use preset::{ExportPreset, Platform};
