//! Late join: the documents at a revision as canonical text. A joiner builds a
//! [`Replica`](crate::Replica) from it and replays the op events after it.

use ara_diag::Diagnostic;
use ara_doc::{AssetIndex, Workspace, read, write};
use ara_ops::Store;
use ara_schema::SchemaDb;
use serde::{Deserialize, Serialize};

use crate::branch::SavedDoc;

/// Every loaded document at one revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// The authority revision the documents are at.
    pub revision: u64,
    /// Storage prefix omitted from authored references; absent for generic workspaces.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reference_root: String,
    /// Each document's canonical text, by path.
    pub docs: Vec<SavedDoc>,
}

impl Snapshot {
    /// The store's documents now.
    pub fn of(store: &Store) -> Snapshot {
        let docs = store
            .workspace()
            .docs()
            .map(|(path, doc)| SavedDoc {
                path: path.to_owned(),
                text: write(doc, store.schema()),
            })
            .collect();
        Snapshot {
            revision: store.revision(),
            reference_root: store.workspace().reference_root().to_owned(),
            docs,
        }
    }

    /// The documents, read back.
    pub fn workspace(&self, db: &SchemaDb) -> Result<Workspace, Vec<Diagnostic>> {
        let mut ws = Workspace::new(AssetIndex::new());
        ws.set_reference_root(&self.reference_root);
        for d in &self.docs {
            let r = read(&d.text, &d.path, db, None);
            let doc = r.doc.ok_or(r.diags)?;
            let asset_path = if doc.kind == "import" {
                d.path.strip_suffix(".import.json").unwrap_or(&d.path)
            } else {
                &d.path
            };
            ws.insert_with_asset_path(d.path.clone(), doc, asset_path);
        }
        Ok(ws)
    }
}
