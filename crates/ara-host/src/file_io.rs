//! The host's file thread. Every document write and removal, journal append and
//! compaction, `.ara/diagnostics.jsonl` write, external-edit check and project scan
//! runs here, off the thread that ticks the host (the editor's UI thread): on a
//! large project a scan took ~100 ms every two seconds, a canonical write of a large
//! scene tens of milliseconds, and during a cold import a single command's journal
//! append once stalled the editor for four seconds. Requests run in the order the
//! host sends them, so the journal is written in commit order and a document's file
//! never gets ahead of the transaction the journal holds for it.
//!
//! OS change notifications (`notify`) drive the checks, with a slow full rescan as
//! the safety net; without notifications it polls at the configured cadence. As the
//! only writer it records each save's stamp, so the host's own saves never come back
//! as external edits, and saves and removals of one file happen in request order.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Document};
use ara_schema::SchemaDb;
use notify::Watcher as _;

use crate::journal::{Entry, Journal};

/// How often a full check and rescan runs while OS notifications are active.
const RESCAN: Duration = Duration::from_secs(30);

/// What a file looked like when last seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// The watched incarnation that observed a deletion, before the file disappeared.
#[derive(Debug, Clone)]
pub(crate) struct Deletion {
    generation: Option<u64>,
    stamp: Option<Stamp>,
}

#[derive(Debug)]
struct Watched {
    generation: Option<u64>,
    stamp: Option<Stamp>,
}

/// The file's modified time and size, or None when it does not exist.
pub(crate) fn stamp(path: &Path) -> Option<Stamp> {
    let m = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// A document snapshot to write, and the store revision it holds.
#[derive(Debug)]
pub(crate) struct Save {
    pub id: AssetId,
    /// Project-relative path.
    pub path: String,
    pub doc: Document,
    pub rev: u64,
}

/// A `.ara/diagnostics.jsonl` write the host prepared (`diag::Persist`): lines to
/// append, or the whole file. `seqs` are the records it persists, marked changed
/// again when the write fails.
#[derive(Debug)]
pub(crate) struct DiagWrite {
    pub path: PathBuf,
    pub text: String,
    pub append: bool,
    pub seqs: Vec<u64>,
}

/// What the host asks of the thread.
enum Request {
    /// Append an entry to the journal and flush it to the OS.
    Append {
        entry: Entry,
        generations: Vec<(String, Option<u64>)>,
    },
    /// Compact the journal (`Journal::compact`), then answer `Compacted`.
    Compact,
    /// Write `.ara/diagnostics.jsonl`.
    Diagnostics(DiagWrite),
    /// Answer `Settled` once everything requested before is done.
    Settle,
    /// A tagged nonblocking physical-completion boundary.
    Barrier(crate::files::FileCommitReceipt),
    /// Block the thread until `release` sends or closes, after telling `entered`.
    #[cfg(test)]
    Hold {
        entered: Sender<()>,
        release: Receiver<()>,
    },
    /// Watch a loaded document's file, as it was when read.
    Track {
        path: String,
        stamp: Option<Stamp>,
        generation: Option<u64>,
    },
    /// Stop watching a file (its document was unloaded).
    Untrack { path: String },
    /// Write a document canonically (temp file, then rename).
    Save { save: Save, generation: Option<u64> },
    /// Delete a document's file (the document was deleted, and that is flushed).
    Remove { id: AssetId, path: String, rev: u64 },
    /// Check every watched file and scan for new ones now, then answer `Done`.
    Sync,
    /// An OS change notification.
    Notified(notify::Result<notify::Event>),
}

/// What the thread reports; the host applies it on its own thread.
#[derive(Debug)]
pub(crate) enum Reply {
    /// Committed file transitions finished on the file worker.
    Materialized {
        revision: u64,
        result: Result<(), Diagnostic>,
    },
    /// A watched file's contents changed on disk (possibly back to what the host
    /// last wrote: compare before merging).
    Changed { path: String, text: String },
    /// A watched file disappeared.
    Deleted { path: String, observed: Deletion },
    /// A document file appeared that nothing watches. Reported again only when it
    /// changes; `track` it once loaded.
    Found {
        path: String,
        text: String,
        stamp: Option<Stamp>,
    },
    /// A `*.types.json` file changed (types load at startup only).
    TypesChanged { path: PathBuf },
    /// A save finished: the file now holds the returned text, or nothing changed.
    Saved {
        save: Save,
        result: Result<String, Diagnostic>,
    },
    /// A removal finished.
    Removed {
        id: AssetId,
        path: String,
        rev: u64,
        result: Result<(), Diagnostic>,
    },
    /// Everything requested before the matching `sync` is done.
    Done,
    /// A compaction finished: the journal now holds `entries` entries.
    Compacted {
        entries: usize,
        result: Result<(), Diagnostic>,
    },
    /// A journal append failed.
    JournalFailed(Diagnostic),
    /// A diagnostics write failed; its records need writing again.
    DiagnosticsFailed { seqs: Vec<u64>, error: String },
    /// Everything requested before the matching `settle` is done.
    Settled,
    /// Everything before this tagged boundary has completed on the worker.
    Barrier(crate::files::FileCommitReceipt),
}

/// The host's handle on its file thread. Dropping it lets queued saves finish, then
/// joins the thread.
pub(crate) struct FileThread {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
    thread: Option<JoinHandle<()>>,
    watcher: Option<notify::RecommendedWatcher>,
    /// Saves and removals sent and not answered yet.
    pending: usize,
    /// Journal entries, counting appends still queued.
    journal_len: usize,
    /// Appends sent since the compaction in progress, if one is.
    compacting: Option<usize>,
    disconnected: bool,
    /// Host intent advances before queued materialization or saves can report echoes.
    generations: HashMap<String, Option<u64>>,
    next_generation: Option<u64>,
}

impl std::fmt::Debug for FileThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileThread")
            .field("pending", &self.pending)
            .field("journal_len", &self.journal_len)
            .field("notified", &self.watcher.is_some())
            .finish()
    }
}

