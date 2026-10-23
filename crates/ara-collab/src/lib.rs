//! Collaboration foundations (D21): many humans and many agents in one project,
//! live, without conflicts.
//!
//! - **Presence** ([`presence`]): who is here, what they look at (document, camera,
//!   selection, pointer) and what they do (command or turn and its why). Ephemeral,
//!   rate-limited broadcast.
//! - **Claims** ([`claims`]): soft locks on documents, entities, components/fields
//!   and asset paths, with expiry and heartbeats. Advisory for humans, enforced for
//!   agents through an `ara_ops::Guard` (`ARA2500` names the claimant).
//! - **Live op sync** ([`sync`], [`Hub`], [`Replica`]): the authority orders every
//!   transaction and broadcasts it with causal metadata; replicas edit
//!   optimistically and rebase. Different fields merge; the same field is
//!   last-writer-wins by authority order, and the loser goes to the conflict log.
//! - **Per-participant undo** ([`undo`]): undoing mine never undoes yours.
//! - **Branch sessions** ([`branch`]): agents work on an overlay and submit; merges
//!   are field-level three-way with a conflict report, per the apply policy.
//! - **Late join** ([`Snapshot`]) and the **editor seam** ([`view::CollabView`]).
//!
//! No IO and no clock: callers pass `now` (ms). The project host (`ara-host`'s
//! `collab` module) serves this over the Engine Protocol.

pub mod branch;
pub mod claims;
pub mod codes;
mod hub;
pub mod participant;
pub mod presence;
pub mod replica;
pub mod snapshot;
pub mod subject;
pub mod sync;
pub mod undo;
pub mod view;
pub mod who;

pub use branch::{Branch, BranchInfo, BranchStatus, MergeReport, Prefer, SavedBranch};
pub use claims::{Claim, ClaimGuard, Wanted};
pub use hub::{Ack, ApplyPolicy, Audience, Event, Hub, PRESENCE_INTERVAL_MS, not_joined};
pub use participant::{Kind, Participant, ParticipantId};
pub use presence::{Activity, Camera, Presence};
pub use replica::Replica;
pub use snapshot::Snapshot;
pub use subject::Subject;
pub use sync::{Conflict, ConflictKind, OpEvent, Reject, Submit};
pub use undo::UndoOutcome;
