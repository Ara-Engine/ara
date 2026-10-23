use crate::{CellBlob, Manifest, Result, cook_cell, invalid};
use ara_asset::{Artifact, ArtifactKind, CacheBackend, Digest, ImportOutput};
use ara_doc::Workspace;
use ara_schema::SchemaDb;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Worker-only source of immutable cooked cell data. Never called on the frame thread.
pub trait CellSource: Send + Sync + 'static {
    /// Reads a cooked blob, or text through the binary cache. Bound backing input
    /// to `max_bytes`; custom sources must also bound their decoding temporaries.
    fn load(&self, cell: &str, max_bytes: usize) -> Result<CellBlob>;
}

/// Shipping source: one bounded binary cell blob per pack-aligned logical path.
pub struct CookedSource {
    root: PathBuf,
}
impl CookedSource {
    /// Opens an immutable cooked cell directory, containing `c_<id>.aracell` files.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            root: root.into().canonicalize()?,
        })
    }
}
impl CellSource for CookedSource {
    fn load(&self, cell: &str, max_bytes: usize) -> Result<CellBlob> {
        if ara_doc::RecordId::parse(cell).map_err(invalid)?.prefix() != "c" {
            return Err(invalid("invalid cell ID"));
        }
        CellBlob::decode(&read_bounded(
            &self.root,
            &format!("{cell}.aracell"),
            max_bytes,
        )?)
    }
}

/// Shipping pack reader. Immutable backing bytes are shared without per-frame copies.
pub struct PackSource {
    bytes: Arc<[u8]>,
    key: Option<ara_pack::EncryptionKey>,
}
impl PackSource {
    /// Validates an immutable pack at setup; workers read/decompress individual cells.
    pub fn new(bytes: Arc<[u8]>, key: Option<ara_pack::EncryptionKey>) -> Result<Self> {
        ara_pack::Pack::open(&bytes).map_err(|e| invalid(e.to_string()))?;
        Ok(Self { bytes, key })
    }
}
impl CellSource for PackSource {
    fn load(&self, cell: &str, max_bytes: usize) -> Result<CellBlob> {
        if ara_doc::RecordId::parse(cell).map_err(invalid)?.prefix() != "c" {
            return Err(invalid("invalid cell ID"));
        }
        let pack = ara_pack::Pack::open(&self.bytes).map_err(|e| invalid(e.to_string()))?;
        let bytes = pack
            .read(
                &format!("world/cells/{cell}.aracell"),
                max_bytes.min(crate::cook::MAX_CELL_BYTES) as u64,
                self.key.as_ref(),
            )
            .map_err(|e| invalid(e.to_string()))?;
        CellBlob::decode(&bytes)
    }
}

/// Editor source with immutable manifest/schema and the shared asset cache backend.
/// Construct a new source/pool after an authoring revision; old worlds stay active
/// until the host swaps generations at its D20 application boundary.
pub struct EditorSource {
    root: PathBuf,
    manifest: Manifest,
    schema: SchemaDb,
    cache: Arc<dyn CacheBackend>,
    dependencies: Workspace,
    dependency_hash: [u8; 32],
}
impl EditorSource {
    /// Binds a canonical manifest snapshot to project files and binary derived data.
    pub fn new(
        root: impl Into<PathBuf>,
        manifest: Manifest,
        schema: SchemaDb,
        cache: Arc<dyn CacheBackend>,
    ) -> Result<Self> {
        Ok(Self {
            root: root.into().canonicalize()?,
            manifest,
            schema,
            cache,
            dependencies: Workspace::default(),
            dependency_hash: [0; 32],
        })
    }