impl FileThread {
    /// Start watching `root` and writing `journal`. With `watch_every` zero the
    /// thread only looks when asked (`sync`); otherwise OS notifications drive it,
    /// or, when they are unavailable, polling every `watch_every` (watched files) and
    /// `scan_every` (new files).
    pub(crate) fn start(
        root: PathBuf,
        db: SchemaDb,
        type_files: Vec<(PathBuf, Option<SystemTime>)>,
        journal: Journal,
        watch_every: Duration,
        scan_every: Duration,
    ) -> std::io::Result<Self> {
        let journal_len = journal.len();
        let (requests, inbox) = mpsc::channel();
        let (outbox, replies) = mpsc::channel();
        let watcher = if watch_every.is_zero() {
            None
        } else {
            let notify_to = requests.clone();
            notify::recommended_watcher(move |event| {
                let _ = notify_to.send(Request::Notified(event));
            })
            .and_then(|mut w| w.watch(&root, notify::RecursiveMode::Recursive).map(|()| w))
            .ok()
        };
        let cadence = match (watch_every.is_zero(), watcher.is_some()) {
            (true, _) => None,
            (false, true) => Some((RESCAN, RESCAN)),
            (false, false) => Some((watch_every, scan_every)),
        };
        let worker = Worker {
            extensions: crate::project::document_extensions(&db),
            root,
            db,
            types: type_files,
            journal,
            watched: HashMap::new(),
            unwatched: HashMap::new(),
            replies: outbox,
        };
        let thread = std::thread::Builder::new()
            .name("ara-host files".into())
            .spawn(move || worker.run(&inbox, cadence))?;
        Ok(FileThread {
            requests,
            replies,
            thread: Some(thread),
            watcher,
            pending: 0,
            journal_len,
            compacting: None,
            disconnected: false,
            generations: HashMap::new(),
            next_generation: Some(1),
        })
    }

