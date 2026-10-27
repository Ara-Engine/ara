//! File preparation jobs and history recovery; no hashing or copying on host ticks.
use crate::{Cx, Service, file_state::FileHistory, host::State, journal::Entry};
use ara_diag::{Diagnostic, codes};
use ara_doc::{
    Op,
    file_state::{FileState, PreparedInventory},
};
use ara_ops::TxnId;
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver},
};

type Observed = Vec<(String, Option<FileState>)>;
type FileTransitions = Vec<(usize, Option<FileState>, Option<FileState>)>;

struct Pending {
    job: String,
    session: String,
    method: String,
    params: Value,
    revision: u64,
    ready: Receiver<Result<PreparedInventory, Vec<Diagnostic>>>,
    revalidated: bool,
}

#[derive(Default)]
pub(crate) struct FileJobs {
    pending: Vec<Pending>,
    pub(crate) applying: bool,
}

impl State {
    /// Defer file source preparation and file-bearing undo/redo to a worker, then
    /// resume the same validated call under its original session at a host tick.
    pub(crate) fn prepare_file_call(
        &mut self,
        sid: &str,
        method: &str,
        params: &Value,
    ) -> Result<Option<Value>, Vec<Diagnostic>> {
        if self.file_jobs.applying {
            return Ok(None);
        }
        let mut sources = Vec::new();
        let mut inverse = Vec::new();
        if method == "cmd.apply" {
            if let Some(calls) = params["commands"].as_array() {
                for call in calls.iter().filter(|c| c["cmd"] == "source.replace") {
                    sources.push(
                        serde_json::from_value::<ara_cmd::builtin::SourceReplaceParams>(
                            call["args"].clone(),
                        )
                        .map_err(|e| {
                            vec![Diagnostic::error(codes::INVALID_PARAMS, e.to_string())]
                        })?,
                    );
                }
            }
        } else if matches!(method, "history.undo" | "history.redo") {
            let author = self.sessions.live(sid).map(|s| &s.author);
            let target = if method == "history.redo" {
                author.and_then(|a| self.store.next_redo(a))
            } else if let Some(id) = params["txn"].as_str() {
                self.store.txn(&TxnId(id.to_owned()))
            } else {
                author.and_then(|a| self.store.next_undo(a))
            };
            if let Some(target) = target {
                inverse = target
                    .ops
                    .iter()
                    .rev()
                    .filter(|op| matches!(op, Op::File { .. }))
                    .map(Op::inverse)
                    .collect();
            }
        }
        if sources.is_empty() && inverse.is_empty() {
            return Ok(None);
        }
        let root = self.root.clone();
        let (send, ready) = mpsc::channel();
        std::thread::Builder::new()
            .name("ara-file-prepare".into())
            .spawn(move || {
                let store = FileHistory::new(root);
                let result = (|| {
                    let mut inventory = PreparedInventory::default();
                    let mut staged = BTreeMap::new();
                    for source in sources {
                        let state = store.snapshot(&source.path).map_err(|d| vec![d])?;
                        let current = staged.get(&source.path).unwrap_or(&state);
                        if current.as_ref().is_none_or(|s| s.hash != source.base_hash) {
                            return Err(vec![
                                Diagnostic::error(
                                    codes::STALE_BASE,
                                    "source changed since analysis; analyze it again",
                                )
                                .file(&source.path),
                            ]);
                        }
                        let next = store.put(source.text.as_bytes()).map_err(|d| vec![d])?;
                        staged.insert(source.path.clone(), Some(next));
                        inventory.entries.entry(source.path).or_insert(state);
                    }
                    store.preflight_ops(&inverse).map_err(|d| vec![d])?;
                    for op in inverse {
                        if let Op::File { path, prev, .. } = op {
                            inventory.entries.entry(path).or_insert(prev);
                        }
                    }
                    let parents: Vec<_> = inventory
                        .entries
                        .keys()
                        .map(|path| {
                            path.rsplit_once('/')
                                .map_or("", |(parent, _)| parent)
                                .to_owned()
                        })
                        .collect();
                    for parent in parents {
                        inventory.directories.insert(
                            parent.clone(),
                            store.directory_entries(&parent).map_err(|d| vec![d])?,
                        );
                    }
                    store.revalidate(&inventory).map_err(|d| vec![d])?;
                    Ok(inventory)
                })();
                let _ = send.send(result);
            })
            .map_err(|e| vec![Diagnostic::error(codes::IO, e.to_string())])?;
        let job = self.start_job(method, Some(sid.to_owned()));
        self.file_jobs.pending.push(Pending {
            job: job.clone(),
            session: sid.to_owned(),
            method: method.to_owned(),
            params: params.clone(),
            revision: self.store.revision(),
            ready,
            revalidated: false,
        });
        Ok(Some(json!({"ok":true,"job":job,"preparing":true})))
    }

