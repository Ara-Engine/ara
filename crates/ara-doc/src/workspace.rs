//! A set of loaded documents plus the project's asset index.

use std::collections::{BTreeMap, BTreeSet};

use ara_diag::{Diagnostic, Location, codes};
use ara_schema::SchemaDb;
use ara_schema::grammar::parse_ref;

use crate::document::Document;
use crate::prefab::{DocLookup, Resolved, check_instance, resolve};
use crate::refs::AssetIndex;
use crate::validate::{check_entity_refs, check_record, check_structure};
use crate::{AssetId, Edit, Op, RecordId};

/// Loaded documents keyed by asset ID, each with its project-relative path.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    docs: BTreeMap<AssetId, (String, Document)>,
    index: AssetIndex,
}

impl Workspace {
    /// Reverse a staged op during rollback or optimistic rebase.
    pub fn revert(&mut self, op: &Op, db: &SchemaDb) {
        if matches!(op, Op::Document { .. }) {
            self.apply(op.inverse().to_edit(), db)
                .expect("staged lifecycle inverse applies");
        } else if let Some(doc) = self.doc_mut(op.doc().as_str()) {
            doc.revert(op, db);
        }
    }
    /// Apply an edit, including an empty document's lifecycle, inside a transaction.
    pub fn apply(&mut self, edit: Edit, db: &SchemaDb) -> Result<Op, Diagnostic> {
        if let Edit::Document { doc, path, kind } = edit {
            let relative = std::path::Path::new(&path);
            if path.is_empty()
                || path.contains([':', '\\'])
                || path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|p| !matches!(p, std::path::Component::Normal(_)))
            {
                return Err(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "document path must stay inside the project",
                ));
            }
            if let Some(kind) = &kind {
                let desc = db.kind(kind).ok_or_else(|| {
                    Diagnostic::error(
                        codes::UNKNOWN_SCHEMA,
                        format!("unknown document kind `{kind}`"),
                    )
                })?;
                if doc.prefix() != desc.id_prefix
                    || !path.ends_with(&format!(".{}", desc.extension))
                {
                    return Err(Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "document ID prefix and file extension must match its kind",
                    ));
                }
                if self.doc(doc.as_str()).is_some()
                    || self.index.path_of(doc.as_str()).is_some()
                    || self.docs().any(|(p, _)| {
                        if cfg!(windows) {
                            p.eq_ignore_ascii_case(&path)
                        } else {
                            p == path
                        }
                    })
                    || self.index.id_of(&path).is_some()
                {
                    return Err(Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "document ID or path already exists",
                    ));
                }
                let source = if kind == "import" {
                    path.trim_end_matches(".import.json").to_owned()
                } else {
                    path.clone()
                };
                self.insert_with_asset_path(path.clone(), Document::new(kind, doc.clone()), source);
                Ok(Op::Document {
                    doc,
                    path,
                    kind: Some(kind.clone()),
                    prev: None,
                })
            } else {
                let existing = self.doc(doc.as_str()).ok_or_else(|| {
                    Diagnostic::error(codes::DOC_NOT_FOUND, "document is not loaded")
                })?;
                if self.path_of(doc.as_str()) != Some(path.as_str())
                    || !existing.records().is_empty()
                {
                    return Err(Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "only the addressed empty document can be removed; undo its records first",
                    ));
                }
                let prev = Some(existing.kind.clone());
                self.remove(doc.as_str());
                Ok(Op::Document {
                    doc,
                    path,
                    kind: None,
                    prev,
                })
            }
        } else {
            let doc = edit.doc().clone();
            self.doc_mut(doc.as_str())
                .ok_or_else(|| {
                    Diagnostic::error(
                        codes::DOC_NOT_FOUND,
                        format!("no document `{doc}` is loaded"),
                    )
                })?
                .apply(edit, db)
        }
    }

    /// A workspace over a project index (which may list assets that aren't loaded).
    pub fn new(index: AssetIndex) -> Self {
        Workspace {
            docs: BTreeMap::new(),
            index,
        }
    }

    /// Adds or replaces a document at `path` and indexes it.
    pub fn insert(&mut self, path: impl Into<String>, doc: Document) {
        let path = path.into();
        self.insert_with_asset_path(path.clone(), doc, path);
    }

    /// Loads a document at `path` while references to its ID resolve to `asset_path`.
    /// Import settings can therefore be saved as a sidecar of their source media.
    pub fn insert_with_asset_path(
        &mut self,
        path: impl Into<String>,
        doc: Document,
        asset_path: impl Into<String>,
    ) {
        let path = path.into();
        let kind = (doc.kind != "import").then(|| doc.kind.clone());
        self.index.insert(doc.id.to_string(), asset_path, kind);
        self.docs.insert(doc.id.clone(), (path, doc));
    }

    /// Unloads a document and drops it from the index (its file was deleted). Returns
    /// its path and contents.
    pub fn remove(&mut self, id: &str) -> Option<(String, Document)> {
        self.index.remove(id);
        self.docs.remove(id)
    }

    /// A document by asset ID.
    pub fn doc(&self, id: &str) -> Option<&Document> {
        self.docs.get(id).map(|(_, d)| d)
    }

    /// A document for mutation. Only ara-ops transactions should use this.
    pub fn doc_mut(&mut self, id: &str) -> Option<&mut Document> {
        self.docs.get_mut(id).map(|(_, d)| d)
    }

    /// The project-relative path of a loaded document.
    pub fn path_of(&self, id: &str) -> Option<&str> {
        self.docs.get(id).map(|(p, _)| p.as_str())
    }

    /// Finds a loaded document by asset ID, path or `@path#id` reference.
    pub fn find(&self, key: &str) -> Option<&AssetId> {
        let id = if key.starts_with('@') {
            let parts = parse_ref(key).ok()?;
            parts
                .id
                .or_else(|| parts.path.and_then(|p| self.index.id_of(p)))?
        } else if self.docs.contains_key(key) {
            key
        } else {
            self.index.id_of(key)?
        };
        self.docs.get_key_value(id).map(|(k, _)| k)
    }

    /// Loaded documents with their paths, sorted by asset ID.
    pub fn docs(&self) -> impl Iterator<Item = (&str, &Document)> {
        self.docs.values().map(|(p, d)| (p.as_str(), d))
    }

    /// The asset index.
    pub fn index(&self) -> &AssetIndex {
        &self.index
    }

    /// Location of a document, for diagnostics.
    pub fn location(&self, id: &AssetId) -> Location {
        Location {
            file: self.path_of(id.as_str()).map(str::to_owned),
            doc: Some(id.to_string()),
            ..Location::default()
        }
    }

    /// Resolves a document's prefab instances (see [`crate::prefab`]).
    pub fn resolve(&self, id: &AssetId, db: &SchemaDb, out: &mut Vec<Diagnostic>) -> Resolved {
        match self.doc(id.as_str()) {
            Some(doc) => resolve(doc, db, self, &self.location(id), out),
            None => Resolved::new(),
        }
    }

    /// Validates `records` of a document (all records if `None`), its structure and its
    /// prefab instances. Transactions call this on what they touched.
    pub fn check(
        &self,
        id: &AssetId,
        records: Option<&[RecordId]>,
        db: &SchemaDb,
    ) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(doc) = self.doc(id.as_str()) else {
            return out;
        };
        let Some(kind) = db.kind(&doc.kind) else {
            return out;
        };
        let base = self.location(id);
        let all: Vec<RecordId>;
        let ids = match records {
            Some(ids) => ids,
            None => {
                all = doc.records().keys().cloned().collect();
                &all
            }
        };
        for rid in ids {
            if let Some(rec) = doc.record(rid.as_str()) {
                check_record(db, kind, rid, rec, &base, &mut out);
            }
        }
        check_structure(doc, kind, &base, &mut out);
        check_entity_refs(db, doc, &base, &mut out);
        resolve(doc, db, self, &base, &mut out);
        out
    }

    /// What `records` of a document report on their own, in document order: each
    /// record against the schema, then each prefab instance's prefab and overrides.
    /// A [local edit](Edit::is_local) changes only these, so a transaction of local
    /// edits to one document validates just its records; [`check`](Self::check)
    /// reports the same problems for them, plus structure, references and resolution.
    pub fn check_local(
        &self,
        id: &AssetId,
        records: &BTreeSet<RecordId>,
        db: &SchemaDb,
    ) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(doc) = self.doc(id.as_str()) else {
            return out;
        };
        let Some(kind) = db.kind(&doc.kind) else {
            return out;
        };
        let base = self.location(id);
        // Records are keyed in ID order, so walking the sorted set keeps document order.
        for rid in records {
            if let Some(rec) = doc.record(rid.as_str()) {
                check_record(db, kind, rid, rec, &base, &mut out);
            }
        }
        for rid in records {
            check_instance(doc, rid, db, self, &base, &mut out);
        }
        out
    }
}

impl DocLookup for Workspace {
    fn lookup(&self, id: &str) -> Option<&Document> {
        self.doc(id)
    }
}