    /// Queue a journal entry; a failure comes back as `JournalFailed`.
    pub(crate) fn append(&mut self, entry: Entry) {
        if matches!(&entry, Entry::Txn { txn, .. } if txn.ops.iter().any(|op| matches!(op, ara_doc::Op::File { .. })))
        {
            self.pending += 1;
        }
        self.journal_len += 1;
        if let Some(since) = &mut self.compacting {
            *since += 1;
        }
        let generations = if let Entry::Txn { txn, .. } = &entry {
            let mut paths = std::collections::BTreeMap::new();
            for op in &txn.ops {
                if let ara_doc::Op::File { path, .. } | ara_doc::Op::Document { path, .. } = op {
                    paths.insert(ara_doc::file_state::path_key(path), path.clone());
                }
            }
            paths
                .into_values()
                .map(|path| {
                    let generation = self.advance(&path);
                    (path, generation)
                })
                .collect()
        } else {
            Vec::new()
        };
        self.send(Request::Append { entry, generations });
    }
    /// Queue a journal compaction; `Compacted` answers it.
    pub(crate) fn compact(&mut self) {
        self.compacting = Some(0);
        self.send(Request::Compact);
    }
    /// Journal entries, counting queued appends.
    pub(crate) fn journal_len(&self) -> usize {
        self.journal_len
    }
    /// Whether a compaction is queued or running.
    pub(crate) fn compacting(&self) -> bool {
        self.compacting.is_some()
    }
    /// Queue a diagnostics file write; a failure comes back as `DiagnosticsFailed`.
    pub(crate) fn diagnostics(&self, write: DiagWrite) {
        self.send(Request::Diagnostics(write));
    }
    /// Ask for `Settled` once everything queued so far is done.
    pub(crate) fn settle(&self) {
        self.send(Request::Settle);
    }
    /// Queues a tagged acknowledgement, without waiting for the worker.
    pub(crate) fn barrier(
        &mut self,
        receipt: crate::files::FileCommitReceipt,
    ) -> Result<(), Diagnostic> {
        self.requests.send(Request::Barrier(receipt)).map_err(|_| {
            self.disconnected = true;
            Diagnostic::error(codes::IO, "the project file worker is unavailable")
        })
    }
    pub(crate) fn disconnected(&self) -> bool {
        self.disconnected
    }
    /// Block the thread until the returned sender sends or drops; returns once the
    /// thread is held (tests: the host must not wait for it).
    #[cfg(test)]
    pub(crate) fn hold(&self) -> Sender<()> {
        let (entered, held) = mpsc::channel();
        let (release, released) = mpsc::channel();
        self.send(Request::Hold {
            entered,
            release: released,
        });
        let _ = held.recv();
        release
    }

    /// Watch a loaded document's file from `stamp` (what it was when read).
    pub(crate) fn track(&mut self, path: String, stamp: Option<Stamp>) {
        let generation = self.advance(&path);
        self.send(Request::Track {
            path,
            stamp,
            generation,
        });
    }
    /// Stop watching a file.
    pub(crate) fn untrack(&mut self, path: String) {
        self.send(Request::Untrack { path });
    }
    /// Queue a save; its `Saved` reply arrives through `try_reply`/`reply`.
    pub(crate) fn save(&mut self, save: Save) {
        self.pending += 1;
        let generation = self.advance(&save.path);
        self.send(Request::Save { save, generation });
    }
    /// Queue a file removal (after any queued save of it).
    pub(crate) fn remove(&mut self, id: AssetId, path: String, rev: u64) {
        self.pending += 1;
        let _ = self.advance(&path);
        self.send(Request::Remove { id, path, rev });
    }

    fn advance(&mut self, path: &str) -> Option<u64> {
        let generation = self.next_generation;
        self.next_generation = generation.and_then(|generation| generation.checked_add(1));
        self.generations
            .insert(ara_doc::file_state::path_key(path), generation);
        generation
    }

    /// Reject observations made before a newer owned transition or track request.
    /// No filesystem access occurs on the authority/UI thread.
    pub(crate) fn current_deletion(
        &self,
        path: &str,
        observed: &Deletion,
    ) -> Result<bool, Diagnostic> {
        let Some(generation) = observed.generation else {
            return Err(Diagnostic::error(
                codes::IO,
                "file-watch observation generation exhausted; restart the project authority",
            ));
        };
        Ok(observed.stamp.is_some()
            && self.generations.get(&ara_doc::file_state::path_key(path))
                == Some(&Some(generation)))
    }

