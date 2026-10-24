//! "Always saved" persistence and external edits (R03 §6.9–6.10). The file I/O
//! runs on the host's file thread (`file_io`); this applies its findings.
//!
//! - **Flush:** a document edited by a transaction is written canonically once it
//!   has been quiet for the debounce time (temp file, then rename), and a `flush`
//!   entry with its revision and hash goes to the journal once the write lands.
//! - **Watch:** OS notifications (or polling) report changed files. A changed file
//!   is validated, then three-way merged (base: the last synced text, ours: memory,
//!   theirs: disk) and applied as a human transaction (`external edit`). Invalid
//!   files and conflicts with unsaved changes are reported and the document is
//!   marked diverged: nothing is overwritten until `doc.sync` or a clean edit.
//! - New document files are loaded; deleted ones are unloaded (or, with unsaved
//!   changes, kept and reported).

use std::collections::BTreeMap;
use std::fs;
use std::time::Instant;

use crate::file_io::{FileThread, Reply, Save, Stamp};
use crate::project_settings::read;
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Document, Edit, OrderKey, field};
use ara_ops::{Author, TxnMeta};
use ara_schema::FieldPath;
use serde_json::Value;

use crate::host::State;
use crate::journal::{self, Entry};
use crate::params::Keep;

/// The last state a document and its file agreed on.
#[derive(Debug, Clone)]
pub(crate) struct Synced {
    /// Project-relative path.
    pub path: String,
    /// The file's text.
    pub text: String,
    /// The document that text reads as: the merge base for external edits.
    pub doc: Document,
}

impl Synced {
    pub(crate) fn new(path: String, text: String, doc: Document) -> Self {
        Synced { path, text, doc }
    }
}

/// Why a document's file is not being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Diverged {
    /// The file on disk has errors.
    Invalid,
    /// The file changed in a way that conflicts with unsaved changes.
    Conflict,
    /// The file was deleted while the document had unsaved changes.
    Deleted,
}

/// Per-document persistence state.
#[derive(Debug)]
pub(crate) struct Files {
    /// Paths retained until an undo of document creation is flushed.
    pub deleted: BTreeMap<AssetId, String>,
    pub synced: BTreeMap<AssetId, Synced>,
    /// Documents changed by a transaction, with the time of the latest change.
    pub changed: BTreeMap<AssetId, Instant>,
    pub diverged: BTreeMap<AssetId, Diverged>,
    /// The thread that writes and watches the files.
    pub io: FileThread,
}

impl Files {
    pub(crate) fn new(io: FileThread) -> Self {
        Files {
            deleted: BTreeMap::new(),
            synced: BTreeMap::new(),
            changed: BTreeMap::new(),
            diverged: BTreeMap::new(),
            io,
        }
    }
}

/// The edits that turn `from` into `to`, per record and top-level key.
pub(crate) fn diff(from: &Document, to: &Document) -> Vec<Edit> {
    let doc = to.id.clone();
    let mut edits = Vec::new();
    for (id, rec) in to.records() {
        let Some(old) = from.record(id.as_str()) else {
            edits.push(Edit::Create {
                doc: doc.clone(),
                id: id.clone(),
                record: rec.clone(),
            });
            continue;
        };
        if old == rec {
            continue;
        }
        let moved = [field::PARENT, field::ORDER]
            .iter()
            .any(|k| old.get(*k) != rec.get(*k));
        if moved {
            edits.push(Edit::Move {
                doc: doc.clone(),
                id: id.clone(),
                parent: rec
                    .get(field::PARENT)
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                order: rec
                    .get(field::ORDER)
                    .and_then(Value::as_str)
                    .and_then(|s| OrderKey::parse(s).ok()),
            });
        }
        let mut keys: Vec<&String> = old.keys().chain(rec.keys()).collect();
        keys.sort();
        keys.dedup();
        for key in keys {
            if key == field::PARENT || key == field::ORDER || old.get(key) == rec.get(key) {
                continue;
            }
            edits.push(Edit::Set {
                doc: doc.clone(),
                id: id.clone(),
                path: FieldPath::field(key.as_str()),
                value: rec.get(key).cloned(),
            });
        }
    }
    for id in from.records().keys() {
        if to.record(id.as_str()).is_none() {
            edits.push(Edit::Delete {
                doc: doc.clone(),
                id: id.clone(),
            });
        }
    }
    edits
}

