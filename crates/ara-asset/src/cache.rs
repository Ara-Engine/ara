use crate::{Artifact, ArtifactKind, Digest, Error, ImportOutput, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Immutable result plus its action identity.
#[derive(Debug, Clone)]
pub struct CachedImport {
    /// Hash of every cook input.
    pub key: Digest,
    /// Validated cooked artifacts.
    pub output: ImportOutput,
}
/// Shared-cache seam: a server can implement identical immutable action semantics.
pub trait CacheBackend: Send + Sync {
    /// Returns a checked action result, or a cache miss.
    fn load(&self, key: Digest) -> Result<Option<CachedImport>>;
    /// Installs a complete result atomically; conflicting results are errors.
    fn store(&self, key: Digest, output: &ImportOutput) -> Result<()>;
}
/// Disk content-addressed storage: objects by byte digest, actions by input digest.
#[derive(Debug, Clone)]
pub struct FileCache {
    root: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    version: u32,
    artifacts: Vec<Blob>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    name: String,
    kind: ArtifactKind,
    hash: Digest,
    length: u64,
}
/// Mark-and-sweep accounting, excluding temporary writes in progress.
#[derive(Debug, Default, Clone, Copy)]
pub struct GcStats {
    /// Unrooted action manifests removed.
    pub actions_removed: u64,
    /// Unreachable immutable objects removed.
    pub objects_removed: u64,
    /// Total reclaimed bytes.
    pub bytes_removed: u64,
}
impl FileCache {
    /// Opens `<project>/.ara/cache`; never writes source assets or sidecars.
    pub fn new(project: impl AsRef<Path>) -> Result<Self> {
        Self::at(project.as_ref().join(".ara/cache"))
    }
    /// Opens an explicit cache root, useful for a shared/local cache adapter.
    pub fn at(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("actions"))?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(".gitignore"))
        {
            Ok(mut ignore) => ignore.write_all(b"*\n")?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Self { root })
    }
    /// The cache directory, for export and diagnostics.
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn object(&self, key: Digest) -> PathBuf {
        self.root.join("objects").join(key.to_string())
    }
    fn action(&self, key: Digest) -> PathBuf {
        self.root.join("actions").join(key.to_string())
    }
    fn entry(&self, key: Digest) -> Result<Option<Entry>> {
        let bytes = match fs::read(self.action(key)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let e: Entry = serde_json::from_slice(&bytes)?;
        if e.version != 1 {
            return Err(Error::Invalid("unsupported cache manifest".into()));
        }
        Ok(Some(e))
    }
    /// Collects unreachable actions/objects. Caller must quiesce writers and include
    /// active, staged and undo generations in `roots` (including other processes).
    pub fn collect(&self, roots: &BTreeSet<Digest>) -> Result<GcStats> {
        let mut live = BTreeSet::new();
        for root in roots {
            if let Some(e) = self.entry(*root)? {
                for a in e.artifacts {
                    live.insert(a.hash.to_string());
                }
            }
        }
        let root_names: BTreeSet<_> = roots.iter().map(ToString::to_string).collect();
        let mut stats = GcStats::default();
        for (folder, keep, objects) in [("actions", &root_names, false), ("objects", &live, true)] {
            for entry in fs::read_dir(self.root.join(folder))? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.len() != 64
                    || !name.bytes().all(|c| c.is_ascii_hexdigit())
                    || keep.contains(&name)
                    || !entry.file_type()?.is_file()
                {
                    continue;
                }
                let len = entry.metadata()?.len();
                fs::remove_file(entry.path())?;
                stats.bytes_removed += len;
                if objects {
                    stats.objects_removed += 1;
                } else {
                    stats.actions_removed += 1;
                }
            }
        }
        Ok(stats)
    }
}
fn install(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("cache path without parent".into()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read(path)? == bytes {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "immutable cache entry conflicts (nondeterministic cook or corruption)".into(),
                ))
            }
        }
        Err(e) => Err(e.error.into()),
    }
}
impl CacheBackend for FileCache {
    fn load(&self, key: Digest) -> Result<Option<CachedImport>> {
        let Some(entry) = self.entry(key)? else {
            return Ok(None);
        };
        let mut artifacts = Vec::with_capacity(entry.artifacts.len());
        for a in entry.artifacts {
            let bytes = match fs::read(self.object(a.hash)) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if bytes.len() as u64 != a.length || Digest::of(&bytes) != a.hash {
                return Err(Error::Invalid("cache object hash mismatch".into()));
            }
            artifacts.push(Artifact {
                name: a.name,
                kind: a.kind,
                bytes,
            });
        }
        let output = ImportOutput { artifacts };
        output.validate()?;
        Ok(Some(CachedImport { key, output }))
    }
    fn store(&self, key: Digest, output: &ImportOutput) -> Result<()> {
        output.validate()?;
        let mut artifacts = Vec::new();
        for a in &output.artifacts {
            let hash = Digest::of(&a.bytes);
            install(&self.object(hash), &a.bytes)?;
            artifacts.push(Blob {
                name: a.name.clone(),
                kind: a.kind,
                hash,
                length: a.bytes.len() as u64,
            });
        }
        artifacts.sort_by(|a, b| a.name.cmp(&b.name));
        let entry = Entry {
            version: 1,
            artifacts,
        };
        let text =
            ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(&entry)?));
        install(&self.action(key), text.as_bytes())
    }
}