    #[cfg(test)]
    pub(crate) fn deletion_observation(&self, path: &str, stamp: Option<Stamp>) -> Deletion {
        Deletion {
            generation: self
                .generations
                .get(&ara_doc::file_state::path_key(path))
                .copied()
                .flatten(),
            stamp,
        }
    }
    /// Ask for a full check now; `Done` follows its findings.
    pub(crate) fn sync(&self) {
        self.send(Request::Sync);
    }
    /// Saves and removals not answered yet.
    pub(crate) fn pending(&self) -> usize {
        self.pending
    }
    /// The next reply, if one is waiting.
    pub(crate) fn try_reply(&mut self) -> Option<Reply> {
        let reply = match self.replies.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.disconnected = true;
                return None;
            }
            Err(mpsc::TryRecvError::Empty) => return None,
        };
        Some(self.count(reply))
    }
    /// The next reply, waiting for it; None if the thread is gone.
    pub(crate) fn reply(&mut self) -> Option<Reply> {
        let reply = match self.replies.recv() {
            Ok(reply) => reply,
            Err(_) => {
                self.disconnected = true;
                return None;
            }
        };
        Some(self.count(reply))
    }
    fn count(&mut self, reply: Reply) -> Reply {
        match &reply {
            Reply::Saved { .. } | Reply::Removed { .. } | Reply::Materialized { .. } => {
                self.pending = self.pending.saturating_sub(1);
            }
            Reply::Compacted { entries, .. } => {
                // Appends sent after the compaction land after it.
                self.journal_len = entries + self.compacting.take().unwrap_or(0);
            }
            _ => {}
        }
        reply
    }
    fn send(&self, request: Request) {
        // The thread only stops when this handle drops.
        let _ = self.requests.send(request);
    }
}