impl State {
    /// Queues writes of documents whose last change is older than the debounce time
    /// (all of them with `force`, which also waits until every write has landed).
    /// Diverged documents are skipped.
    pub(crate) fn flush(&mut self, force: bool) {
        let now = Instant::now();
        let due: Vec<AssetId> = self
            .files
            .changed
            .iter()
            .filter(|(_, t)| force || now.duration_since(**t) >= self.debounce)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.files.changed.remove(&id);
            if !self.files.diverged.contains_key(&id) {
                self.write_doc(&id, false);
            }
        }
        if force {
            self.wait_writes();
        }
        self.maybe_compact();
    }

    /// Queues a write of one document if it differs from its file (always with
    /// `always`); the file thread writes it, and `saved` journals that the file
    /// holds this revision. An unchanged document is journaled at once.
    pub(crate) fn write_doc(&mut self, id: &AssetId, always: bool) {
        let rev = self.store.revision();
        let Some(doc) = self.store.workspace().doc(id.as_str()) else {
            if let Some(path) = self.files.deleted.get(id).cloned() {
                if let Err(d) = crate::project_settings::check_parent(&self.root, &path) {
                    self.files.changed.insert(id.clone(), Instant::now());
                    self.log(d);
                    return;
                }
                self.files.io.remove(id.clone(), path, rev);
            }
            return;
        };
        let path = self
            .store
            .workspace()
            .path_of(id.as_str())
            .unwrap_or_default()
            .to_owned();
        if let Err(d) = crate::project_settings::check_parent(&self.root, &path) {
            self.files.changed.insert(id.clone(), Instant::now());
            self.log(d);
            return;
        }
        let synced = self.files.synced.get(id).filter(|s| s.doc == *doc);
        if let Some(synced) = synced.filter(|_| !always) {
            let hash = journal::hash(&synced.text);
            self.journal(&Entry::Flush {
                doc: id.to_string(),
                rev,
                hash,
            });
            return;
        }
        let doc = doc.clone();
        self.files.io.save(Save {
            id: id.clone(),
            path,
            doc,
            rev,
        });
    }

    /// Waits until every queued write and removal has landed, applying whatever else
    /// the file thread reports meanwhile.
    pub(crate) fn wait_writes(&mut self) {
        while self.files.io.pending() > 0 {
            let Some(reply) = self.files.io.reply() else {
                return;
            };
            self.file_reply(reply);
        }
    }

    /// Applies what the file thread found: external edits, deletions, new documents,
    /// changed type files and finished writes. With `watch_every` zero (tests) it
    /// first has the thread check everything and waits for the answer.
    pub(crate) fn watch(&mut self) {
        if self.watch_every.is_zero() {
            self.files.io.sync();
            while let Some(reply) = self.files.io.reply() {
                let done = matches!(reply, Reply::Done);
                self.file_reply(reply);
                if done {
                    break;
                }
            }
        }
        while let Some(reply) = self.files.io.try_reply() {
            self.file_reply(reply);
        }
    }

    fn file_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Changed { path, text } => {
                if let Some((id, synced)) = self.synced_at(&path)
                    && synced.text != text
                {
                    self.reconcile(&id, text);
                }
            }
            Reply::Deleted { path } => {
                if let Some((id, _)) = self.synced_at(&path) {
                    self.deleted(&id);
                }
            }
            Reply::Found { path, text, stamp } => self.found(path, text, stamp),
            Reply::TypesChanged { path } => {
                let rel = crate::project::rel_to(&self.root, &path);
                self.log(
                    Diagnostic::warning(
                        codes::TYPES_CHANGED,
                        format!("`{rel}` changed; restart the daemon to load the new types"),
                    )
                    .file(rel),
                );
            }
            Reply::Saved { save, result } => self.saved(save, result),
            Reply::Removed {
                id,
                path,
                rev,
                result,
            } => match result {
                Ok(()) => {
                    self.files.deleted.remove(&id);
                    self.files.synced.remove(&id);
                    self.files.diverged.remove(&id);
                    self.journal(&Entry::Flush {
                        doc: id.to_string(),
                        rev,
                        hash: journal::hash("deleted"),
                    });
                }
                Err(d) => {
                    self.files.changed.insert(id, Instant::now());
                    self.log(d.file(path));
                }
            },
            Reply::Done => {}
        }
    }

    /// A write landed: the file now agrees with the saved snapshot.
    fn saved(&mut self, save: Save, result: Result<String, Diagnostic>) {
        let Save { id, path, doc, rev } = save;
        match result {
            Ok(text) => {
                let hash = journal::hash(&text);
                self.files
                    .synced
                    .insert(id.clone(), Synced::new(path, text, doc));
                self.journal(&Entry::Flush {
                    doc: id.to_string(),
                    rev,
                    hash,
                });
            }
            Err(d) => {
                self.files.changed.insert(id, Instant::now());
                self.log(d);
            }
        }
    }

    /// The loaded document whose file is `path`, with its synced state.
    fn synced_at(&self, path: &str) -> Option<(AssetId, &Synced)> {
        self.files
            .synced
            .iter()
            .find(|(_, s)| s.path == path)
            .map(|(id, s)| (id.clone(), s))
    }

    fn deleted(&mut self, id: &AssetId) {
        let Some(synced) = self.files.synced.get(id) else {
            return;
        };
        let path = synced.path.clone();
        let dirty = self
            .store
            .workspace()
            .doc(id.as_str())
            .is_some_and(|d| *d != synced.doc);
        if dirty {
            if self
                .files
                .diverged
                .insert(id.clone(), crate::files::Diverged::Deleted)
                != Some(Diverged::Deleted)
            {
                self.log(Diagnostic::warning(
                    codes::DOC_DELETED,
                    format!("`{path}` was deleted on disk but has unsaved changes; kept in memory. `doc.sync` with `keep: memory` writes it back, `keep: disk` unloads it"),
                ).file(path));
            }
        } else {
            self.unload(id);
            self.log(
                Diagnostic::info(
                    codes::DOC_DELETED,
                    format!("`{path}` was deleted on disk; unloaded it"),
                )
                .file(path),
            );
        }
    }

    fn unload(&mut self, id: &AssetId) {
        self.store.unload_doc(id.as_str());
        if let Some(synced) = self.files.synced.remove(id) {
            self.files.io.untrack(synced.path);
        }
        self.files.changed.remove(id);
        self.files.diverged.remove(id);
    }

    /// Loads a document file that appeared on disk (the file thread found it).
    fn found(&mut self, rel: String, text: String, stamp: Option<Stamp>) {
        let result = read(
            &text,
            &rel,
            self.store.schema(),
            Some(self.store.workspace().index()),
        );
        let Some(doc) = result.doc.filter(|_| !ara_diag::has_errors(&result.diags)) else {
            self.log(
                Diagnostic::warning(
                    codes::EXTERNAL_INVALID,
                    format!("new document `{rel}` has errors; not loaded (see `ara check`)"),
                )
                .file(rel),
            );
            return;
        };
        if self.store.workspace().doc(doc.id.as_str()).is_some() {
            self.log(Diagnostic::warning(
                codes::EXTERNAL_INVALID,
                format!("new document `{rel}` reuses the asset ID `{}` of a loaded document; not loaded", doc.id),
            ).file(rel));
            return;
        }
        self.files.io.track(rel.clone(), stamp);
        self.files
            .synced
            .insert(doc.id.clone(), Synced::new(rel.clone(), text, doc.clone()));
        let source = crate::project::asset_path(&rel, &doc.kind);
        self.store
            .load_doc_with_asset_path(rel.clone(), doc, source);
        self.log(
            Diagnostic::info(codes::EXTERNAL_EDIT, format!("loaded new document `{rel}`"))
                .file(rel),
        );
    }

    /// Brings an external edit of a loaded document into memory (see module docs).
    fn reconcile(&mut self, id: &AssetId, text: String) {
        let Some(synced) = self.files.synced.get(id) else {
            return;
        };
        let path = synced.path.clone();
        let base = synced.doc.clone();
        let db = self.store.schema();
        let result = read(&text, &path, db, Some(self.store.workspace().index()));
        let theirs = match result.doc {
            Some(doc) if !ara_diag::has_errors(&result.diags) => doc,
            _ => {
                self.files.diverged.insert(id.clone(), Diverged::Invalid);
                self.log(Diagnostic::warning(
                    codes::EXTERNAL_INVALID,
                    format!("`{path}` changed on disk but has errors; keeping the loaded version until the file is fixed (`doc.sync` with `keep: memory` overwrites it)"),
                ).file(path.clone()));
                for d in result.diags.into_iter().filter(Diagnostic::is_error) {
                    self.log(d);
                }
                return;
            }
        };
        let Some(ours) = self.store.workspace().doc(id.as_str()).cloned() else {
            return;
        };
        let merged = match ara_merge::merge(&base, &ours, &theirs, db) {
            Ok(m) => m,
            Err(d) => {
                self.files.diverged.insert(id.clone(), Diverged::Invalid);
                self.log(d.file(path));
                return;
            }
        };
        if !merged.conflicts.is_empty() {
            self.files.diverged.insert(id.clone(), Diverged::Conflict);
            let fields: Vec<String> = merged
                .conflicts
                .iter()
                .map(|c| format!("{}.{}", c.record, c.path))
                .collect();
            self.log(Diagnostic::warning(
                codes::EXTERNAL_CONFLICT,
                format!("`{path}` changed on disk and conflicts with unsaved changes at {}; nothing was overwritten. Edit the file to resolve, or `doc.sync` with `keep: memory` or `keep: disk`", fields.join(", ")),
            ).file(path));
            return;
        }
        let mut merged_doc = merged.doc;
        merged_doc.conflicts.clear();
        if let Err(diags) = self.apply_external(&ours, &merged_doc, &path, "external edit") {
            self.files.diverged.insert(id.clone(), Diverged::Invalid);
            for d in diags {
                self.log(d);
            }
            return;
        }
        self.files
            .synced
            .insert(id.clone(), Synced::new(path.clone(), text, theirs));
        self.files.diverged.remove(id);
        self.log(
            Diagnostic::info(
                codes::EXTERNAL_EDIT,
                format!("`{path}` changed on disk; merged into the loaded document"),
            )
            .file(path),
        );
    }

    /// Commits the edits from `from` to `to` as one human transaction.
    fn apply_external(
        &mut self,
        from: &Document,
        to: &Document,
        path: &str,
        label: &str,
    ) -> Result<(), Vec<Diagnostic>> {
        let edits = diff(from, to);
        if edits.is_empty() {
            return Ok(());
        }
        let meta = TxnMeta {
            author: Author::Human,
            why: Some(format!("`{path}` changed on disk")),
            label: Some(label.to_owned()),
            base: None,
            idempotency_key: None,
        };
        let calling = self.calling.take();
        let result = self.store.transact(meta, edits).map(drop);
        self.drain();
        self.calling = calling;
        result
    }

    /// `doc.sync`: resolves a diverged document by keeping memory or disk.
    pub(crate) fn sync(&mut self, id: &AssetId, keep: Keep) -> Result<Value, Vec<Diagnostic>> {
        let Some(synced) = self.files.synced.get(id).cloned() else {
            return Err(vec![Diagnostic::error(
                codes::DOC_NOT_FOUND,
                format!("`{id}` is not loaded from a file"),
            )]);
        };
        let abs = self.root.join(&synced.path);
        match keep {
            Keep::Memory => {
                self.files.diverged.remove(id);
                self.write_doc(id, true);
                self.wait_writes();
            }
            Keep::Disk => {
                let Ok(text) = fs::read_to_string(&abs) else {
                    self.unload(id);
                    return Ok(serde_json::json!({ "doc": id, "unloaded": true }));
                };
                let result = read(
                    &text,
                    &synced.path,
                    self.store.schema(),
                    Some(self.store.workspace().index()),
                );
                let theirs = match result.doc {
                    Some(d) if !ara_diag::has_errors(&result.diags) => d,
                    _ => {
                        return Err(result
                            .diags
                            .into_iter()
                            .filter(Diagnostic::is_error)
                            .collect());
                    }
                };
                let ours = self
                    .store
                    .workspace()
                    .doc(id.as_str())
                    .cloned()
                    .ok_or_else(|| {
                        vec![Diagnostic::error(
                            codes::DOC_NOT_FOUND,
                            format!("`{id}` is not loaded"),
                        )]
                    })?;
                self.apply_external(&ours, &theirs, &synced.path, "doc.sync: keep disk")?;
                self.files
                    .synced
                    .insert(id.clone(), Synced::new(synced.path.clone(), text, theirs));
                self.files.diverged.remove(id);
                self.files.changed.remove(id);
            }
        }
        Ok(
            serde_json::json!({ "doc": id, "path": synced.path, "kept": keep, "revision": self.store.revision() }),
        )
    }

    /// Documents that differ from their files, and diverged ones, for `session.status`.
    pub(crate) fn file_status(&self) -> Value {
        let dirty: Vec<&str> = self
            .files
            .synced
            .iter()
            .filter(|(id, s)| {
                self.store
                    .workspace()
                    .doc(id.as_str())
                    .is_some_and(|d| *d != s.doc)
            })
            .map(|(_, s)| s.path.as_str())
            .collect();
        let diverged: BTreeMap<&str, &str> = self
            .files
            .diverged
            .iter()
            .filter_map(|(id, why)| {
                let path = self.files.synced.get(id)?.path.as_str();
                Some((
                    path,
                    match why {
                        Diverged::Invalid => "invalid",
                        Diverged::Conflict => "conflict",
                        Diverged::Deleted => "deleted",
                    },
                ))
            })
            .collect();
        serde_json::json!({ "unsaved": dirty, "diverged": diverged })
    }
}
