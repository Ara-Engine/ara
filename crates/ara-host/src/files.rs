//! "Always saved" persistence and external edits (R03 §6.9–6.10).
//!
//! - **Flush:** a document edited by a transaction is written canonically once it
//!   has been quiet for the debounce time (temp file, then rename), and a `flush`
//!   entry with its revision and hash goes to the journal.
//! - **Watch:** loaded files are polled (modified time and size). A changed file is
//!   read and validated, then three-way merged (base: the last synced text, ours:
//!   memory, theirs: disk) and applied as a human transaction (`external edit`).
//!   Invalid files and conflicts with unsaved changes are reported and the document
//!   is marked diverged: nothing is overwritten until `doc.sync` or a clean edit.
//! - New document files are loaded; deleted ones are unloaded (or, with unsaved
//!   changes, kept and reported).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime};

use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Document, Edit, OrderKey, field, read, write};
use ara_ops::{Author, TxnMeta};
use ara_schema::FieldPath;
use serde_json::Value;

use crate::host::State;
use crate::journal::{self, Entry};
use crate::params::Keep;

/// What a file looked like when last seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let m = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// The last state a document and its file agreed on.
#[derive(Debug, Clone)]
pub(crate) struct Synced {
    /// Project-relative path.
    pub path: String,
    /// The file's text.
    pub text: String,
    /// The document that text reads as: the merge base for external edits.
    pub doc: Document,
    stamp: Option<Stamp>,
}

impl Synced {
    pub(crate) fn new(path: String, text: String, doc: Document, abs: &Path) -> Self {
        Synced {
            path,
            text,
            doc,
            stamp: stamp(abs),
        }
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
    pub synced: BTreeMap<AssetId, Synced>,
    /// Documents changed by a transaction, with the time of the latest change.
    pub changed: BTreeMap<AssetId, Instant>,
    pub diverged: BTreeMap<AssetId, Diverged>,
    last_watch: Instant,
    last_scan: Instant,
}

impl Default for Files {
    fn default() -> Self {
        Files {
            synced: BTreeMap::new(),
            changed: BTreeMap::new(),
            diverged: BTreeMap::new(),
            last_watch: Instant::now(),
            last_scan: Instant::now(),
        }
    }
}

/// Writes `text` to `path` through a temp file and a rename.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("ara-tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
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
    /// Writes documents whose last change is older than the debounce time (all of
    /// them with `force`). Diverged documents are skipped.
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
        self.maybe_compact();
    }

    /// Writes one document if it differs from its file (always with `always`), and
    /// journals that the file now holds the current revision.
    pub(crate) fn write_doc(&mut self, id: &AssetId, always: bool) {
        let Some(doc) = self.store.workspace().doc(id.as_str()) else {
            return;
        };
        let path = self
            .store
            .workspace()
            .path_of(id.as_str())
            .unwrap_or_default()
            .to_owned();
        let abs = self.root.join(&path);
        let unchanged = self.files.synced.get(id).is_some_and(|s| s.doc == *doc);
        let text = if unchanged && !always {
            self.files
                .synced
                .get(id)
                .map(|s| s.text.clone())
                .unwrap_or_default()
        } else {
            let text = write(doc, self.store.schema());
            if let Err(e) = write_atomic(&abs, &text) {
                self.files.changed.insert(id.clone(), Instant::now());
                self.log(
                    Diagnostic::error(codes::FLUSH_FAILED, format!("cannot write `{path}`: {e}"))
                        .file(path),
                );
                return;
            }
            let doc = doc.clone();
            self.files
                .synced
                .insert(id.clone(), Synced::new(path, text.clone(), doc, &abs));
            text
        };
        let rev = self.store.revision();
        self.journal(&Entry::Flush {
            doc: id.to_string(),
            rev,
            hash: journal::hash(&text),
        });
    }

    /// Polls loaded files for external edits, and the project for new documents.
    pub(crate) fn watch(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.files.last_watch) < self.watch_every {
            return;
        }
        self.files.last_watch = now;
        let ids: Vec<AssetId> = self.files.synced.keys().cloned().collect();
        for id in ids {
            let Some(synced) = self.files.synced.get(&id) else {
                continue;
            };
            let abs = self.root.join(&synced.path);
            match stamp(&abs) {
                None => self.deleted(&id),
                Some(s) if Some(&s) != synced.stamp.as_ref() => {
                    let Ok(text) = fs::read_to_string(&abs) else {
                        continue;
                    };
                    if text == synced.text {
                        if let Some(syn) = self.files.synced.get_mut(&id) {
                            syn.stamp = Some(s);
                        }
                    } else {
                        self.reconcile(&id, text, Some(s));
                    }
                }
                Some(_) => {}
            }
        }
        if now.duration_since(self.files.last_scan) >= self.scan_every {
            self.files.last_scan = now;
            self.scan();
        }
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
        self.files.synced.remove(id);
        self.files.changed.remove(id);
        self.files.diverged.remove(id);
    }

    /// Loads document files that appeared since the last scan, and reports changed
    /// type files.
    fn scan(&mut self) {
        let known: Vec<String> = self.files.synced.values().map(|s| s.path.clone()).collect();
        let mut found = Vec::new();
        crate::project::walk_documents(&self.root, self.store.schema(), &mut found);
        for abs in found {
            let rel = crate::project::rel_to(&self.root, &abs);
            if known.contains(&rel) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&abs) else {
                continue;
            };
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
                continue;
            };
            if self.store.workspace().doc(doc.id.as_str()).is_some() {
                self.log(Diagnostic::warning(
                    codes::EXTERNAL_INVALID,
                    format!("new document `{rel}` reuses the asset ID `{}` of a loaded document; not loaded", doc.id),
                ).file(rel));
                continue;
            }
            self.files.synced.insert(
                doc.id.clone(),
                Synced::new(rel.clone(), text, doc.clone(), &abs),
            );
            self.store.load_doc(rel.clone(), doc);
            self.log(
                Diagnostic::info(codes::EXTERNAL_EDIT, format!("loaded new document `{rel}`"))
                    .file(rel),
            );
        }
        let mut changed = Vec::new();
        for (path, seen) in &mut self.type_files {
            let now = fs::metadata(&*path).and_then(|m| m.modified()).ok();
            if now != *seen {
                *seen = now;
                changed.push(crate::project::rel_to(&self.root, path));
            }
        }
        for rel in changed {
            self.log(
                Diagnostic::warning(
                    codes::TYPES_CHANGED,
                    format!("`{rel}` changed; restart the daemon to load the new types"),
                )
                .file(rel),
            );
        }
    }

    /// Brings an external edit of a loaded document into memory (see module docs).
    fn reconcile(&mut self, id: &AssetId, text: String, seen: Option<Stamp>) {
        let Some(synced) = self.files.synced.get_mut(id) else {
            return;
        };
        synced.stamp = seen;
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
        let abs = self.root.join(&path);
        let mut fresh = Synced::new(path.clone(), text, theirs, &abs);
        fresh.stamp = self.files.synced.get(id).and_then(|s| s.stamp.clone());
        self.files.synced.insert(id.clone(), fresh);
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
                self.files.synced.insert(
                    id.clone(),
                    Synced::new(synced.path.clone(), text, theirs, &abs),
                );
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
