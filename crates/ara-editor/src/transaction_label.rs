//! Human-readable action identity across recorded Undo/Redo transactions.
use ara_ops::{Store, Transaction};

/// Follow typed inverse IDs to the original action without rewriting history or
/// interpreting its title. Strictly decreasing revisions exclude malformed cycles.
pub(crate) fn original<'a>(
    store: &'a Store,
    mut transaction: &'a Transaction,
) -> Option<&'a Transaction> {
    while let Some(id) = &transaction.reverts {
        let original = store.txn(id)?;
        if original.result >= transaction.result {
            return None;
        }
        transaction = original;
    }
    Some(transaction)
}
