//! Per-participant undo (D21): undoing my change never undoes yours.
//!
//! Each participant has its own undo and redo stacks of transaction IDs (fed by
//! [`Hub::publish`]). Undo applies the inverse of my latest change still in effect,
//! rebased over everyone else's later work: fields someone else changed since keep
//! their value and are reported as skipped (`Store::undo_rebased`). Redo undoes the
//! undo the same way.

use ara_diag::{Diagnostic, codes as kernel};
use ara_doc::{AssetId, RecordId};
use ara_ops::{Store, Transaction, TxnId};
use ara_schema::FieldPath;
use serde::Serialize;

use crate::ParticipantId;
use crate::hub::{Hub, Origin};
use crate::sync::{overlap, path_of};

/// A field an undo left alone because someone else changed it afterwards.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    /// Document.
    pub doc: AssetId,
    /// Record.
    pub record: RecordId,
    /// Field path; absent for the whole record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<FieldPath>,
    /// Who changed it since (display name).
    pub by: String,
}

/// What an undo or redo did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    /// The transaction undone (or, for redo, the undo undone).
    pub undone: TxnId,
    /// The new transaction; absent when everything was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txn: Option<TxnId>,
    /// Fields left as others set them.
    pub skipped: Vec<Skipped>,
}

impl Hub {
    /// Undoes `id`'s latest change still in effect.
    pub fn undo(
        &mut self,
        store: &mut Store,
        id: &ParticipantId,
    ) -> Result<UndoOutcome, Vec<Diagnostic>> {
        self.step(store, id, false)
    }

    /// Redoes `id`'s latest undo.
    pub fn redo(
        &mut self,
        store: &mut Store,
        id: &ParticipantId,
    ) -> Result<UndoOutcome, Vec<Diagnostic>> {
        self.step(store, id, true)
    }

    fn step(
        &mut self,
        store: &mut Store,
        id: &ParticipantId,
        redo: bool,
    ) -> Result<UndoOutcome, Vec<Diagnostic>> {
        let author = self.participant(id)?.author.clone();
        let stacks = self.stacks.entry(id.clone()).or_default();
        let stack = if redo {
            &mut stacks.redo
        } else {
            &mut stacks.undo
        };
        let target = loop {
            match stack.pop() {
                Some(t) if store.reverted_by(&t).is_some() => continue,
                Some(t) => break t,
                None => {
                    let what = if redo { "redo" } else { "undo" };
                    return Err(vec![Diagnostic::error(
                        kernel::NOTHING_TO_UNDO,
                        format!("{} has nothing to {what}", self.roster.name(id)),
                    )]);
                }
            }
        };
        let origins = &self.origins;
        let foreign =
            |t: &Transaction| origins.get(&t.id).and_then(|o| o.participant.as_ref()) != Some(id);
        let result = store
            .undo_rebased(&target, author, &foreign)
            .map(|r| (r.txn.cloned(), r.skipped));
        let (txn, skipped_ops) = match result {
            Ok(r) => r,
            Err(e) => {
                let stacks = self.stacks.entry(id.clone()).or_default();
                if redo {
                    stacks.redo.push(target);
                } else {
                    stacks.undo.push(target);
                }
                return Err(e);
            }
        };
        let later: Vec<Transaction> = store
            .history()
            .iter()
            .filter(|t| foreign(t) && store.txn(&target).is_some_and(|x| t.result > x.result))
            .cloned()
            .collect();
        let skipped = skipped_ops
            .iter()
            .map(|op| {
                let by = later
                    .iter()
                    .rev()
                    .find(|t| t.ops.iter().any(|o| overlap(o, op)))
                    .map_or_else(String::new, |t| {
                        self.owner(&t.id)
                            .map_or_else(|| t.author.to_string(), |p| self.roster.name(p))
                    });
                Skipped {
                    doc: op.doc().clone(),
                    record: op.id().clone(),
                    path: path_of(op),
                    by,
                }
            })
            .collect();
        if let Some(t) = &txn {
            self.origins.insert(
                t.id.clone(),
                Origin {
                    participant: Some(id.clone()),
                    seq: None,
                    base: t.base,
                },
            );
            let stacks = self.stacks.entry(id.clone()).or_default();
            if redo {
                stacks.undo.push(t.id.clone());
            } else {
                stacks.redo.push(t.id.clone());
            }
        }
        Ok(UndoOutcome {
            undone: target,
            txn: txn.map(|t| t.id),
            skipped,
        })
    }
}
