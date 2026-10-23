use crate::{
    CacheBackend, CachedImport, Cancellation, DependencyGraph, Digest, Error, ImportInput,
    ProcessWorker, Progress, Result,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
};

/// Human-selected application timing, enforced only at explicit frame/tick boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyPolicy {
    /// Publish at the next permitted boundary.
    Immediately,
    /// Wait for an idle editor boundary.
    WhenIdle,
    /// Wait for an explicit per-asset approval.
    OnApproval,
}
/// A stable asset slot. Failed or incomplete imports never replace `current`.
#[derive(Debug, Default)]
pub struct AssetSlot {
    /// Last validated and published generation, retained through import errors.
    pub current: Option<CachedImport>,
    /// Published generation counter.
    pub generation: u64,
}
/// Published only after a complete generation swaps at a frame/tick boundary.
#[derive(Debug, Clone)]
pub struct AssetChanged {
    /// Stable authoring asset ID.
    pub asset: String,
    /// Prior action digest, if this was a reload.
    pub previous: Option<Digest>,
    /// New action digest.
    pub current: Digest,
    /// Monotonic generation.
    pub generation: u64,
}
/// Background events suitable for protocol job/subscription adapters.
#[derive(Debug)]
pub enum ReloadEvent {
    /// A worker/cache stage reported progress.
    Progress {
        /// Asset being imported.
        asset: String,
        /// Stage details.
        progress: Progress,
    },
    /// A complete result is staged, awaiting the human's application policy.
    Ready {
        /// Asset ready for publication.
        asset: String,
        /// True if no importer process was necessary.
        cache_hit: bool,
        /// False for a preview; true when the requested final preset is complete.
        final_pass: bool,
    },
    /// Source/worker/cache failure; the last good slot is unchanged.
    Failed {
        /// Asset whose new generation failed.
        asset: String,
        /// Structured pipeline failure.
        error: Error,
    },
}
enum Background {
    Progress(String, u64, Progress),
    Complete(String, u64, bool, Result<(CachedImport, bool)>),
}
/// Owns asynchronous jobs and stable slots. Frame code only drains events and moves
/// completed results; hashing, disk I/O and importers run on background threads.
pub struct HotReload {
    worker: ProcessWorker,
    cache: Arc<dyn CacheBackend>,
    tx: mpsc::Sender<Background>,
    rx: Receiver<Background>,
    epochs: BTreeMap<String, u64>,
    jobs: BTreeMap<String, Cancellation>,
    slots: BTreeMap<String, AssetSlot>,
    ready: BTreeMap<String, CachedImport>,
    retired: mpsc::Sender<CachedImport>,
    /// Publish fast previews before the requested final preset on cache misses.
    /// Disable for export/headless workflows, which execute the input preset exactly.
    pub progressive: bool,
    /// Human-selected application timing.
    pub policy: ApplyPolicy,
    /// Defaults false: imports cannot alter a human's Play session without consent.
    pub allow_during_play: bool,
    /// Queryable graph, updated when a source snapshot is requested.
    pub graph: DependencyGraph,
}
impl HotReload {
    /// Starts with no active generations; does not create a daemon or watcher.
    pub fn new(worker: ProcessWorker, cache: Arc<dyn CacheBackend>) -> Self {
        let (tx, rx) = mpsc::channel();
        let (retired, retired_rx) = mpsc::channel::<CachedImport>();
        thread::spawn(move || {
            crate::worker::background_priority();
            while let Ok(generation) = retired_rx.recv() {
                drop(generation);
            }
        });
        Self {
            worker,
            cache,
            tx,
            rx,
            epochs: BTreeMap::new(),
            jobs: BTreeMap::new(),
            slots: BTreeMap::new(),
            ready: BTreeMap::new(),
            retired,
            progressive: true,
            policy: ApplyPolicy::Immediately,
            allow_during_play: false,
            graph: DependencyGraph::default(),
        }
    }
    /// Cancels superseded jobs and schedules cache lookup/cook without blocking.
    pub fn request(&mut self, asset: impl Into<String>, input: ImportInput) {
        let asset = asset.into();
        if let Some(old) = self.jobs.remove(&asset) {
            old.cancel();
        }
        if let Some(staged) = self.ready.remove(&asset) {
            let _ = self.retired.send(staged);
        }
        let epoch = self.epochs.entry(asset.clone()).or_default();
        *epoch += 1;
        let epoch = *epoch;
        let cancellation = Cancellation::default();
        self.jobs.insert(asset.clone(), cancellation.clone());
        let tx = self.tx.clone();
        let worker = self.worker.clone();
        let cache = Arc::clone(&self.cache);
        self.graph.replace(
            asset.clone(),
            std::iter::once(input.source_path.clone())
                .chain(input.dependencies.keys().map(|uri| {
                    let parent = Path::new(&input.source_path)
                        .parent()
                        .unwrap_or(Path::new(""));
                    normalize_path(&parent.join(uri))
                }))
                .chain(input.asset_dependencies.keys().cloned()),
        );
        let progressive = self.progressive;
        thread::spawn(move || {
            crate::worker::background_priority();
            let result = (|| {
                cancellation.check()?;
                let key = input.key()?;
                if let Some(hit) = cache.load(key)? {
                    cancellation.check()?;
                    return Ok((hit, true));
                }
                if progressive && input.preset != crate::CookPreset::Fast {
                    let mut preview = input.clone();
                    preview.preset = crate::CookPreset::Fast;
                    let preview_key = preview.key()?;
                    if let Some(hit) = cache.load(preview_key)? {
                        cancellation.check()?;
                        let _ = tx.send(Background::Complete(
                            asset.clone(),
                            epoch,
                            false,
                            Ok((hit, true)),
                        ));
                    } else {
                        let output = worker.start(preview)?.wait(&cancellation, |p| {
                            let _ = tx.send(Background::Progress(asset.clone(), epoch, p));
                        })?;
                        cancellation.check()?;
                        // Publish validated bytes before potentially slow disk persistence.
                        // This copy and the write stay on the background job, never a frame.
                        let ready = CachedImport {
                            key: preview_key,
                            output: output.clone(),
                        };
                        let _ = tx.send(Background::Complete(
                            asset.clone(),
                            epoch,
                            false,
                            Ok((ready, false)),
                        ));
                        cache.store(preview_key, &output)?;
                    }
                }
                cook_cached(&worker, cache.as_ref(), input, &cancellation, |p| {
                    let _ = tx.send(Background::Progress(asset.clone(), epoch, p));
                })
            })();
            let _ = tx.send(Background::Complete(asset, epoch, true, result));
        });
    }
    /// Drains job events, rejecting stale results even if a cancelled worker raced.
    pub fn poll(&mut self) -> Vec<ReloadEvent> {
        let mut events = Vec::new();
        for message in self.rx.try_iter().take(64) {
            match message {
                Background::Progress(asset, epoch, progress)
                    if self.epochs.get(&asset) == Some(&epoch) =>
                {
                    events.push(ReloadEvent::Progress { asset, progress })
                }
                Background::Complete(asset, epoch, terminal, result)
                    if self.epochs.get(&asset) == Some(&epoch) =>
                {
                    if terminal {
                        self.jobs.remove(&asset);
                    }
                    match result {
                        Ok((cached, cache_hit)) => {
                            if let Some(staged) = self.ready.insert(asset.clone(), cached) {
                                let _ = self.retired.send(staged);
                            }
                            events.push(ReloadEvent::Ready {
                                asset,
                                cache_hit,
                                final_pass: terminal,
                            });
                        }
                        Err(error) => {
                            error.diagnostic(&asset).emit();
                            events.push(ReloadEvent::Failed { asset, error })
                        }
                    }
                }
                Background::Complete(_, _, _, Ok((cached, _))) => {
                    let _ = self.retired.send(cached);
                }
                _ => {}
            }
        }
        events
    }
    /// Publishes staged generations at a caller-designated tick/frame boundary.
    /// `approved` is explicit per-asset consent, including while Play is running.
    pub fn apply_boundary(
        &mut self,
        idle: bool,
        playing: bool,
        approved: &BTreeSet<String>,
    ) -> Vec<AssetChanged> {
        let allowed: Vec<_> = self
            .ready
            .keys()
            .filter(|asset| {
                (!playing || self.allow_during_play || approved.contains(*asset))
                    && match self.policy {
                        ApplyPolicy::Immediately => true,
                        ApplyPolicy::WhenIdle => idle || approved.contains(*asset),
                        ApplyPolicy::OnApproval => approved.contains(*asset),
                    }
            })
            .cloned()
            .collect();
        let mut events = Vec::new();
        for asset in allowed {
            if let Some(ready) = self.ready.remove(&asset) {
                let slot = self.slots.entry(asset.clone()).or_default();
                let previous = slot.current.as_ref().map(|c| c.key);
                if previous == Some(ready.key) {
                    let _ = self.retired.send(ready);
                    continue;
                }
                let current = ready.key;
                if let Some(old) = slot.current.replace(ready) {
                    let _ = self.retired.send(old);
                }
                slot.generation += 1;
                events.push(AssetChanged {
                    asset,
                    previous,
                    current,
                    generation: slot.generation,
                });
            }
        }
        events
    }
    /// Reads a stable slot by asset ID.
    pub fn slot(&self, asset: &str) -> Option<&AssetSlot> {
        self.slots.get(asset)
    }
    /// Roots for GC; quiesce active jobs before collecting.
    pub fn cache_roots(&self) -> BTreeSet<Digest> {
        self.slots
            .values()
            .filter_map(|s| s.current.as_ref().map(|c| c.key))
            .chain(self.ready.values().map(|c| c.key))
            .collect()
    }
    /// Cancels a pending import, preserving the current live asset.
    pub fn cancel(&mut self, asset: &str) {
        if let Some(job) = self.jobs.remove(asset) {
            job.cancel();
        }
        if let Some(e) = self.epochs.get_mut(asset) {
            *e += 1;
        }
        if let Some(staged) = self.ready.remove(asset) {
            let _ = self.retired.send(staged);
        }
    }
    /// Whether work is still in flight (staged results do not count).
    pub fn is_importing(&self) -> bool {
        !self.jobs.is_empty()
    }
}
impl Drop for HotReload {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel();
        }
    }
}

