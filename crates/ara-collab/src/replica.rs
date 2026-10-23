//! [`Replica`]: a participant's local copy of the documents, edited optimistically.
//!
//! The replica holds the authority's state up to [`revision`](Replica::revision)
//! plus its own pending (unacknowledged) changes on top. A local edit applies at
//! once (no round trip for the human) and becomes a [`Submit`]. When an op event
//! arrives, the pending changes are rewound, the event applied, and the pending
//! changes re-applied on top; an event carrying one of our sequence numbers is the
//! acknowledgement and retires that change. A pending change that no longer applies
//! stays hidden until the authority answers (it will drop it too). Once nothing is
//! pending the documents equal the authority's exactly: they are the same ops
//! applied to the same state in the same order.

use ara_diag::{Diagnostic, codes as kernel};
use ara_doc::{Edit, Op, Workspace};
use ara_schema::SchemaDb;

use crate::codes;
use crate::participant::ParticipantId;
use crate::snapshot::Snapshot;
use crate::sync::{OpEvent, Submit};

#[derive(Debug, Clone)]
struct Pending {
    submit: Submit,
    /// The ops applied locally for it (empty while it does not apply).
    ops: Vec<Op>,
}

/// A participant's optimistic copy of the documents.
#[derive(Debug, Clone)]
pub struct Replica {
    me: ParticipantId,
    db: SchemaDb,
    ws: Workspace,
    rev: u64,
    next_seq: u64,
    pending: Vec<Pending>,
}

impl Replica {
    /// A replica of `ws` at authority revision `rev`.
    pub fn new(me: ParticipantId, db: SchemaDb, ws: Workspace, rev: u64) -> Replica {
        Replica {
            me,
            db,
            ws,
            rev,
            next_seq: 1,
            pending: Vec::new(),
        }
    }

    /// A replica from a late-join snapshot.
    pub fn from_snapshot(
        me: ParticipantId,
        db: SchemaDb,
        snapshot: &Snapshot,
    ) -> Result<Replica, Vec<Diagnostic>> {
        let ws = snapshot.workspace(&db)?;
        Ok(Replica::new(me, db, ws, snapshot.revision))
    }

    /// The documents as this participant sees them (authority state plus pending).
    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }

    /// The Schema DB.
    pub fn schema(&self) -> &SchemaDb {
        &self.db
    }

    /// The last authority revision applied.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// Changes not yet acknowledged.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Applies `edits` locally, all or nothing, and returns the submit to send.
    pub fn edit(
        &mut self,
        edits: Vec<Edit>,
        why: Option<String>,
        label: Option<String>,
    ) -> Result<Submit, Diagnostic> {
        if edits.is_empty() {
            return Err(Diagnostic::error(
                kernel::INVALID_PARAMS,
                "an edit needs at least one change",
            ));
        }
        let ops = self.apply_all(&edits)?;
        let submit = Submit {
            seq: self.next_seq,
            base: self.rev,
            edits,
            why,
            label,
        };
        self.next_seq += 1;
        self.pending.push(Pending {
            submit: submit.clone(),
            ops,
        });
        Ok(submit)
    }

    /// Applies one authority op event. Events must arrive in revision order;
    /// duplicates are ignored. A gap or an event that does not apply is `ARA2508`:
    /// resync from a snapshot.
    pub fn receive(&mut self, ev: &OpEvent) -> Result<(), Diagnostic> {
        let rev = ev.txn.result;
        if rev <= self.rev {
            return Ok(());
        }
        if rev != self.rev + 1 {
            return Err(Diagnostic::error(
                codes::REPLICA_GAP,
                format!(
                    "replica at revision {} received revision {rev}; resync",
                    self.rev
                ),
            ));
        }
        self.rewind();
        let edits: Vec<Edit> = ev.txn.ops.iter().map(Op::to_edit).collect();
        if let Err(d) = self.apply_all(&edits) {
            self.replay();
            return Err(Diagnostic::error(
                codes::REPLICA_GAP,
                format!(
                    "revision {rev} does not apply to this replica ({}); resync",
                    d.message
                ),
            ));
        }
        self.rev = rev;
        if ev.participant.as_ref() == Some(&self.me)
            && let Some(seq) = ev.seq
        {
            self.pending.retain(|p| p.submit.seq != seq);
        }
        self.replay();
        Ok(())
    }

    /// The authority dropped submit `seq`: forget it.
    pub fn rejected(&mut self, seq: u64) {
        self.rewind();
        self.pending.retain(|p| p.submit.seq != seq);
        self.replay();
    }

    /// Pending submits to send again after a reconnect (the authority deduplicates
    /// by sequence number).
    pub fn resubmit(&self) -> Vec<Submit> {
        self.pending.iter().map(|p| p.submit.clone()).collect()
    }

    /// Replaces the state with a snapshot (after a gap), keeping pending changes on
    /// top.
    pub fn resync(&mut self, snapshot: &Snapshot) -> Result<(), Vec<Diagnostic>> {
        self.rewind();
        self.ws = snapshot.workspace(&self.db)?;
        self.rev = snapshot.revision;
        self.replay();
        Ok(())
    }

    /// Takes every pending change off the documents, newest first.
    fn rewind(&mut self) {
        for p in self.pending.iter_mut().rev() {
            for op in p.ops.drain(..).rev() {
                self.ws.revert(&op, &self.db);
            }
        }
    }

    /// Puts pending changes back on, oldest first; ones that no longer apply stay off.
    fn replay(&mut self) {
        let mut pending = std::mem::take(&mut self.pending);
        for p in &mut pending {
            p.ops = self.apply_all(&p.submit.edits).unwrap_or_default();
        }
        self.pending = pending;
    }

    /// Applies edits in order, all or nothing.
    fn apply_all(&mut self, edits: &[Edit]) -> Result<Vec<Op>, Diagnostic> {
        let mut ops: Vec<Op> = Vec::new();
        for edit in edits {
            let result = self.ws.apply(edit.clone(), &self.db);
            match result {
                Ok(op) => ops.push(op),
                Err(d) => {
                    for op in ops.iter().rev() {
                        self.ws.revert(op, &self.db);
                    }
                    return Err(d);
                }
            }
        }
        Ok(ops)
    }
}
