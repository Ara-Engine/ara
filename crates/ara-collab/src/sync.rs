//! Live op sync: the wire types and the conflict log.
//!
//! One authority (the project host) orders every transaction; participants receive
//! each as an [`OpEvent`] with its causal metadata (participant, sequence, base
//! revision), gapless and in authority order. Replicas submit optimistic changes as
//! [`Submit`]s. The authority applies them field-level on top of whatever happened
//! since their base: different fields merge, the same field is last-writer-wins by
//! authority order, and the overwritten change is logged as a [`Conflict`] and
//! surfaced to its author. A submit that no longer applies (its record was deleted)
//! is dropped whole, logged, and answered with a [`Reject`].

use std::collections::VecDeque;

use ara_diag::Diagnostic;
use ara_doc::{AssetId, Edit, Op, RecordId};
use ara_json::json_eq;
use ara_ops::{Transaction, TxnId};
use ara_schema::FieldPath;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ParticipantId;

/// Notification methods the host sends to joined participants.
pub mod methods {
    /// Every committed transaction ([`OpEvent`](super::OpEvent)), to everyone.
    pub const OP: &str = "collab.op";
    /// A submit the authority could not apply ([`Reject`](super::Reject)), to its author.
    pub const REJECT: &str = "collab.reject";
    /// `{participant, presence}`, rate limited, to everyone.
    pub const PRESENCE: &str = "collab.presence";
    /// `{participants}` after a join or leave, to everyone.
    pub const ROSTER: &str = "collab.roster";
    /// `{claims}` after any claim change, to everyone.
    pub const CLAIMS: &str = "collab.claims";
    /// A [`Conflict`](super::Conflict), to the participant whose change lost.
    pub const CONFLICT: &str = "collab.conflict";
    /// A branch session's summary after it changes, to everyone.
    pub const BRANCH: &str = "collab.branch";
}

/// One committed transaction as participants receive it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpEvent {
    /// Who committed it (their session), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participant: Option<ParticipantId>,
    /// The participant's submit sequence number (`collab.submit` only): their ack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The revision the participant based it on.
    pub base: u64,
    /// The transaction; `txn.result` is its authority revision.
    pub txn: Transaction,
}

/// An optimistic change a replica sends to the authority (`collab.submit`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Submit {
    /// The replica's sequence number (per participant, from 1).
    pub seq: u64,
    /// The authority revision the replica had applied when it made the change.
    pub base: u64,
    /// The edits, applied as one transaction.
    pub edits: Vec<Edit>,
    /// Why, if the participant gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Short label (`transform.set`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A submit the authority dropped, sent to its author.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reject {
    /// The submit's sequence number.
    pub seq: u64,
    /// Why it could not apply.
    pub diagnostics: Vec<Diagnostic>,
}

/// How a change lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    /// A concurrent change by someone else to the same field came later in authority
    /// order and replaced it.
    Overwritten,
    /// The change no longer applied at the authority and was dropped whole.
    Dropped,
    /// A branch merge kept the other side's value for a field both changed.
    Merge,
}

/// One side of a conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    /// The participant, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participant: Option<ParticipantId>,
    /// Display name (or author).
    pub name: String,
    /// Its transaction (none for a dropped submit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txn: Option<TxnId>,
    /// Its submit sequence number, for replica changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The value it wrote (absent: removed, or not a field write).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// A logged conflict: the losing change is preserved here, never silently lost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    /// Log sequence number.
    pub id: u64,
    /// Overwritten or dropped.
    pub kind: ConflictKind,
    /// The authority revision where it was decided.
    pub revision: u64,
    /// The document.
    pub doc: AssetId,
    /// The record, if one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<RecordId>,
    /// The field path; absent for whole-record changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<FieldPath>,
    /// The change that lost.
    pub loser: Side,
    /// The change that won (for drops: the likely cause), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<Side>,
    /// One line for the loser: what happened and what stands now.
    pub message: String,
}

/// The most recent conflicts, oldest first (bounded).
#[derive(Debug)]
pub struct ConflictLog {
    entries: VecDeque<Conflict>,
    next: u64,
    cap: usize,
}

impl ConflictLog {
    /// A log keeping the newest `cap` entries.
    pub fn new(cap: usize) -> Self {
        ConflictLog {
            entries: VecDeque::new(),
            next: 1,
            cap,
        }
    }

    /// Appends an entry (its `id` is assigned) and returns it.
    pub fn push(&mut self, mut c: Conflict) -> Conflict {
        c.id = self.next;
        self.next += 1;
        if self.entries.len() == self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(c.clone());
        c
    }

    /// Every kept entry, oldest first.
    pub fn all(&self) -> impl Iterator<Item = &Conflict> {
        self.entries.iter()
    }

    /// Entries whose losing side is `who`.
    pub fn lost_by<'a>(&'a self, who: &'a ParticipantId) -> impl Iterator<Item = &'a Conflict> {
        self.entries
            .iter()
            .filter(move |c| c.loser.participant.as_ref() == Some(who))
    }
}

/// The record paths an op touches (`None`: the whole record).
pub(crate) fn overlap(a: &Op, b: &Op) -> bool {
    a.doc() == b.doc()
        && (a.id() == b.id()
            || matches!(a, Op::Document { .. })
            || matches!(b, Op::Document { .. }))
        && a.touches().iter().any(|pa| {
            b.touches().iter().any(|pb| match (pa, pb) {
                (Some(x), Some(y)) => x.overlaps(y),
                _ => true,
            })
        })
}

/// What an op wrote, for conflict reports.
pub(crate) fn written(op: &Op) -> Option<Value> {
    match op {
        Op::File { path, state, .. } => Some(json!({"path":path,"state":state})),
        Op::Document { kind, .. } => kind.as_ref().map(|kind| json!({"kind": kind})),
        Op::Set { value, .. } => value.clone(),
        Op::Create { record, .. } => Some(Value::Object(record.clone())),
        Op::Move { parent, order, .. } => Some(json!({ "parent": parent, "order": order })),
        Op::Delete { .. } => None,
    }
}

/// The field an op writes (`None` for whole-record ops and moves).
pub(crate) fn path_of(op: &Op) -> Option<FieldPath> {
    match op {
        Op::Set { path, .. } => Some(path.clone()),
        Op::Move { .. } => Some(FieldPath::field(ara_doc::field::PARENT)),
        Op::Create { .. } | Op::Delete { .. } | Op::Document { .. } | Op::File { .. } => None,
    }
}

/// True if two overlapping ops leave the same result (no real conflict).
pub(crate) fn agree(a: &Op, b: &Op) -> bool {
    match (a, b) {
        (
            Op::Set {
                path: pa,
                value: va,
                ..
            },
            Op::Set {
                path: pb,
                value: vb,
                ..
            },
        ) => {
            pa == pb
                && match (va, vb) {
                    (Some(x), Some(y)) => json_eq(x, y),
                    (None, None) => true,
                    _ => false,
                }
        }
        (Op::Delete { .. }, Op::Delete { .. }) => true,
        _ => false,
    }
}
