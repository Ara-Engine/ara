//! Legacy review API over immediately accepted agent transactions (D24).
//!
//! Status lives per (transaction, entity). A transaction is `applied` while any of
//! its entities is, `reverted` when all are, `accepted` otherwise. Human edits and
//! undos are not tracked (they need no review).

use std::collections::BTreeMap;

use ara_ops::{Transaction, TxnId};

use crate::params::ReviewStatus;

/// An entity key: `<doc asset ID>/<record ID>` (`sc_arena001/e_8b21`).
pub fn entity_key(doc: &str, record: &str) -> String {
    format!("{doc}/{record}")
}

/// The entities a transaction touched, as entity keys, in first-touch order.
pub fn touched(txn: &Transaction) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for op in &txn.ops {
        let key = entity_key(op.doc().as_str(), op.id().as_str());
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// Review status of tracked transactions.
#[derive(Debug, Default)]
pub struct Review {
    txns: BTreeMap<TxnId, BTreeMap<String, ReviewStatus>>,
}

impl Review {
    /// Tracks attribution and revert state without creating a review requirement.
    pub fn track(&mut self, txn: &Transaction) {
        let entities = touched(txn)
            .into_iter()
            .map(|e| (e, ReviewStatus::Accepted))
            .collect();
        self.txns.insert(txn.id.clone(), entities);
    }

    /// True if the transaction is tracked.
    pub fn is_tracked(&self, txn: &TxnId) -> bool {
        self.txns.contains_key(txn)
    }

    /// Sets one entity's status (`entity: None` sets all of them).
    pub fn set(&mut self, txn: &TxnId, entity: Option<&str>, status: ReviewStatus) {
        if let Some(entities) = self.txns.get_mut(txn) {
            for (key, s) in entities.iter_mut() {
                if entity.is_none_or(|e| e == key) {
                    *s = status;
                }
            }
        }
    }

    /// Per-entity status of a tracked transaction.
    pub fn entities(&self, txn: &TxnId) -> Option<&BTreeMap<String, ReviewStatus>> {
        self.txns.get(txn)
    }

    /// The transaction's overall status; `None` if untracked.
    pub fn status(&self, txn: &TxnId) -> Option<ReviewStatus> {
        let entities = self.txns.get(txn)?;
        Some(if entities.values().any(|s| *s == ReviewStatus::Applied) {
            ReviewStatus::Applied
        } else if entities.values().all(|s| *s == ReviewStatus::Reverted) {
            ReviewStatus::Reverted
        } else {
            ReviewStatus::Accepted
        })
    }

    /// Tracked transactions with at least one entity in `status`.
    pub fn with_entity_status(&self, status: ReviewStatus) -> impl Iterator<Item = &TxnId> {
        self.txns
            .iter()
            .filter(move |(_, e)| e.values().any(|s| *s == status))
            .map(|(id, _)| id)
    }
}

#[cfg(test)]
mod tests {
    use ara_ops::{Author, Transaction, TxnId};
    use serde_json::json;

    use super::*;

    fn txn() -> Transaction {
        serde_json::from_value(json!({
            "id": "tx_a", "author": "agent:claude", "base": 0, "result": 1,
            "ops": [
                { "op": "set", "doc": "sc_a0000001", "id": "e_aaaa", "path": "name", "value": "A" },
                { "op": "set", "doc": "sc_a0000001", "id": "e_bbbb", "path": "name", "value": "B" },
                { "op": "set", "doc": "sc_a0000001", "id": "e_aaaa", "path": "Transform", "value": {} }
            ]
        }))
        .expect("transaction")
    }

    #[test]
    fn status_rolls_up_from_entities() {
        let t = txn();
        assert_eq!(t.author, Author::Agent("claude".into()));
        let mut r = Review::default();
        r.track(&t);
        let id = TxnId("tx_a".into());
        assert_eq!(touched(&t), ["sc_a0000001/e_aaaa", "sc_a0000001/e_bbbb"]);
        assert_eq!(r.status(&id), Some(ReviewStatus::Accepted));
        r.set(&id, Some("sc_a0000001/e_aaaa"), ReviewStatus::Reverted);
        assert_eq!(r.status(&id), Some(ReviewStatus::Accepted));
        r.set(&id, Some("sc_a0000001/e_bbbb"), ReviewStatus::Accepted);
        assert_eq!(r.status(&id), Some(ReviewStatus::Accepted));
        r.set(&id, None, ReviewStatus::Reverted);
        assert_eq!(r.status(&id), Some(ReviewStatus::Reverted));
    }
}