impl Drop for FileThread {
    fn drop(&mut self) {
        // Notifications stop first; the closed channel then ends the thread once
        // the queued requests are done.
        self.watcher = None;
        let (closed, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.requests, closed));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread's state: what it watches and how each file last looked.
struct Worker {
    root: PathBuf,
    db: SchemaDb,
    extensions: HashSet<String>,
    types: Vec<(PathBuf, Option<SystemTime>)>,
    journal: Journal,
    /// Loaded documents' files (project-relative) and their last seen stamp.
    watched: HashMap<String, Watched>,
    /// Document files nothing loads, as last reported.
    unwatched: HashMap<String, Option<Stamp>>,
    replies: Sender<Reply>,
}

impl Worker {
    /// Serve requests until the host drops its handle; with a `(check, scan)`
    /// cadence, also check watched files and scan for new ones on their own.
    fn run(mut self, inbox: &Receiver<Request>, cadence: Option<(Duration, Duration)>) {
        let now = Instant::now();
        let mut next = cadence.map(|(check, scan)| (now + check, now + scan));
        loop {
            let request = match next {
                None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some((check, scan)) => {
                    inbox.recv_timeout(check.min(scan).saturating_duration_since(Instant::now()))
                }
            };
            match request {
                Ok(request) => self.handle(request),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if let (Some((check_every, scan_every)), Some((check, scan))) = (cadence, &mut next) {
                let now = Instant::now();
                if now >= *check {
                    self.check_watched();
                    *check = now + check_every;
                }
                if now >= *scan {
                    self.scan();
                    *scan = now + scan_every;
                }
            }
        }
    }

    fn handle(&mut self, request: Request) {
        match request {
            Request::Append { entry, generations } => {
                let result = self.journal.append(&entry).map_err(|e| {
                    Diagnostic::error(codes::IO, format!("cannot append to `.ara/journal`: {e}"))
                });
                if result.is_ok() {
                    for (path, generation) in &generations {
                        if let Some(watched) = self.watched.get_mut(path) {
                            watched.generation = *generation;
                        }
                    }
                }
                if let Entry::Txn { txn, .. } = &entry
                    && txn
                        .ops
                        .iter()
                        .any(|op| matches!(op, ara_doc::Op::File { .. }))
                {
                    let result = result.and_then(|()| {
                        crate::file_state::FileHistory::new(&self.root).materialize(&txn.ops)?;
                        // Owned FileOps are writes too. Refresh their watched
                        // baselines before queued OS notifications can echo a
                        // move's deletion against the document's new location.
                        for (path, generation) in &generations {
                            if let Some(watched) = self.watched.get_mut(path) {
                                watched.generation = *generation;
                                watched.stamp = stamp(&self.root.join(path));
                            }
                        }
                        // A completed flush is distinct from a journaled intent:
                        // recovery must preserve later external edits, even when
                        // they restore bytes seen in an older transaction.
                        let mut final_states = std::collections::BTreeMap::new();
                        for op in &txn.ops {
                            if let ara_doc::Op::File { path, state, .. } = op {
                                final_states
                                    .insert(ara_doc::file_state::path_key(path), (path, state));
                            }
                        }
                        for (path, state) in final_states.into_values() {
                            self.journal
                                .append(&Entry::FileFlush {
                                    path: path.clone(),
                                    rev: txn.result,
                                    state: state.clone(),
                                })
                                .map_err(|e| {
                                    Diagnostic::error(
                                        codes::IO,
                                        format!("cannot record file flush: {e}"),
                                    )
                                })?;
                        }
                        Ok(())
                    });
                    self.reply(Reply::Materialized {
                        revision: txn.result,
                        result,
                    });
                } else if let Err(d) = result {
                    self.reply(Reply::JournalFailed(d));
                }
            }
            Request::Compact => {
                let result = self.journal.compact().map_err(|e| {
                    Diagnostic::warning(codes::IO, format!("cannot compact `.ara/journal`: {e}"))
                });
                self.reply(Reply::Compacted {
                    entries: self.journal.len(),
                    result,
                });
            }
            Request::Diagnostics(write) => {
                if let Err(e) = write_diagnostics(&write) {
                    self.reply(Reply::DiagnosticsFailed {
                        seqs: write.seqs,
                        error: e.to_string(),
                    });
                }
            }
            Request::Settle => self.reply(Reply::Settled),
            Request::Barrier(receipt) => self.reply(Reply::Barrier(receipt)),
            #[cfg(test)]
            Request::Hold { entered, release } => {
                let _ = entered.send(());
                // Bounded, so a host that does wait fails its test instead of hanging.
                let _ = release.recv_timeout(Duration::from_secs(30));
            }
            Request::Track {
                path,
                stamp,
                generation,
            } => {
                self.unwatched.remove(&path);
                self.watched.insert(path, Watched { generation, stamp });
            }
            Request::Untrack { path } => {
                self.watched.remove(&path);
            }
            Request::Save { save, generation } => {
                let result = self.write(&save);
                if result.is_ok() {
                    let stamp = stamp(&self.root.join(&save.path));
                    self.unwatched.remove(&save.path);
                    self.watched
                        .insert(save.path.clone(), Watched { generation, stamp });
                }
                self.reply(Reply::Saved { save, result });
            }
            Request::Remove { id, path, rev } => {
                self.watched.remove(&path);
                let result = match fs::remove_file(self.root.join(&path)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Diagnostic::error(
                        codes::FLUSH_FAILED,
                        format!("cannot remove `{path}`: {e}"),
                    )
                    .file(path.clone())),
                    _ => Ok(()),
                };
                self.reply(Reply::Removed {
                    id,
                    path,
                    rev,
                    result,
                });
            }
            Request::Sync => {
                self.check_watched();
                self.scan();
                self.reply(Reply::Done);
            }
            Request::Notified(Ok(event)) if !event.need_rescan() => {
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }
                for path in &event.paths {
                    self.notified(path);
                }
            }
            // A dropped or overflowing notification stream: look at everything.
            Request::Notified(_) => {
                self.check_watched();
                self.scan();
            }
        }
    }

