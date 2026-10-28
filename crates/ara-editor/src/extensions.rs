//! Editor C# callbacks run out of process; only the authority applies returned commands.
use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub(crate) struct Catalog {
    pub commands: Vec<Command>,
    pub menus: Vec<Menu>,
    pub generation: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) struct Command {
    pub id: String,
    pub title: String,
    pub summary: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(rename = "disabledReason")]
    pub disabled_reason: Option<String>,
}
fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) struct Menu {
    pub path: String,
    pub command: String,
}
#[derive(Deserialize)]
pub(crate) struct Reply {
    #[serde(flatten)]
    pub catalog: Catalog,
    pub accepted: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub authoring: Vec<ara_host::params::CallSpec>,
    pub selection: Option<Vec<String>>,
}
enum Request {
    Changed,
    Invoke(String, Value),
    Stop,
}
pub(crate) enum Completion {
    Catalog(Result<Reply, Diagnostic>),
    Invoke(String, Result<Reply, Diagnostic>),
}
pub(crate) struct Pending {
    pub session: String,
    pub doc: String,
    pub revision: u64,
    pub command: String,
    pub navigation: crate::interaction::NavigationOwner,
}

pub(crate) struct Extensions {
    pub interaction: crate::interaction::Interaction,
    pub catalog: Catalog,
    pub pending: BTreeMap<String, Pending>,
    pub completed: mpsc::Receiver<Completion>,
    requests: mpsc::Sender<Request>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Extensions {
    pub fn new(
        root: &Path,
        interaction: crate::interaction::Interaction,
    ) -> Result<Self, Diagnostic> {
        let root = root.to_owned();
        let (requests, incoming) = mpsc::channel();
        let (send, completed) = mpsc::channel();
        let changed = requests.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&cancelled);
        let worker = std::thread::Builder::new()
            .name("ara-editor-csharp".into())
            .spawn(move || {
                use notify::Watcher;
                let root = match std::fs::canonicalize(root) {
                    Ok(root) => root,
                    Err(error) => {
                        let _ = send.send(Completion::Catalog(Err(failure(error))));
                        return;
                    }
                };
                let authored = root.join(ara_doc::AUTHORING_ROOT);
                let watch_root = authored.clone();
                let watcher = notify::recommended_watcher(
                    move |event: Result<notify::Event, notify::Error>| {
                        if let Ok(event) = event
                            && event.paths.iter().any(|path| {
                                path.starts_with(&watch_root)
                                    && (matches!(
                                        event.kind,
                                        notify::EventKind::Create(
                                            notify::event::CreateKind::Folder
                                        ) | notify::EventKind::Remove(
                                            notify::event::RemoveKind::Folder
                                        ) | notify::EventKind::Modify(
                                            notify::event::ModifyKind::Name(_)
                                        )
                                    ) || path.extension().is_none()
                                        || path
                                            .extension()
                                            .is_some_and(|s| s.eq_ignore_ascii_case("cs"))
                                        || path.to_string_lossy().ends_with(".module.json"))
                            })
                        {
                            let _ = changed.send(Request::Changed);
                        }
                    },
                )
                .and_then(|mut watcher| {
                    watcher.watch(&root, notify::RecursiveMode::Recursive)?;
                    Ok(watcher)
                });
                let _watcher = match watcher {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        let _ = send.send(Completion::Catalog(Err(failure(error))));
                        return;
                    }
                };
                let reserved: Vec<_> = crate::shell::commands::all().iter().map(|c| c.id).collect();
                let mut client = None;
                let mut reload = true;
                loop {
                    if stopping.load(Ordering::Relaxed) {
                        break;
                    }
                    if reload {
                        let result = (|| {
                            if client.is_none() && !ara_script::project::has_sources(&root)? {
                                return Ok(Reply {
                                    catalog: Catalog::default(),
                                    accepted: true,
                                    diagnostics: vec![],
                                    authoring: vec![],
                                    selection: None,
                                });
                            }
                            if client.is_none() {
                                client = Some(ara_script::project::EditorClient::start(&root)?);
                            }
                            let value = client
                                .as_mut()
                                .expect("editor worker started")
                                .call_cancelled(
                                    json!({"operation":"catalog","reserved":reserved}),
                                    &stopping,
                                )?;
                            serde_json::from_value(value).map_err(failure)
                        })();
                        if send.send(Completion::Catalog(result)).is_err() {
                            break;
                        }
                        reload = false;
                    }
                    match incoming.recv() {
                        Ok(Request::Changed) => {
                            // Batch a save/rename burst on the worker, never on a host tick.
                            while let Ok(request) =
                                incoming.recv_timeout(std::time::Duration::from_millis(100))
                            {
                                match request {
                                    Request::Changed => {}
                                    Request::Stop => return,
                                    Request::Invoke(job, request) => {
                                        invoke(&mut client, &send, job, request, &stopping)
                                    }
                                }
                            }
                            reload = true;
                        }
                        Ok(Request::Invoke(job, request)) => {
                            invoke(&mut client, &send, job, request, &stopping)
                        }
                        Ok(Request::Stop) | Err(_) => break,
                    }
                }
            })
            .map_err(failure)?;
        Ok(Self {
            interaction,
            catalog: Catalog::default(),
            pending: BTreeMap::new(),
            completed,
            requests,
            cancelled,
            worker: Some(worker),
        })
    }

    pub fn invoke(
        &mut self,
        job: String,
        pending: Pending,
        request: Value,
    ) -> Result<(), Diagnostic> {
        self.requests
            .send(Request::Invoke(job.clone(), request))
            .map_err(failure)?;
        self.pending.insert(job, pending);
        Ok(())
    }
}
impl Drop for Extensions {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let _ = self.requests.send(Request::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn invoke(
    client: &mut Option<ara_script::project::EditorClient>,
    send: &mpsc::Sender<Completion>,
    job: String,
    request: Value,
    stopping: &AtomicBool,
) {
    let result = client
        .as_mut()
        .ok_or_else(|| failure("no editor generation is loaded"))
        .and_then(|client| client.call_cancelled(request, stopping))
        .and_then(|value| serde_json::from_value(value).map_err(failure));
    let _ = send.send(Completion::Invoke(job, result));
}
fn failure(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(ara_script::codes::COMPILE_SERVER, error.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
