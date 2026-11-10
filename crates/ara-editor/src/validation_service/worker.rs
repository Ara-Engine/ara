//! Read-only kernel checks and snapshot disposal never run on an editor input frame.
use super::snapshot::Snapshot;
use ara_diag::{Diagnostic, Severity};
use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
};

pub(super) struct Report {
    pub checked: usize,
    pub records: usize,
    pub errors: usize,
    pub warnings: usize,
    pub published: usize,
    pub cancelled: bool,
    pub diagnostics: VecDeque<Diagnostic>,
}
pub(super) struct Running {
    thread: JoinHandle<Result<Report, String>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: usize,
}
impl Running {
    pub fn start(snapshot: Snapshot, revision: u64) -> std::io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicUsize::new(0));
        let worker_cancel = Arc::clone(&cancel);
        let worker_done = Arc::clone(&done);
        let total = snapshot.workspace.docs().count();
        let thread = std::thread::Builder::new()
            .name("editor-validation".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    let mut report = Report {
                        checked: 0,
                        records: snapshot.records,
                        errors: 0,
                        warnings: 0,
                        published: 0,
                        cancelled: false,
                        diagnostics: VecDeque::new(),
                    };
                    for (_, doc) in snapshot.workspace.docs() {
                        if worker_cancel.load(Ordering::Acquire) {
                            report.cancelled = true;
                            break;
                        }
                        let diagnostics = snapshot.workspace.check(&doc.id, None, &snapshot.schema);
                        for mut diagnostic in diagnostics {
                            match diagnostic.severity {
                                Severity::Error => report.errors += 1,
                                Severity::Warning => report.warnings += 1,
                                Severity::Info => {}
                            }
                            // Problems is a historical bus: identify the source even
                            // if editing advances while the worker checks this revision.
                            diagnostic.message = format!(
                                "Validation at revision {revision}: {}",
                                diagnostic.message
                            );
                            report.diagnostics.push_back(diagnostic);
                        }
                        report.checked += 1;
                        worker_done.store(report.checked, Ordering::Release);
                    }
                    report
                }));
                // Snapshot record payloads are destroyed on this worker as well.
                drop(snapshot);
                result.map_err(|payload| {
                    let message = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("non-text panic");
                    format!("Loaded authoring validator panicked: {message}")
                })
            })?;
        Ok(Self {
            thread,
            cancel,
            done,
            total,
        })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn finished(&self) -> bool {
        self.thread.is_finished()
    }
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.
        } else {
            self.done.load(Ordering::Acquire) as f32 / self.total as f32
        }
    }
    pub fn complete(self) -> Result<Report, String> {
        self.thread
            .join()
            .map_err(|_| "Validation worker stopped unexpectedly".to_owned())?
    }
}