    /// A notification named `path`: check it if watched, report it if it is a new
    /// document, recheck type files if it is one.
    fn notified(&mut self, path: &Path) {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return;
        };
        let key = slash(rel);
        if self.watched.contains_key(&key) {
            self.check(&key);
        } else if crate::project::is_document(rel, &self.extensions)
            && crate::project::walked(&self.root, rel)
        {
            self.discover(key);
        }
        if self.types.iter().any(|(p, _)| p == path) {
            self.check_types();
        }
    }

    /// Canonical text of the save, written through a temp file and a rename.
    fn write(&self, save: &Save) -> Result<String, Diagnostic> {
        let path = &save.path;
        let abs = self.root.join(path);
        let failed = |what: String| Diagnostic::error(codes::FLUSH_FAILED, what).file(path.clone());
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| failed(format!("cannot create `{path}` parent: {e}")))?;
        }
        let text = crate::ui_document::validate(&save.doc, path)
            .and_then(|()| crate::project_settings::write(&save.doc, &self.db))
            .map_err(|d| d.file(path.clone()))?;
        if let Ok(existing) = fs::read_to_string(&abs) {
            if existing == text {
                return Ok(existing);
            }
            let read = crate::project_settings::read(&existing, path, &self.db, None);
            if let Some(doc) = read.doc.filter(|_| !ara_diag::has_errors(&read.diags))
                && crate::files::equivalent(&doc, &save.doc, &self.db)
            {
                // A file op may just have restored its immutable snapshot. Keep
                // those exact bytes and acknowledge them as the write receipt;
                // canonicalizing them again would invalidate the Redo base hash.
                return Ok(existing);
            }
        }
        let tmp = abs.with_extension("ara-tmp");
        fs::write(&tmp, &text)
            .and_then(|()| fs::rename(&tmp, &abs))
            .map_err(|e| failed(format!("cannot write `{path}`: {e}")))?;
        Ok(text)
    }

    fn check_watched(&mut self) {
        let paths: Vec<String> = self.watched.keys().cloned().collect();
        for path in paths {
            self.check(&path);
        }
        self.check_types();
    }

    /// Report a watched file that changed or disappeared since it was last seen.
    fn check(&mut self, path: &str) {
        let Some(known) = self.watched.get(path) else {
            return;
        };
        let now = stamp(&self.root.join(path));
        if now == known.stamp {
            return;
        }
        match now {
            None => {
                let observed = Deletion {
                    generation: known.generation,
                    stamp: known.stamp.clone(),
                };
                if let Some(watched) = self.watched.get_mut(path) {
                    watched.stamp = None;
                }
                self.reply(Reply::Deleted {
                    path: path.to_owned(),
                    observed,
                });
            }
            Some(_) => {
                // Unreadable mid-write: the next check or notification retries.
                let Ok(text) = fs::read_to_string(self.root.join(path)) else {
                    return;
                };
                if let Some(watched) = self.watched.get_mut(path) {
                    watched.stamp = now;
                }
                self.reply(Reply::Changed {
                    path: path.to_owned(),
                    text,
                });
            }
        }
    }

    /// Report document files nothing watches yet (new, or changed since reported).
    fn scan(&mut self) {
        for abs in crate::project::document_files(&self.root, &self.extensions) {
            let Ok(rel) = abs.strip_prefix(&self.root) else {
                continue;
            };
            let key = slash(rel);
            if !self.watched.contains_key(&key) {
                self.discover(key);
            }
        }
        self.check_types();
    }

    fn discover(&mut self, path: String) {
        let abs = self.root.join(&path);
        let now = stamp(&abs);
        if now.is_none() || self.unwatched.get(&path) == Some(&now) {
            return;
        }
        let Ok(text) = fs::read_to_string(&abs) else {
            return;
        };
        self.unwatched.insert(path.clone(), now.clone());
        self.reply(Reply::Found {
            path,
            text,
            stamp: now,
        });
    }

    fn check_types(&mut self) {
        let mut changed = Vec::new();
        for (path, seen) in &mut self.types {
            let now = fs::metadata(&*path).and_then(|m| m.modified()).ok();
            if now != *seen {
                *seen = now;
                changed.push(path.clone());
            }
        }
        for path in changed {
            self.reply(Reply::TypesChanged { path });
        }
    }

    fn reply(&self, reply: Reply) {
        // A closed channel means the host is gone; its drop ends this thread.
        let _ = self.replies.send(reply);
    }
}