    /// Adds an immutable snapshot of referenced prefab documents. Include nested
    /// dependencies too; changing the snapshot invalidates binary cache identities.
    pub fn with_dependencies(mut self, workspace: Workspace) -> Result<Self> {
        let mut hash = blake3::Hasher::new_derive_key("Ara world prefab dependencies v1");
        for (path, document) in workspace.docs() {
            if document.kind != "prefab" {
                return Err(invalid("world dependencies must be prefab documents"));
            }
            let text = ara_doc::write(document, &self.schema);
            hash.update(&(path.len() as u64).to_le_bytes());
            hash.update(path.as_bytes());
            hash.update(&(text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
        self.dependency_hash = *hash.finalize().as_bytes();
        self.dependencies = workspace;
        Ok(self)
    }
}
impl CellSource for EditorSource {
    fn load(&self, cell: &str, max_bytes: usize) -> Result<CellBlob> {
        let spec = self
            .manifest
            .cells
            .get(cell)
            .ok_or_else(|| invalid("unknown editor cell"))?;
        let mut inputs = Vec::new();
        let mut action = blake3::Hasher::new_derive_key("Ara world cell text cache v1");
        action.update(&crate::cook::schema_hash(&self.schema));
        action.update(&self.dependency_hash);
        action.update(cell.as_bytes());
        action.update(&spec.x.to_le_bytes());
        action.update(&spec.z.to_le_bytes());
        action.update(&spec.slot.to_le_bytes());
        let mut total = 0usize;
        for reference in spec.documents.iter().chain(&spec.actors) {
            let (path, expected) = crate::model::document_ref(reference)?;
            let bytes = read_bounded(&self.root, path, max_bytes)?;
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| invalid("cell input overflow"))?;
            if total > max_bytes.min(crate::cook::MAX_CELL_BYTES) {
                return Err(invalid("cell text exceeds 64 MiB"));
            }
            action.update(&(path.len() as u64).to_le_bytes());
            action.update(path.as_bytes());
            action.update(&(bytes.len() as u64).to_le_bytes());
            action.update(&bytes);
            // Parse only on a binary-cache miss below.
            inputs.push((path, expected, bytes));
        }
        let key = Digest(*action.finalize().as_bytes());
        if let Some(hit) = self.cache.load(key)? {
            let artifact = hit
                .output
                .artifacts
                .first()
                .ok_or_else(|| invalid("empty world cache result"))?;
            if artifact.bytes.len() > max_bytes {
                return Err(invalid("cached cell exceeds worker input limit"));
            }
            return CellBlob::decode(&artifact.bytes);
        }
        let mut workspace = self.dependencies.clone();
        for (path, expected, bytes) in inputs {
            workspace.index_bytes(path, expected, bytes, &self.schema)?;
        }
        let blob = cook_cell(&self.manifest, cell, &workspace, &self.schema, None)?;
        let output = ImportOutput {
            artifacts: vec![Artifact {
                name: "cell".into(),
                kind: ArtifactKind::Custom,
                bytes: blob.encode()?,
            }],
        };
        self.cache.store(key, &output)?;
        Ok(blob)
    }
}

// Keep file input separate from document decoding so warm cache hits do no parsing.
trait IndexBytes {
    fn index_bytes(
        &mut self,
        path: &str,
        expected: &str,
        bytes: Vec<u8>,
        schema: &SchemaDb,
    ) -> Result<()>;
}
impl IndexBytes for Workspace {
    fn index_bytes(
        &mut self,
        path: &str,
        expected: &str,
        bytes: Vec<u8>,
        schema: &SchemaDb,
    ) -> Result<()> {
        let text = std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?;
        let read = ara_doc::read(text, path, schema, None);
        if read.diags.iter().any(|d| d.is_error()) {
            return Err(crate::Error::Transaction(read.diags));
        }
        let doc = read
            .doc
            .ok_or_else(|| invalid("cell document failed to read"))?;
        if doc.id.as_str() != expected {
            return Err(invalid("cell document identity mismatch"));
        }
        self.insert(path, doc);
        Ok(())
    }
}

fn read_bounded(root: &Path, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
    ara_pack::validate_path(path).map_err(|e| invalid(e.to_string()))?;
    let full = root.join(path).canonicalize()?;
    if !full.starts_with(root) {
        return Err(invalid("cell path escapes project root"));
    }
    let mut bytes = Vec::new();
    let limit = max_bytes.min(crate::cook::MAX_CELL_BYTES);
    fs::File::open(full)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("cell file exceeds size limit"));
    }
    Ok(bytes)
}
