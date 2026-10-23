//! Exact source mirrors for pending op preconditions, plus canonical scene identity.
use crate::{Result, StateHash};
use ara_doc::{AssetId, Document, Record, RecordId, Workspace};
use ara_schema::SchemaDb;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SourceDocument {
    kind: String,
    id: AssetId,
    records: BTreeMap<RecordId, Record>,
    conflicts: Vec<Value>,
}

impl SourceDocument {
    /// Retains stored key presence exactly, including explicit defaults used by ops.
    pub fn capture(document: &Document) -> Self {
        Self {
            kind: document.kind.clone(),
            id: document.id.clone(),
            records: document.records().clone(),
            conflicts: document.conflicts.clone(),
        }
    }

    /// Reconstructs a checkpoint's document mirror as a reader, without authoring edits.
    pub fn restore(&self) -> Document {
        let mut document = Document::new(self.kind.clone(), self.id.clone());
        document.conflicts = self.conflicts.clone();
        for (id, record) in &self.records {
            document.insert_unchecked(id.clone(), record.clone());
        }
        document
    }
}

pub(crate) fn scene_hash(workspace: &Workspace, schema: &SchemaDb) -> Result<StateHash> {
    let canonical: BTreeMap<_, _> = workspace
        .docs()
        .map(|(path, document)| (path, ara_doc::write(document, schema)))
        .collect();
    Ok(StateHash::of_value(&serde_json::to_value(canonical)?))
}
