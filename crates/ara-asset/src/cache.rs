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
/// Disk content-addressed storage: objects by byte digest, actions by input digest.
#[derive(Debug, Clone)]
pub struct FileCache {
    root: PathBuf,
    durable: bool,
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

fn detail_scope(stage: &'static str, subject: &str) -> Option<crate::trace::Scope> {
    static DETAIL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    (crate::trace::enabled()
        && *DETAIL.get_or_init(|| std::env::var_os("ARA_IMPORT_TRACE_CACHE_DETAIL").is_some()))
    .then(|| crate::trace::Scope::new(stage, subject).with_thread_cpu())
}

impl FileCache {
    fn install_object(&self, hash: Digest, bytes: &[u8]) -> Result<()> {
        let path = self.object(hash);
        // Intermediate and final results contain identical immutable objects.
        // Flush before publishing the final hard link, preserving the durable
        // object-before-action ordering without writing the payload twice.
        if self.durable && !path.exists() {
            let memo = self
                .root
                .join("intermediates/objects")
                .join(hash.to_string());
            let label = memo.display().to_string();
            let matches = {
                let _part = detail_scope("cache.object.memo_read", &label);
                fs::read(&memo).is_ok_and(|existing| existing == bytes)
            };
            let file = matches.then(|| {
                let _part = detail_scope("cache.object.open", &label);
                fs::OpenOptions::new().read(true).write(true).open(&memo)
            });
            if let Some(Ok(file)) = file {
                let _sync = crate::trace::Scope::new("cache.sync", &label);
                {
                    let _part = detail_scope("cache.object.sync", &label);
                    file.sync_all()?;
                }
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
        install(&path, bytes, self.durable)
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
            root,
            durable: true,
        })
    }
    pub(crate) fn intermediates(root: &Path) -> Result<Self> {
        let mut cache = Self::at(root.join("intermediates"))?;
        // Intermediate results are expendable. Final action/object publication
        // still uses the durable parent cache, including its own object fsync.
        cache.durable = false;
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
        let mut intermediates = GcStats::default();
        if self.durable && self.root.join("intermediates").is_dir() {
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
fn install(path: &Path, bytes: &[u8], durable: bool) -> Result<()> {
    let label = path.display().to_string();
    let _trace = crate::trace::Scope::new("cache.install", &label).bytes(bytes.len());
    let part = |stage| detail_scope(stage, &label);
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("cache path without parent".into()))?;
    // Repeated textures share objects. Avoid temporary writes and fsyncs while
    // retaining the immutable-entry conflict check.
    let existing = {
        let _part = part("cache.install.read");
        fs::read(path)
    };
    match existing {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(Error::Invalid(
                "immutable cache entry conflicts (nondeterministic cook or corruption)".into(),
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut temp = {
        let _part = part("cache.install.create");
        tempfile::NamedTempFile::new_in(parent)?
    };
    {
        let _part = part("cache.install.write");
        temp.write_all(bytes)?;
    }
    if durable {
        let _sync = crate::trace::Scope::new("cache.sync", &path.display().to_string());
        let _part = part("cache.install.sync");
        temp.as_file().sync_all()?;
    }
    let persisted = {
        let _part = part("cache.install.persist");
        temp.persist_noclobber(path)
    };
    match persisted {
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
    fn worker_memo_root(&self) -> Option<&Path> {
        Some(&self.root)
    }
    fn load(&self, key: Digest) -> Result<Option<CachedImport>> {
        let _trace = crate::trace::Scope::new("cache.load", &key.to_string());
        let entry = match self.entry(key) {
            Ok(entry) => entry,
            Err(_) if !self.durable => {
                fs::remove_file(self.action(key))?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = entry else {
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
                if !self.durable {
                    fs::remove_file(self.object(a.hash))?;
                    fs::remove_file(self.action(key))?;
                    return Ok(None);
                }
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
        install(&self.action(key), text.as_bytes(), self.durable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intermediate_cache_is_disposable_and_does_not_weaken_final_storage() {
        let root = tempfile::tempdir().expect("root");
        let final_cache = FileCache::at(root.path()).expect("final cache");
        let memo = FileCache::intermediates(root.path()).expect("memo");
        let key = Digest::of(b"key");
        let output = ImportOutput {
            artifacts: vec![Artifact {
                name: "data".into(),
                kind: ArtifactKind::Custom,
                bytes: vec![1, 2, 3],
            }],
        };
        memo.store(key, &output).expect("intermediate");
        assert!(final_cache.load(key).expect("separate namespace").is_none());
        final_cache.store(key, &output).expect("durable final");
        let hash = Digest::of(&output.artifacts[0].bytes);
        fs::remove_file(memo.object(hash)).expect("lose memo name");
        fs::write(memo.object(hash), b"broken").expect("simulate lost memo data");
        assert!(
            memo.load(key)
                .expect("damaged intermediate is a miss")
                .is_none()
        );
        memo.store(key, &output).expect("recompute");
        let roots = BTreeSet::from([key]);
        let gc = final_cache.collect(&roots).expect("quiescent GC");
        assert_eq!(gc.actions_removed, 1);
        assert!(memo.load(key).expect("memo evicted").is_none());
        assert_eq!(
            final_cache
                .load(key)
                .expect("final hit")
                .expect("root")
                .output,
            output
        );
        fs::write(final_cache.object(hash), b"broken").expect("damage final object");
        assert!(
            final_cache.load(key).is_err(),
            "published storage still rejects corruption"
        );
    }
}
