//! Compile completion publishes the actual generated schema before attaching.
use super::*;
use std::sync::mpsc::{self, Receiver};
#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;

pub(super) struct Compile {
    session: String,
    path: String,
    doc: Option<String>,
    attach: Vec<String>,
    result: Receiver<Result<Value, Vec<ara_diag::Diagnostic>>>,
}
impl ScriptService {
    pub(super) fn start(
        &mut self,
        cx: &mut Cx<'_>,
        session: &str,
        path: &str,
        doc: Option<String>,
        attach: Vec<String>,
    ) -> Result<String, String> {
        self.watch_session = Some(session.into());
        if self.sources.is_none() {
            let (tx, rx) = mpsc::channel();
            let root = cx.root().to_owned();
            std::thread::spawn(move || {
                loop {
                    if tx.send(source_signature(&root)).is_err() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            });
            self.sources = Some(rx);
        }
        let job = cx.start_job("Compile C# script");
        let root = cx.root().to_owned();
        let (tx, result) = mpsc::channel();
        std::thread::spawn(move || {
            let response = super::worker::response(super::worker::compile(&root));
            let _ = tx.send(Ok(response));
        });
        self.jobs.insert(
            job.clone(),
            Compile {
                session: session.into(),
                path: path.into(),
                doc,
                attach,
                result,
            },
        );
        Ok(job)
    }

    pub(super) fn watch(&mut self, cx: &mut Cx<'_>) {
        if !self.jobs.is_empty() || self.authoring_source() {
            return;
        }
        let Some(signature) = self.sources.as_ref().and_then(|rx| rx.try_iter().last()) else {
            return;
        };
        let previous = self.signature.replace(signature);
        if previous.is_none() || previous == Some(signature) {
            return;
        }
        let Some(session) = self.watch_session.clone() else {
            return;
        };
        let _ = cx.resume_session(&session, |cx| {
            self.start(cx, &session, "", None, Vec::new())
        });
    }

    pub(super) fn poll(&mut self, cx: &mut Cx<'_>) {
        self.jobs.retain(|job, pending| {
            if cx.job_cancelled(job) {
                return false;
            }
            let result = match pending.result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => Err(vec![crate::invalid(
                    "C# compiler worker stopped unexpectedly",
                )]),
            };
            let result = match result {
                Ok(result) => super::publish::complete(
                    cx,
                    &pending.session,
                    &pending.path,
                    pending.doc.as_deref(),
                    &pending.attach,
                    result,
                ),
                Err(diagnostics) => {
                    for diagnostic in &diagnostics {
                        cx.log(diagnostic.clone());
                    }
                    Err(diagnostics)
                }
            };
            cx.finish_job(job, result);
            false
        });
    }
}

fn source_signature(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::hash::DefaultHasher::new();
    match ara_script::project::inventory(root) {
        Ok(inventory) => {
            for diagnostic in inventory.diagnostics {
                diagnostic.to_string().hash(&mut hash);
            }
            for module in inventory.modules {
                module.name.hash(&mut hash);
                module.references.hash(&mut hash);
                module.hot.hash(&mut hash);
                module.profile.as_str().hash(&mut hash);
            }
            for source in inventory.sources {
                source.storage_path.hash(&mut hash);
                source.module.hash(&mut hash);
                if let Ok(metadata) = std::fs::metadata(root.join(source.storage_path)) {
                    metadata.len().hash(&mut hash);
                    metadata.modified().ok().hash(&mut hash);
                }
            }
        }
        Err(diagnostic) => diagnostic.to_string().hash(&mut hash),
    }
    hash.finish()
}
