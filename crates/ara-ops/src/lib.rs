//! Transactions over Ara documents (decision A4).
//!
//! - [`Store`] owns the Schema DB and the [`Workspace`](ara_doc::Workspace), and is the
//!   only writer of documents.
//! - A [`Staged`] transaction applies [`Edit`](ara_doc::Edit)s immediately, then
//!   [`commit`](Staged::commit) validates every touched document and records all ops
//!   or none. Dropping it rolls back; [`Staged::rollback`] returns the diff (dry run).
//! - Each [`Transaction`] carries an ID, [`Author`], optional `why` and label,
//!   base/result revisions, an idempotency key and its ops.
//! - Undo applies inverse ops as a new transaction, after checking that no later,
//!   still-effective transaction touched the same paths. Redo undoes the undo.
//! - [`Store::subscribe`] streams committed transactions; [`Store::since`] returns
//!   them from a revision onward.

mod store;
mod txn;

pub use store::{Commit, Staged, Store};
pub use txn::{Author, Transaction, TxnId, TxnMeta};
