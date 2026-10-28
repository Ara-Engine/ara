//! Deterministic native UI capture and semantic tests for agents and the UI Designer.
#![warn(missing_docs)]
mod capture;
mod conformance;
mod geometry;
mod golden;
mod harness;
mod inspection;
mod query;
pub mod report;
pub use ara_ui_stories::{Query, QueryBy};
pub use capture::{Capture, CaptureEngine};
pub use conformance::audit_attached_ids;
pub use golden::{Comparison, Tolerance, compare};
pub use harness::{Harness, Key};
pub use query::Failure;
pub use report::{CaptureReport, register, report_schema};
/// Native capture and semantic-test errors.
pub type Result<T> = ara_ui::Result<T>;