/// Append the lines, or replace the file through a temporary file and a rename.
fn write_diagnostics(write: &DiagWrite) -> std::io::Result<()> {
    if write.append {
        return fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&write.path)?
            .write_all(write.text.as_bytes());
    }
    let tmp = write.path.with_extension("jsonl.tmp");
    fs::write(&tmp, &write.text)?;
    fs::rename(&tmp, &write.path)
}

/// A relative path with `/` separators (the workspace's path form).
fn slash(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_file_delete_recreate_refreshes_watch_baseline_but_external_delete_is_reported() {
        let dir = std::env::var_os("ARA_TEST_ROOT")
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("owned-watch-baseline-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch");
        let path = "owned.txt";
        fs::write(dir.join(path), b"native file bytes").expect("source");
        let history = crate::file_state::FileHistory::new(&dir);
        let state = history.snapshot(path).expect("snapshot");
        let (journal, _, _) = Journal::open(&dir.join(".ara")).expect("journal");
        let mut io = FileThread::start(
            dir.clone(),
            SchemaDb::new(),
            Vec::new(),
            journal,
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("worker");
        io.track(path.into(), stamp(&dir.join(path)));
        for (revision, prev, next) in [(1, state.clone(), None), (2, None, state)] {
            let mut entry = txn(revision);
            if let Entry::Txn { txn, .. } = &mut entry {
                txn.ops.push(ara_doc::Op::File {
                    doc: AssetId::parse("pj_project").expect("project ID"),
                    id: ara_doc::RecordId::parse("f_owned").expect("file ID"),
                    path: path.into(),
                    prev,
                    state: next,
                });
            }
            io.append(entry);
            io.sync();
            loop {
                match io.reply().expect("worker reply") {
                    Reply::Materialized { result, .. } => result.expect("materialized"),
                    Reply::Deleted { .. } => {
                        panic!("owned transition must not echo as an external deletion")
                    }
                    Reply::Done => break,
                    _ => {}
                }
            }
        }
        fs::remove_file(dir.join(path)).expect("genuine external deletion");
        io.sync();
        let mut observed = false;
        loop {
            match io.reply().expect("worker reply") {
                Reply::Deleted {
                    path,
                    observed: deletion,
                } => {
                    assert!(
                        io.current_deletion(&path, &deletion)
                            .expect("live generation")
                    );
                    observed = true;
                }
                Reply::Done => break,
                _ => {}
            }
        }
        assert!(
            observed,
            "a genuine external deletion still reaches the authority"
        );
    }

    fn txn(n: u64) -> Entry {
        Entry::Txn {
            untracked: false,
            session: None,
            txn: serde_json::from_value(serde_json::json!({
                "id": format!("tx_{n}"), "author": "agent:a", "base": n - 1, "result": n, "ops": []
            }))
            .expect("transaction"),
        }
    }

    /// Appends queued behind a compaction land after its rewrite, and the host's
    /// count of entries agrees with the file once `Compacted` arrives.
    #[test]
    fn appends_after_a_compaction_follow_it() {
        let dir = std::env::temp_dir().join(format!("ara-host-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (journal, _, _) = Journal::open(&dir).expect("journal");
        let mut io = FileThread::start(
            dir.clone(),
            SchemaDb::new(),
            Vec::new(),
            journal,
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("file thread");
        (1..=3).for_each(|n| io.append(txn(n)));
        io.compact();
        (4..=5).for_each(|n| io.append(txn(n)));
        loop {
            match io.reply() {
                Some(Reply::Compacted { result, .. }) => break result.expect("compacted"),
                Some(_) => {}
                None => panic!("the file thread stopped"),
            }
        }
        let counted = io.journal_len();
        drop(io);
        let (_, entries, _) = Journal::open(&dir).expect("journal");
        assert_eq!(counted, entries.len());
        assert!(matches!(entries[0], Entry::Base { .. }));
        let ids: Vec<String> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Txn { txn, .. } => Some(txn.id.0.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["tx_1", "tx_2", "tx_3", "tx_4", "tx_5"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
