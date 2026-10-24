//! The host's file thread. Every document write and removal, external-edit check
//! and project scan runs here, off the thread that ticks the host (the editor's UI
//! thread): on a large project a scan took ~100 ms every two seconds and a canonical
//! write of a large scene tens of milliseconds.
//!
//! OS change notifications (`notify`) drive the checks, with a slow full rescan as
//! the safety net; without notifications it polls at the configured cadence. As the
//! only writer it records each save's stamp, so the host's own saves never come back
//! as external edits, and saves and removals of one file happen in request order.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Document};
use ara_schema::SchemaDb;
use notify::Watcher as _;

/// How often a full check and rescan runs while OS notifications are active.
const RESCAN: Duration = Duration::from_secs(30);

/// What a file looked like when last seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
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

/// What the host asks of the thread.
enum Request {
    /// Watch a loaded document's file, as it was when read.
    Track { path: String, stamp: Option<Stamp> },
    /// Stop watching a file (its document was unloaded).
    Untrack { path: String },
    /// Write a document canonically (temp file, then rename).
    Save(Save),
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
    /// A watched file's contents changed on disk (possibly back to what the host
    /// last wrote: compare before merging).
    Changed { path: String, text: String },
    /// A watched file disappeared.
    Deleted { path: String },
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
}

impl std::fmt::Debug for FileThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileThread")
            .field("pending", &self.pending)
            .field("notified", &self.watcher.is_some())
            .finish()
    }
}

impl FileThread {
    /// Start watching `root`. With `watch_every` zero the thread only looks when
    /// asked (`sync`); otherwise OS notifications drive it, or, when they are
    /// unavailable, polling every `watch_every` (watched files) and `scan_every`
    /// (new files).
    pub(crate) fn start(
        root: PathBuf,
        db: SchemaDb,
        type_files: Vec<(PathBuf, Option<SystemTime>)>,
        watch_every: Duration,
        scan_every: Duration,
    ) -> std::io::Result<Self> {
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
        })
    }

    /// Watch a loaded document's file from `stamp` (what it was when read).
    pub(crate) fn track(&self, path: String, stamp: Option<Stamp>) {
        self.send(Request::Track { path, stamp });
    }
    /// Stop watching a file.
    pub(crate) fn untrack(&self, path: String) {
        self.send(Request::Untrack { path });
    }
    /// Queue a save; its `Saved` reply arrives through `try_reply`/`reply`.
    pub(crate) fn save(&mut self, save: Save) {
        self.pending += 1;
        self.send(Request::Save(save));
    }
    /// Queue a file removal (after any queued save of it).
    pub(crate) fn remove(&mut self, id: AssetId, path: String, rev: u64) {
        self.pending += 1;
        self.send(Request::Remove { id, path, rev });
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
        let reply = self.replies.try_recv().ok()?;
        Some(self.count(reply))
    }
    /// The next reply, waiting for it; None if the thread is gone.
    pub(crate) fn reply(&mut self) -> Option<Reply> {
        let reply = self.replies.recv().ok()?;
        Some(self.count(reply))
    }
    fn count(&mut self, reply: Reply) -> Reply {
        if matches!(reply, Reply::Saved { .. } | Reply::Removed { .. }) {
            self.pending = self.pending.saturating_sub(1);
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
    /// Loaded documents' files (project-relative) and their last seen stamp.
    watched: HashMap<String, Option<Stamp>>,
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
            Request::Track { path, stamp } => {
                self.unwatched.remove(&path);
                self.watched.insert(path, stamp);
            }
            Request::Untrack { path } => {
                self.watched.remove(&path);
            }
            Request::Save(save) => {
                let result = self.write(&save);
                if result.is_ok() {
                    let stamp = stamp(&self.root.join(&save.path));
                    self.unwatched.remove(&save.path);
                    self.watched.insert(save.path.clone(), stamp);
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
        if now == *known {
            return;
        }
        match now {
            None => {
                self.watched.insert(path.to_owned(), None);
                self.reply(Reply::Deleted {
                    path: path.to_owned(),
                });
            }
            Some(_) => {
                // Unreadable mid-write: the next check or notification retries.
                let Ok(text) = fs::read_to_string(self.root.join(path)) else {
                    return;
                };
                self.watched.insert(path.to_owned(), now);
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

/// A relative path with `/` separators (the workspace's path form).
fn slash(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
