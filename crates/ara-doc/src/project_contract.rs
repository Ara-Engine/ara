//! Shared on-disk authoring convention. Project hosts own path validation and I/O.

/// The single authored-content root. Asset references are relative to this directory.
pub const AUTHORING_ROOT: &str = "game";
