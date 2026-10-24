//! Read-only attachment to an owner's completed asset generations.
use ara_asset::{ArtifactKind, CacheBackend, Digest, FileCache, HotReload};
use ara_runtime::assets::AssetSource;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, RwLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

/// One complete, persisted cook; pending and failed work never enters this record.
#[derive(Clone)]
pub struct PublishedGeneration {
    digest: Digest,
    kinds: Vec<ArtifactKind>,
}
impl PublishedGeneration {
    /// Immutable FileCache action identity.
    pub fn digest(&self) -> Digest {
        self.digest
    }
    /// Artifact kinds present in the completed cook, in producer order without duplicates.
    pub fn kinds(&self) -> &[ArtifactKind] {
        &self.kinds
    }
}

/// Immutable generation snapshot. Loading reads checked FileCache entries; it can
/// neither schedule imports nor change which generations the owner publishes.
pub struct PublishedSnapshot {
    revision: u64,
    entries: BTreeMap<String, PublishedGeneration>,
    cache: FileCache,
}
impl PublishedSnapshot {
    /// Owner publication revision captured by this snapshot.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Root of the file-backed content-addressed cache; no cache is opened by a reader.
    pub fn cache_path(&self) -> &Path {
        self.cache.root()
    }
    /// Complete published metadata for an asset ID.
    pub fn generation(&self, asset: &str) -> Option<&PublishedGeneration> {
        self.entries.get(asset)
    }
    /// Stable asset-ID order, suitable for incremental model/material refresh.
    pub fn generations(&self) -> impl Iterator<Item = (&str, &PublishedGeneration)> {
        self.entries
            .iter()
            .map(|(id, generation)| (id.as_str(), generation))
    }
}
impl AssetSource for PublishedSnapshot {
    fn digest(&self, asset: &str) -> Option<Digest> {
        self.entries.get(asset).map(|generation| generation.digest)
    }

    fn load(&self, asset: &str) -> ara_runtime::Result<Option<(Digest, ara_asset::CachedImport)>> {
        let Some(generation) = self.entries.get(asset) else {
            return Ok(None);
        };
        self.cache
            .load(generation.digest)
            .map(|entry| entry.map(|entry| (generation.digest, entry)))
            .map_err(|error| ara_runtime::RuntimeError(error.to_string()))
    }
}

/// Where the owner's imports stand, for views that show loading (the editor's status
/// chip, Assets spinners, the Scene overlay). Every registered source is exactly one of
/// done, pending or failed. The owner republishes it only when it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportProgress {
    /// Sources with a current generation and nothing newer in flight.
    pub done: usize,
    /// Registered sources.
    pub total: usize,
    /// Sources with no generation yet, or a newer one cooking.
    pub pending: BTreeSet<String>,
    /// Sources whose latest import failed, with the error. They keep their last good
    /// generation, else a placeholder.
    pub failed: BTreeMap<String, String>,
}

#[derive(Default)]
struct Shared {
    revision: AtomicU64,
    snapshot: RwLock<Option<Arc<PublishedSnapshot>>>,
    progress_revision: AtomicU64,
    progress: RwLock<Option<Arc<ImportProgress>>>,
}
// Only project attachment takes this registry lock. Frame loops read an atomic
// revision and retain their immutable snapshot; no registry or import lock is used.
type Registry = BTreeMap<PathBuf, Weak<Shared>>;
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

/// Read-only project attachment. Acquire once at a view/control boundary, then
/// check `revision` each frame and request a snapshot only when that value changes.
pub struct PublishedAssets {
    shared: Arc<Shared>,
}
impl PublishedAssets {
    /// Attaches to a project, including before its owner initializes assets. This
    /// does no discovery, cache creation, worker launch, cooking or waiting.
    pub fn for_project(root: &Path) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let mut registry = REGISTRY
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "published-asset attachment registry poisoned".to_owned())?;
        if let Some(shared) = registry.get(&root).and_then(Weak::upgrade) {
            return Ok(Self { shared });
        }
        registry.retain(|_, value| value.strong_count() != 0);
        let shared = Arc::new(Shared::default());
        registry.insert(root, Arc::downgrade(&shared));
        Ok(Self { shared })
    }
    /// Cheap lock-free publication counter; zero means the owner has no snapshot yet.
    pub fn revision(&self) -> u64 {
        self.shared.revision.load(Ordering::Acquire)
    }
    /// Retains the latest immutable publication without waiting for a writer or
    /// pending import. `None` means unavailable this boundary; retain the last good view.
    pub fn snapshot(&self) -> Option<Arc<PublishedSnapshot>> {
        let value = self.shared.snapshot.try_read().ok()?;
        value.as_ref().map(Arc::clone)
    }
    /// Cheap lock-free import-progress counter; zero until the owner's pipeline starts.
    /// Poll it each frame and call [`Self::progress`] only when it changes.
    pub fn progress_revision(&self) -> u64 {
        self.shared.progress_revision.load(Ordering::Acquire)
    }
    /// The latest import progress, without waiting for the owner. `None` before the
    /// pipeline starts or while the owner is publishing; keep the last one you read.
    pub fn progress(&self) -> Option<Arc<ImportProgress>> {
        let value = self.shared.progress.try_read().ok()?;
        value.as_ref().map(Arc::clone)
    }
}