    pub(crate) fn poll_file_jobs(&mut self) {
        let mut waiting = Vec::new();
        for mut pending in std::mem::take(&mut self.file_jobs.pending) {
            let prepared = match pending.ready.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    waiting.push(pending);
                    continue;
                }
                Err(_) => Err(vec![Diagnostic::error(
                    codes::IO,
                    "file preparation worker disconnected",
                )]),
            };
            if self
                .jobs
                .get(&pending.job)
                .is_none_or(|j| j.status != crate::jobs::JobStatus::Running)
            {
                continue;
            }
            // A distinct validation phase runs after preparation, immediately before
            // the authority turn that commits. Owners can use the same hook for
            // opaque worker/policy metadata without importer knowledge in the host.
            if !pending.revalidated
                && let Ok(inventory) = prepared.as_ref()
            {
                let inventory = inventory.clone();
                let history = FileHistory::new(self.root.clone());
                let (send, ready) = mpsc::channel();
                match std::thread::Builder::new()
                    .name("ara-file-revalidate".into())
                    .spawn(move || {
                        let checked = history
                            .revalidate(&inventory)
                            .map(|()| inventory)
                            .map_err(|d| vec![d]);
                        let _ = send.send(checked);
                    }) {
                    Ok(_) => {
                        pending.ready = ready;
                        pending.revalidated = true;
                        waiting.push(pending);
                    }
                    Err(error) => self.finish_job(
                        &pending.job,
                        Err(vec![Diagnostic::error(codes::IO, error.to_string())]),
                    ),
                }
                continue;
            }
            let result = prepared.and_then(|states| {
                if self.store.revision() != pending.revision {
                    return Err(vec![Diagnostic::error(
                        codes::STALE_BASE,
                        "project changed while preparing file edits; retry the command",
                    )]);
                }
                if self.sessions.live(&pending.session).is_none() {
                    return Err(vec![Diagnostic::error(
                        codes::SESSION_NOT_FOUND,
                        "file command session ended during preparation",
                    )]);
                }
                self.store.prepare_files(states).map_err(|d| vec![d])?;
                self.file_jobs.applying = true;
                self.calling = Some(pending.session.clone());
                self.store.set_origin(Some(pending.session.clone()));
                let result = crate::core::Core.call(
                    &mut Cx {
                        st: self,
                        conn: ConnId::LOCAL,
                        session: Some(pending.session.clone()),
                    },
                    &pending.method,
                    pending.params,
                );
                self.drain();
                self.store.clear_prepared_files();
                self.file_jobs.applying = false;
                self.calling = None;
                self.store.set_origin(None);
                result
            });
            self.finish_job(&pending.job, result);
        }
        waiting.append(&mut self.file_jobs.pending);
        self.file_jobs.pending = waiting;
    }

    /// Recover a journal's ordered file transitions before rebuilding document history.
    /// The last matching state per path identifies a partial flush; unknown external
    /// bytes are retained and diagnosed instead of being overwritten.
    pub(crate) fn recover_file_states(&mut self, entries: &[Entry]) {
        let ops: Vec<_> = entries
            .iter()
            .filter_map(|e| {
                if let Entry::Txn { txn, .. } = e {
                    Some(&txn.ops)
                } else {
                    None
                }
            })
            .flatten()
            .filter(|op| matches!(op, Op::File { .. }))
            .cloned()
            .collect();
        if ops.is_empty() {
            return;
        }
        let root = self.root.clone();
        let task = std::thread::Builder::new().name("ara-file-recovery".into()).spawn(move || {
            let history = FileHistory::new(root);
            let mut paths: BTreeMap<String, FileTransitions> = BTreeMap::new();
            for (n, op) in ops.iter().enumerate() { if let Op::File { path, state, prev, .. } = op { paths.entry(path.clone()).or_default().push((n, prev.clone(), state.clone())); } }
            let mut positions = BTreeMap::new();
            let mut diagnostics = Vec::new();
            for (path, changes) in &paths {
                match history.observe(path) {
                    Ok(actual) => {
                        let position = changes.iter().rev().find(|(_, _, state)| state == &actual).map(|(n, _, _)| n + 1)
                            .or_else(|| (changes[0].1 == actual).then_some(0));
                        if let Some(position) = position { positions.insert(path.clone(), position); }
                        else { diagnostics.push(Diagnostic::warning(codes::EXTERNAL_CONFLICT, "file differs from every retained history state; kept external bytes").file(path)); }
                    }
                    Err(d) => diagnostics.push(d),
                }
            }
            for (n, op) in ops.iter().enumerate() {
                if let Op::File { path, state, .. } = op && positions.get(path).is_some_and(|position| n >= *position)
                    && let Err(d) = history.restore(path, state.as_ref()) { diagnostics.push(d); positions.remove(path); }
            }
            let states = paths.keys().filter_map(|path| match history.observe(path) { Ok(state) => Some((path.clone(), state)), Err(d) => { diagnostics.push(d); None } }).collect::<Observed>();
            (states, diagnostics)
        });
        match task.and_then(|t| {
            t.join()
                .map_err(|_| std::io::Error::other("file recovery worker panicked"))
        }) {
            Ok((states, diagnostics)) => {
                for (path, state) in states {
                    self.store.observe_file(path, state);
                }
                for d in diagnostics {
                    self.log(d);
                }
            }
            Err(e) => self.log(Diagnostic::error(codes::IO, e.to_string())),
        }
    }
}
