//! A set of loaded documents plus the project's asset index.

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, Location};
use ara_schema::SchemaDb;
use ara_schema::grammar::parse_ref;

use crate::document::Document;
use crate::prefab::{DocLookup, Resolved, resolve};
use crate::refs::AssetIndex;
use crate::validate::{check_entity_refs, check_record, check_structure};
use crate::{AssetId, RecordId};

/// Loaded documents keyed by asset ID, each with its project-relative path.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    docs: BTreeMap<AssetId, (String, Document)>,
    index: AssetIndex,
}

impl Workspace {
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
        self.index
            .insert(doc.id.to_string(), path.clone(), Some(doc.kind.clone()));
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
}

impl DocLookup for Workspace {
    fn lookup(&self, id: &str) -> Option<&Document> {
        self.doc(id)
    }
}
