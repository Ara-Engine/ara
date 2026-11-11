#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! CPU description of one frame: scene snapshot, native content, look and capture.
//!
//! `ara-render` draws these records. This crate has no device,
//! shader or pass graph. Gameplay state stays in ECS; nothing here is authoritative.
pub mod capture;
pub mod change_feed;
pub mod instanced;
pub mod look;
pub mod scene;