impl crate::PlayService {
    /// Read-only attachment to this project's published generations. Acquire once;
    /// it never initializes the service's asset pipeline or waits for its jobs.
    pub fn published(&self) -> Result<PublishedAssets, String> {
        PublishedAssets::for_project(&self.root)
    }
}

pub(crate) struct Publisher {
    reader: PublishedAssets,
    current: Arc<PublishedSnapshot>,
    dirty: bool,
    progress: Option<Arc<ImportProgress>>,
    progress_dirty: bool,
}
impl Publisher {
    pub(crate) fn new(root: &Path, cache: FileCache) -> Result<Self, String> {
        let reader = PublishedAssets::for_project(root)?;
        let current = reader.snapshot().unwrap_or_else(|| {
            Arc::new(PublishedSnapshot {
                revision: 1,
                entries: BTreeMap::new(),
                cache,
            })
        });
        let mut publisher = Self {
            reader,
            current,
            dirty: true,
            progress: None,
            progress_dirty: false,
        };
        publisher.flush();
        Ok(publisher)
    }
    // Called at publication boundaries. Metadata copies occur only for completed
    // changes, never for pending work or ordinary unchanged frame polls.
    pub(crate) fn complete(&mut self, changes: &[ara_asset::AssetChanged], reload: &HotReload) {
        if changes.iter().any(|change| {
            self.current.entries.get(&change.asset).map(|g| g.digest) != Some(change.current)
        }) {
            let mut entries = self.current.entries.clone();
            for change in changes {
                let Some(cook) = reload
                    .slot(&change.asset)
                    .and_then(|slot| slot.current.as_ref())
                else {
                    continue;
                };
                let mut kinds = Vec::new();
                for artifact in &cook.output.artifacts {
                    if !kinds.contains(&artifact.kind) {
                        kinds.push(artifact.kind);
                    }
                }
                entries.insert(
                    change.asset.to_owned(),
                    PublishedGeneration {
                        digest: cook.key,
                        kinds,
                    },
                );
            }
            self.current = Arc::new(PublishedSnapshot {
                revision: self.current.revision.saturating_add(1),
                entries,
                cache: self.current.cache.clone(),
            });
            self.dirty = true;
        }
        self.flush();
    }
    pub(crate) fn retain(&mut self, sources: &std::collections::BTreeSet<String>) {
        if self.current.entries.keys().any(|id| !sources.contains(id)) {
            let entries = self
                .current
                .entries
                .iter()
                .filter(|(id, _)| sources.contains(*id))
                .map(|(id, generation)| (id.to_owned(), generation.clone()))
                .collect();
            self.current = Arc::new(PublishedSnapshot {
                revision: self.current.revision.saturating_add(1),
                entries,
                cache: self.current.cache.clone(),
            });
            self.dirty = true;
        }
        self.flush();
    }
    /// Publishes `next` if it differs from the last progress (an O(sources) compare,
    /// made only when the owner saw import activity).
    pub(crate) fn progress(&mut self, next: ImportProgress) {
        if self.progress.as_deref() != Some(&next) {
            self.progress = Some(Arc::new(next));
            self.progress_dirty = true;
        }
        self.flush();
    }
    fn flush(&mut self) {
        if self.progress_dirty
            && let Ok(mut value) = self.reader.shared.progress.try_write()
        {
            *value = self.progress.clone();
            drop(value);
            self.reader
                .shared
                .progress_revision
                .fetch_add(1, Ordering::Release);
            self.progress_dirty = false;
        }
        if self.dirty
            && let Ok(mut value) = self.reader.shared.snapshot.try_write()
        {
            *value = Some(Arc::clone(&self.current));
            // Readers observing this revision must be able to acquire its snapshot.
            drop(value);
            self.reader
                .shared
                .revision
                .store(self.current.revision, Ordering::Release);
            self.dirty = false;
        }
    }
}
