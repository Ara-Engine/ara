//! Compile completion publishes the actual generated schema before attaching.
use super::*;
use std::sync::mpsc::{self, Receiver};

pub(super) struct Compile {
    session: String,
    path: String,
    doc: Option<String>,
    attach: Vec<String>,
    result: Receiver<Result<Value, String>>,
}
impl ScriptService {
    pub(super) fn take_path(
        &mut self,
        cx: &mut Cx<'_>,
        path: &str,
    ) -> Option<(Option<String>, Vec<String>)> {
        let ids = self
            .jobs
            .iter()
            .filter(|(_, j)| j.path == path)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut attachment = None;
        for id in ids {
            if let Some(job) = self.jobs.remove(&id) {
                attachment = Some((job.doc, job.attach));
                cx.finish_job(&id, Ok(json!({"supersededByRename":true})));
            }
        }
        attachment
    }
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
        let executable = std::env::var_os("ARA_EDITOR_EXE")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
            .ok_or("Cannot locate the script compiler worker")?;
        let (tx, result) = mpsc::channel();
        std::thread::spawn(move || {
            let mut command = std::process::Command::new(executable);
            command.arg("--compile-scripts").arg(root);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
            let output = command.output().map_err(|e| e.to_string()).and_then(|o| {
                serde_json::from_slice::<Value>(&o.stdout).map_err(|e| {
                    format!(
                        "C# compiler worker failed: {e}; {}",
                        String::from_utf8_lossy(&o.stderr)
                    )
                })
            });
            let _ = tx.send(output);
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
        if !self.jobs.is_empty() {
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
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("C# compiler worker stopped unexpectedly".into())
                }
            };
            let result = result
                .map_err(|e| vec![crate::invalid(e)])
                .and_then(|result| {
                    super::publish::complete(
                        cx,
                        &pending.session,
                        &pending.path,
                        pending.doc.as_deref(),
                        &pending.attach,
                        result,
                    )
                });
            if let Err(diagnostics) = &result {
                for diagnostic in diagnostics {
                    cx.log(diagnostic.clone());
                }
            }
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
