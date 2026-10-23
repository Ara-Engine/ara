//! Versioned save games from opt-in reflected state. Capture/apply at simulation
//! boundaries; compression, checksumming and filesystem work belong on host workers.
#![warn(missing_docs)]
mod diff;
mod format;
mod schema;
mod slots;
pub mod tools;

pub use ara_sim::{
    PersistenceSelection, PersistentCursor, PersistentEntity, PersistentState, StateHash,
};
pub use diff::{FieldDiff, diff};
pub use format::{DecodedSave, Limits, SaveGame, SlotMetadata, decode, encode};
pub use schema::{PersistDeclaration, SaveSchema, Storage, register_schema};
pub use slots::{Authority, LoadedSlot, SlotInfo, SlotListing, SlotStore};

/// Save-game diagnostics, local to this subsystem until the shared catalog expands.
pub mod codes {
    use ara_diag::Code;
    /// Invalid format, checksum, truncation or decompression limit.
    pub const CORRUPT: Code = Code(7501);
    /// Save policy, schema version or migration is invalid.
    pub const SCHEMA: Code = Code(7502);
    /// A client attempted to write/load authoritative state.
    pub const AUTHORITY: Code = Code(7503);
    /// Atomic slot IO failed; the previous generation remains recoverable.
    pub const IO: Code = Code(7504);
    /// A backup supplied the last valid slot generation.
    pub const BACKUP: Code = Code(7505);
}
/// Save operations return stable diagnostics suitable for agent tools.
pub type Result<T> = std::result::Result<T, ara_diag::Diagnostic>;
fn error(code: ara_diag::Code, message: impl Into<String>) -> ara_diag::Diagnostic {
    ara_diag::Diagnostic::error(code, message)
}
