//! Incremental copies of loaded authoring records; never clone a whole workspace on input.
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_ops::Store;
use ara_schema::SchemaDb;
use std::{
    ops::Bound,
    time::{Duration, Instant},
};

const RECORDS_PER_TICK: usize = 64;
const COPY_BUDGET: Duration = Duration::from_millis(2);

struct Header {
    path: String,
    asset_path: String,
    id: AssetId,
}
pub(super) struct Snapshot {
    pub workspace: Workspace,
    pub schema: SchemaDb,
    pub records: usize,
}
pub(super) struct Preparing {
    snapshot: Snapshot,
    headers: Vec<Header>,
    document: usize,
    current: Option<Document>,
    after: Option<RecordId>,
    total: usize,
}
impl Preparing {
    pub fn new(store: &Store) -> Self {
        let source = store.workspace();
        let headers = source
            .docs()
            .map(|(path, doc)| Header {
                path: path.to_owned(),
                asset_path: source
                    .index()
                    .path_of(doc.id.as_str())
                    .unwrap_or(path)
                    .to_owned(),
                id: doc.id.clone(),
            })
            .collect();
        Self {
            // These are metadata maps, not document/record JSON. Record payloads
            // are copied in bounded batches only on subsequent service ticks.
            snapshot: Snapshot {
                workspace: Workspace::new(source.index().clone()),
                schema: store.schema().clone(),
                records: 0,
            },
            headers,
            document: 0,
            current: None,
            after: None,
            total: source.docs().map(|(_, doc)| doc.records().len()).sum(),
        }
    }
    /// The caller rejects revision/generation changes before every batch, so a
    /// completed detached snapshot contains exactly one authority revision.
    pub fn advance(&mut self, store: &Store) -> Result<bool, String> {
        let started = Instant::now();
        let mut copied = 0;
        while copied < RECORDS_PER_TICK && started.elapsed() < COPY_BUDGET {
            let Some(header) = self.headers.get(self.document) else {
                return Ok(true);
            };
            let source = store
                .workspace()
                .doc(header.id.as_str())
                .ok_or("A validation source document was unloaded")?;
            if self.current.is_none() {
                let mut doc = Document::new(&source.kind, source.id.clone());
                doc.conflicts = source.conflicts.clone();
                self.current = Some(doc);
            }
            let next = match self.after.as_ref() {
                Some(id) => source
                    .records()
                    .range::<RecordId, _>((Bound::Excluded(id), Bound::Unbounded))
                    .next(),
                None => source.records().iter().next(),
            };
            if let Some((id, record)) = next {
                // This is an isolated reader snapshot, never an authoring store.
                self.current
                    .as_mut()
                    .ok_or("Validation snapshot document is missing")?
                    .insert_unchecked(id.clone(), record.clone());
                self.after = Some(id.clone());
                self.snapshot.records += 1;
                copied += 1;
            } else {
                let doc = self
                    .current
                    .take()
                    .ok_or("Validation snapshot document is missing")?;
                self.snapshot.workspace.insert_with_asset_path(
                    &header.path,
                    doc,
                    &header.asset_path,
                );
                self.document += 1;
                self.after = None;
            }
        }
        Ok(self.document >= self.headers.len())
    }
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.
        } else {
            self.snapshot.records as f32 / self.total as f32
        }
    }
    pub fn finish(self) -> Snapshot {
        self.snapshot
    }
}
