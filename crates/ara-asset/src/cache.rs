use crate::{Artifact, ArtifactKind, Digest, Error, ImportOutput, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
mod admission;
mod install;
#[cfg(test)]
mod tests;

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
    /// Optional local root passed to disposable workers for intermediate results.
    /// Remote/in-memory backends keep the default and still cache final actions.
    fn worker_memo_root(&self) -> Option<&Path> {
        None
    }
    /// Returns a checked action result, or a cache miss.
    fn load(&self, key: Digest) -> Result<Option<CachedImport>>;
    /// Installs a complete result atomically; conflicting results are errors.
    fn store(&self, key: Digest, output: &ImportOutput) -> Result<()>;
}
/// Verified, disposable storage: objects by byte digest, actions by input digest.
/// Atomic publication avoids partial records; lost/corrupt data is a cache miss.
#[derive(Debug, Clone)]
pub struct FileCache {
    root: PathBuf,
    intermediate: bool,
    writes: admission::Volume,
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
    /// Logical bytes removed; other hard links may still retain an object.
    pub bytes_removed: u64,
}

pub(super) fn detail_scope(stage: &'static str, subject: &str) -> Option<crate::trace::Scope> {
    static DETAIL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    (crate::trace::enabled()
        && *DETAIL.get_or_init(|| std::env::var_os("ARA_IMPORT_TRACE_CACHE_DETAIL").is_some()))
    .then(|| crate::trace::Scope::new(stage, subject).with_thread_cpu())
}

impl FileCache {
    fn install_object(&self, hash: Digest, bytes: &[u8]) -> Result<()> {
        let path = self.object(hash);
        // Identical intermediate objects need no second payload write. Readers
        // verify complete bytes, including after lost unflushed writes.
        if !self.intermediate && !path.exists() {
            let memo = self
                .root
                .join("intermediates/objects")
                .join(hash.to_string());
            let label = memo.display().to_string();
            let matches = {
                let _part = detail_scope("cache.object.memo_read", &label);
                install::read(&memo).is_ok_and(|existing| existing == bytes)
            };
            if matches {
                let linked = {
                    let _part = detail_scope("cache.object.link", &label);
                    fs::hard_link(&memo, &path).is_ok()
                };
                if linked {
                    crate::trace::event(
                        "cache.reuse_object",
                        &hash.to_string(),
                        serde_json::json!({"bytes":bytes.len()}),
                    );
                    return Ok(());
                }
            }
        }
        // Filesystems without hard links retain the portable atomic writer.
        // A different payload under its byte digest is damaged, not a different
        // valid cook. Repair it atomically; never overwrite a file in place.
        install::bytes(&path, bytes, |_| Ok(false), &self.writes)
    }
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
        Ok(Self {
            writes: admission::Volume::new(&root)?,
            root,
            intermediate: false,
        })
    }
    pub(crate) fn intermediates(root: &Path) -> Result<Self> {
        let mut cache = Self::at(root.join("intermediates"))?;
        cache.intermediate = true;
        Ok(cache)
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
        let bytes = match install::read(&self.action(key)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(Self::parse_entry(&bytes))
    }
    fn parse_entry(bytes: &[u8]) -> Option<Entry> {
        serde_json::from_slice::<Entry>(bytes)
            .ok()
            .filter(|entry| entry.version == 1)
    }
    fn verified_output(&self, entry: Entry) -> Result<Option<ImportOutput>> {
        let mut artifacts = Vec::with_capacity(entry.artifacts.len());
        for a in entry.artifacts {
            let bytes = match install::read(&self.object(a.hash)) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if bytes.len() as u64 != a.length || Digest::of(&bytes) != a.hash {
                return Ok(None);
            }
            artifacts.push(Artifact {
                name: a.name,
                kind: a.kind,
                bytes,
            });
        }
        let output = ImportOutput { artifacts };
        Ok(output.validate().is_ok().then_some(output))
    }
    /// Collects unreachable actions/objects. Caller must quiesce writers and include
    /// active, staged and undo generations in `roots` (including other processes).
    pub fn collect(&self, roots: &BTreeSet<Digest>) -> Result<GcStats> {
        let mut intermediates = GcStats::default();
        if !self.intermediate && self.root.join("intermediates").is_dir() {
            intermediates = Self::intermediates(&self.root)?.collect(&BTreeSet::new())?;
        }
        let mut live = BTreeSet::new();
        for root in roots {
            if let Some(e) = self.entry(*root)? {
                for a in e.artifacts {
                    live.insert(a.hash.to_string());
                }
            }
        }
        let root_names: BTreeSet<_> = roots.iter().map(ToString::to_string).collect();
        let mut stats = intermediates;
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
impl CacheBackend for FileCache {
    fn worker_memo_root(&self) -> Option<&Path> {
        Some(&self.root)
    }
    fn load(&self, key: Digest) -> Result<Option<CachedImport>> {
        let _trace = crate::trace::Scope::new("cache.load", &key.to_string());
        let Some(entry) = self.entry(key)? else {
            return Ok(None);
        };
        // Verify the entire result before serving any artifact. Leave a bad
        // entry alone here so a reader cannot unlink a concurrent repair.
        Ok(self
            .verified_output(entry)?
            .map(|output| CachedImport { key, output }))
    }
    fn store(&self, key: Digest, output: &ImportOutput) -> Result<()> {
        let _trace = crate::trace::Scope::new("cache.store", &key.to_string());
        output.validate()?;
        let mut artifacts = Vec::new();
        for a in &output.artifacts {
            let hash = Digest::of(&a.bytes);
            self.install_object(hash, &a.bytes)?;
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
        // Objects finish publication first. An existing action is immutable only
        // while all its referenced bytes still verify; otherwise it is repaired.
        install::bytes(
            &self.action(key),
            text.as_bytes(),
            |existing| {
                let Some(entry) = Self::parse_entry(existing) else {
                    return Ok(false);
                };
                Ok(self.verified_output(entry)?.is_some())
            },
            &self.writes,
        )
    }
}
