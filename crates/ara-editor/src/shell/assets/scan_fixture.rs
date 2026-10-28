//! Controlled worker publications exercise the real Browser inbox without timing or sleeps.
use super::*;

pub(in crate::shell) struct Driver {
    sender: mpsc::SyncSender<(u64, Result<Snapshot, ara_diag::Diagnostic>)>,
    started: Arc<AtomicU64>,
    wake: Receiver<()>,
    generation: u64,
}
pub(in crate::shell) fn controlled(reference_root: &str) -> (Browser, Driver) {
    let (sender, pending) = mpsc::sync_channel(1);
    let (refresh, wake) = mpsc::sync_channel(1);
    let started = Arc::new(AtomicU64::new(0));
    (
        Browser {
            items: Vec::new(),
            files: Vec::new(),
            reference_root: reference_root.into(),
            revision: 0,
            error: None,
            diagnostic: None,
            pending,
            scan_started: Arc::clone(&started),
            scan_completed: 0,
            refresh,
            optimistic: Default::default(),
        },
        Driver {
            sender,
            started,
            wake,
            generation: 0,
        },
    )
}
impl Driver {
    pub(in crate::shell) fn start(&mut self) -> u64 {
        self.generation += 1;
        self.started.store(self.generation, Ordering::Release);
        self.generation
    }
    pub(in crate::shell) fn requested(&self) -> bool {
        self.wake.try_recv().is_ok()
    }
    pub(in crate::shell) fn publish(
        &self,
        generation: u64,
        items: Vec<Asset>,
    ) -> ara_ui::Result<()> {
        let files = items
            .iter()
            .map(|asset| AuthoredFile {
                path: asset.authored_path.as_deref().unwrap_or(&asset.path).into(),
                storage_path: asset.path.clone(),
                scope: if asset.authored_path.is_some() {
                    AuthoredScope::Authored
                } else {
                    AuthoredScope::Project
                },
                role: if asset.kind == "folder" {
                    AuthoredRole::Folder
                } else {
                    AuthoredRole::Document {
                        name: asset.kind.clone(),
                    }
                },
            })
            .collect();
        self.sender
            .send((generation, Ok(Snapshot { files, items })))
            .map_err(|_| "Controlled browser receiver disconnected".into())
    }
    pub(in crate::shell) fn fail(&self, generation: u64) -> ara_ui::Result<()> {
        self.sender
            .send((
                generation,
                Err(ara_diag::Diagnostic::error(
                    ara_diag::codes::IO,
                    "Controlled browser scan failed",
                )),
            ))
            .map_err(|_| "Controlled browser receiver disconnected".into())
    }
}
