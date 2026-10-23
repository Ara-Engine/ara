//! Collaboration diagnostic codes (`ARA2500`–`ARA2599`, owner `ara-collab`). Each has
//! an entry in `crates/ara-diag/codes/collab.md`.

use ara_diag::Code;

/// Retired by D24: claim enforcement was removed.
pub const CLAIMED: Code = Code(2500);
/// Retired by D24: overlapping advisory claims are allowed.
pub const CLAIM_HELD: Code = Code(2501);
/// The session has not joined collaboration (`collab.join`).
pub const NOT_JOINED: Code = Code(2502);
/// A concurrent change by someone else overwrote this participant's change.
pub const OVERWRITTEN: Code = Code(2503);
/// A submitted change no longer applied at the authority and was dropped.
pub const DROPPED: Code = Code(2504);
/// No branch session with this ID.
pub const BRANCH_NOT_FOUND: Code = Code(2505);
/// The branch session is not in a state that allows this (merged, discarded).
pub const BRANCH_STATE: Code = Code(2506);
/// A branch merge kept the live value where both sides changed a field.
pub const MERGE_KEPT_LIVE: Code = Code(2507);
/// A replica missed ops (or diverged) and must resync.
pub const REPLICA_GAP: Code = Code(2508);
/// An undo left fields alone that someone else changed afterwards.
pub const UNDO_PARTIAL: Code = Code(2509);
/// A claim subject does not parse or names nothing.
pub const INVALID_SUBJECT: Code = Code(2510);
/// Only human sessions may explicitly merge another participant's branch.
pub const HUMAN_ONLY: Code = Code(2511);
/// Retired by D24: claims no longer delay branch merges.
pub const MERGE_BLOCKED: Code = Code(2512);