fn cook_cached(
    worker: &ProcessWorker,
    cache: &dyn CacheBackend,
    input: ImportInput,
    cancellation: &Cancellation,
    progress: impl FnMut(Progress),
) -> Result<(CachedImport, bool)> {
    cancellation.check()?;
    let key = input.key()?;
    if let Some(hit) = cache.load(key)? {
        cancellation.check()?;
        return Ok((hit, true));
    }
    let output = worker.start(input)?.wait(cancellation, progress)?;
    cancellation.check()?;
    cache.store(key, &output)?;
    cancellation.check()?;
    Ok((CachedImport { key, output }, false))
}

fn normalize_path(path: &Path) -> String {
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(p) => parts.push(p.to_string_lossy()),
            _ => {}
        }
    }
    parts.join("/")
}

/// Native filesystem watcher. Burst events coalesce by normalized source path.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    root: PathBuf,
}
impl FileWatcher {
    /// Watches the source tree; derived `.ara` cache changes are excluded on drain.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let (tx, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .map_err(|e| Error::Invalid(e.to_string()))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(Self {
            _watcher: watcher,
            events,
            root,
        })
    }
    /// Drains a bounded event burst, ignoring read/access and derived-directory churn.
    pub fn changed(&self) -> Result<BTreeSet<String>> {
        let mut paths = BTreeSet::new();
        for event in self.events.try_iter().take(256) {
            let event = event.map_err(|e| Error::Invalid(e.to_string()))?;
            if matches!(event.kind, notify::EventKind::Access(_)) {
                continue;
            }
            for path in event.paths {
                if let Ok(relative) = path.strip_prefix(&self.root) {
                    if relative.components().next().is_some_and(|c| {
                        [".ara", ".git", "target"]
                            .iter()
                            .any(|name| c.as_os_str() == *name)
                    }) {
                        continue;
                    }
                    paths.insert(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        Ok(paths)
    }
    /// Resolves a watcher burst to the minimal set of transitively affected assets.
    pub fn invalidated(&self, graph: &DependencyGraph) -> Result<BTreeSet<String>> {
        Ok(self
            .changed()?
            .iter()
            .flat_map(|p| graph.invalidated_by(p))
            .collect())
    }
}
