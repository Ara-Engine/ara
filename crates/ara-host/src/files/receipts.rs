//! Nonblocking acknowledgement of materialization, canonical saves and journal flushes.
use crate::host::State;
use ara_diag::{Diagnostic, codes};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Opaque handle for one prepared job's physical file completion. Poll copies until complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileCommitReceipt(u64);

#[derive(Debug, Default)]
pub(super) struct Receipts {
    pending: BTreeMap<FileCommitReceipt, Pending>,
}
#[derive(Debug, Default)]
struct Pending {
    phase: Phase,
    errors: Vec<Diagnostic>,
}
#[derive(Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Writes,
    Journal,
    Complete,
}

impl Receipts {
    fn begin(&mut self) -> FileCommitReceipt {
        let receipt = FileCommitReceipt(NEXT.fetch_add(1, Ordering::Relaxed));
        self.pending.insert(receipt, Pending::default());
        receipt
    }
    pub(super) fn error(&mut self, error: &Diagnostic) {
        for pending in self
            .pending
            .values_mut()
            .filter(|pending| pending.phase != Phase::Complete)
        {
            pending.errors.push(error.clone());
        }
    }
    /// The first barrier covers saves; their replies queue the journal's Flush entries.
    /// The second barrier acknowledges those follow-up appends before success.
    pub(super) fn barrier(&mut self, receipt: FileCommitReceipt) -> bool {
        let Some(pending) = self.pending.get_mut(&receipt) else {
            return false;
        };
        match pending.phase {
            Phase::Writes => {
                pending.phase = Phase::Journal;
                true
            }
            Phase::Journal => {
                pending.phase = Phase::Complete;
                false
            }
            Phase::Complete => false,
        }
    }
    fn worker_failed(&mut self) {
        let error = Diagnostic::error(
            codes::IO,
            "the project file worker stopped before physical completion",
        );
        self.error(&error);
        for pending in self.pending.values_mut() {
            pending.phase = Phase::Complete;
        }
    }
    fn poll(&mut self, receipt: FileCommitReceipt) -> Option<Result<(), Vec<Diagnostic>>> {
        let Some(pending) = self.pending.get(&receipt) else {
            return Some(Err(vec![Diagnostic::error(
                codes::INVALID_PARAMS,
                "unknown or already consumed file commit receipt",
            )]));
        };
        if pending.phase != Phase::Complete {
            return None;
        }
        let pending = self
            .pending
            .remove(&receipt)
            .expect("completed receipt exists");
        Some(if pending.errors.is_empty() {
            Ok(())
        } else {
            Err(pending.errors)
        })
    }
}

impl State {
    pub(crate) fn begin_file_commit(&mut self) -> FileCommitReceipt {
        self.files.receipts.begin()
    }
    pub(crate) fn abort_file_commit(&mut self, receipt: FileCommitReceipt) {
        self.files.receipts.pending.remove(&receipt);
    }
    pub(crate) fn seal_file_commit(&mut self, receipt: FileCommitReceipt) {
        self.queue_writes(true);
        if self.files.io.barrier(receipt).is_err() {
            self.files.receipts.worker_failed();
        }
    }
    pub(crate) fn poll_file_commit(
        &mut self,
        receipt: FileCommitReceipt,
    ) -> Option<Result<(), Vec<Diagnostic>>> {
        while let Some(reply) = self.files.io.try_reply() {
            self.file_reply(reply);
        }
        if self.files.io.disconnected() {
            self.files.receipts.worker_failed();
        }
        self.files.receipts.poll(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        file_io::{FileThread, Reply},
        journal::{Entry, Journal},
    };
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn held_worker_cannot_complete_a_receipt_before_both_barriers() {
        let root = std::env::var_os("ARA_TEST_ROOT")
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "ara-held-receipt-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
        fs::create_dir_all(&root).expect("scratch");
        let (journal, _, _) = Journal::open(&root.join(".ara")).expect("journal");
        let mut io = FileThread::start(
            root.clone(),
            ara_schema::SchemaDb::new(),
            Vec::new(),
            journal,
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("worker");
        let held = io.hold();
        let mut receipts = Receipts::default();
        let receipt = receipts.begin();
        io.barrier(receipt).expect("queue first barrier");
        assert!(
            io.try_reply().is_none(),
            "the held worker cannot acknowledge queued work"
        );
        assert!(receipts.poll(receipt).is_none());
        drop(held);
        assert!(matches!(io.reply(),Some(Reply::Barrier(id)) if id==receipt));
        assert!(receipts.barrier(receipt));
        assert!(
            receipts.poll(receipt).is_none(),
            "the save acknowledgement is not the journal acknowledgement"
        );
        // A Saved reply's follow-up journal append belongs before the final barrier.
        io.append(Entry::Flush {
            doc: "sc_receipt".into(),
            rev: 1,
            hash: "durable".into(),
        });
        io.barrier(receipt).expect("queue journal barrier");
        assert!(matches!(io.reply(),Some(Reply::Barrier(id)) if id==receipt));
        assert!(!receipts.barrier(receipt));
        assert!(receipts.poll(receipt).expect("completed").is_ok());
        assert!(
            fs::read_to_string(root.join(".ara/journal"))
                .expect("journal bytes")
                .contains("durable")
        );
    }

    #[test]
    fn worker_failure_finishes_pending_receipts_with_coded_io_error() {
        let mut receipts = Receipts::default();
        let receipt = receipts.begin();
        receipts.worker_failed();
        let errors = receipts
            .poll(receipt)
            .expect("terminal failure")
            .expect_err("worker stopped");
        assert_eq!(errors[0].code, codes::IO);
    }
}
