//! Shared graph storage, domain validation, and undoable authoring (A3/A4).
//! Node/edge IDs are document record IDs; port IDs are stable keys within nodes.
//! Editor state is a separate record, excluded from compilation identity.

pub mod commands;
mod model;
mod registry;

pub use model::*;
pub use registry::*;

/// Graph diagnostic codes. Kept local because this track cannot edit ara-diag.
pub mod codes {
    use ara_diag::Code;
    /// Invalid graph storage, IDs, registry metadata, or references.
    pub const STRUCTURE: Code = Code(3100);
    /// Node kind/version is unavailable; its data is preserved.
    pub const UNKNOWN_NODE: Code = Code(3101);
    /// Missing, occupied, or incompatible ports.
    pub const PORT: Code = Code(3102);
    /// Domain forbids a cycle.
    pub const CYCLE: Code = Code(3103);
}
