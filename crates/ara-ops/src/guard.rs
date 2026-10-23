//! Commit guards: a hook that can veto a transaction just before it commits.
//!
//! The project host installs one for collaboration claims (D21): an agent's ops on a
//! subject another participant claimed are rejected with a diagnostic naming the
//! claimant. The guard sees the transaction's metadata, the store's current
//! [`origin`](crate::Store::origin) (the committing session, when the caller set one)
//! and the ops; it never changes anything.

use std::fmt;

use ara_diag::Diagnostic;
use ara_doc::Op;

use crate::TxnMeta;

/// Vetoes transactions at commit (and in dry runs, through `Staged::check`).
///
/// Install with [`Store::set_guard`](crate::Store::set_guard). Implementations are
/// snapshots: to change what they enforce, install a new one.
pub trait Guard: fmt::Debug + Send {
    /// Errors that block committing `ops` (empty: allowed). `origin` is who is
    /// committing as the caller identified them (a session handle), if set.
    fn check(&self, meta: &TxnMeta, origin: Option<&str>, ops: &[Op]) -> Vec<Diagnostic>;
}
