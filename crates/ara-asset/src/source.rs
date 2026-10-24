use crate::{
    AssetChanged, CacheBackend, Cancellation, Digest, Error, FileWatcher, HotReload, ImportInput,
    ImportSettings, ProcessWorker, ReloadEvent, Result, Sidecar,
};
use ara_schema::SchemaDb;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
};

/// Host-side source registration. Configuration changes come from typed sidecar ops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAsset {
    /// Final project quality, also used by nonprogressive shipping cooks.
    #[serde(default)]
    pub preset: crate::CookPreset,
    /// Maximum import worker threads; does not affect artifact identity.
    #[serde(default = "crate::preset::default_thread_budget")]
    pub thread_budget: u16,
    /// Stable ID from the import sidecar.
    pub id: String,
    /// Project-relative media path.
    pub path: String,
    /// Stable importer ID installed in the process worker.
    pub importer: String,
    /// Exact importer version including encoder implementations.
    pub version: String,
    /// Importer-specific canonical settings.
    pub settings: serde_json::Value,
    /// Target-specific cook profile.
    pub target: crate::TargetPlatform,
    /// Content hashes of any cooked asset inputs.
    pub asset_dependencies: BTreeMap<String, Digest>,
}
/// Snapshots source and dependencies on a background/CLI thread. All native source
/// parsing, including dependency discovery, runs in a disposable process.
pub fn snapshot(
    root: &Path,
    asset: &SourceAsset,
    worker: &ProcessWorker,
    cancellation: &Cancellation,
) -> Result<ImportInput> {
    let root = root
        .canonicalize()
        .map_err(|e| io_at(e, &root.display().to_string()))?;
    let source_path = checked_path(&root, &root, &asset.path)?;
    let source = std::fs::read(&source_path).map_err(|e| io_at(e, &asset.path))?;
    let parent = source_path
        .parent()
        .ok_or_else(|| Error::Invalid("source has no parent".into()))?;
    let mut dependencies = BTreeMap::new();
    for uri in worker.discover_with_settings(
        &asset.importer,
        &asset.version,
        source.clone(),
        asset.settings.clone(),
        cancellation,
    )? {
        cancellation.check()?;
        let path = checked_path(&root, parent, &uri)?;
        let bytes = std::fs::read(path).map_err(|e| io_at(e, &uri))?;
        dependencies.insert(uri, bytes);
    }
    cancellation.check()?;
    Ok(ImportInput {
        preset: asset.preset,
        thread_budget: asset.thread_budget,
        source_path: asset.path.replace('\\', "/"),
        source,
        importer: asset.importer.clone(),
        version: asset.version.clone(),
        settings: asset.settings.clone(),
        target: asset.target,
        dependencies,
        asset_dependencies: asset.asset_dependencies.clone(),
    })
}
/// An I/O error that names the file it is about (Problems shows which file).
fn io_at(error: std::io::Error, path: &str) -> Error {
    Error::Io(std::io::Error::new(
        error.kind(),
        format!("`{path}`: {error}"),
    ))
}
fn checked_path(root: &Path, base: &Path, uri: &str) -> Result<PathBuf> {
    let relative = Path::new(uri);
    if uri.contains(':')
        || uri.contains('\\')
        || relative.is_absolute()
        || relative
            .components()
            .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
    {
        return Err(Error::Invalid(
            "dependency URI must be a relative project path".into(),
        ));
    }
    let path = base
        .join(relative)
        .canonicalize()
        .map_err(|e| io_at(e, uri))?;
    if !path.starts_with(root) {
        return Err(Error::Invalid("dependency escapes project root".into()));
    }
    Ok(path)
}
struct SnapshotResult {
    asset: String,
    epoch: u64,
    result: Result<ImportInput>,
}
/// End-to-end watcher → snapshot → incremental process cook → staged hot reload.
/// Own it for the editor session; dropping cancels all work and removes the watcher.
pub struct AssetPipeline {
    root: PathBuf,
    worker: ProcessWorker,
    schema: Arc<SchemaDb>,
    watcher: FileWatcher,
    sources: BTreeMap<String, SourceAsset>,
    configurations: BTreeMap<String, ImportSettings>,
    epochs: BTreeMap<String, u64>,
    snapshots: BTreeMap<String, Cancellation>,
    snapshot_changes: BTreeSet<String>,
    tx: mpsc::Sender<SnapshotResult>,
    rx: Receiver<SnapshotResult>,
    /// Stable runtime slots and human-selected boundary application policy.
    pub reload: HotReload,
}
impl AssetPipeline {
    /// Attaches a source watcher. The provided schema must include `register_schema`.
    pub fn new(
        root: impl AsRef<Path>,
        worker: ProcessWorker,
        cache: Arc<dyn CacheBackend>,
        schema: Arc<SchemaDb>,
    ) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let watcher = FileWatcher::new(&root)?;
        let (tx, rx) = mpsc::channel();
        Ok(Self {
            root,
            worker: worker.clone(),
            schema,
            watcher,
            sources: BTreeMap::new(),
            configurations: BTreeMap::new(),
            epochs: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            snapshot_changes: BTreeSet::new(),
            tx,
            rx,
            reload: HotReload::new(worker, cache),
        })
    }
    /// Registers source media. It is imported by request or by the next file change.
    pub fn register(&mut self, asset: SourceAsset) -> Result<()> {
        Self::validate_source(&asset)?;
        if self.sources.contains_key(&asset.id) {
            return Err(Error::Invalid("source asset already registered".into()));
        }
        self.sources.insert(asset.id.clone(), asset);
        Ok(())
    }

    fn validate_source(asset: &SourceAsset) -> Result<()> {
        ara_doc::AssetId::parse(&asset.id).map_err(Error::Invalid)?;
        if asset.path.is_empty()
            || asset.path.contains(':')
            || asset.path.contains('\\')
            || Path::new(&asset.path)
                .components()
                .any(|p| !matches!(p, Component::Normal(_)))
        {
            return Err(Error::Invalid(
                "source path must be project-relative with / separators".into(),
            ));
        }
        Ok(())
    }

    /// Uses command-authored in-memory settings for subsequent immutable snapshots,
    /// without waiting for a debounced sidecar save. Existing generations stay live.
    pub fn configure(
        &mut self,
        asset: SourceAsset,
        dependencies: Vec<ara_schema::AssetRef>,
    ) -> Result<()> {
        Self::validate_source(&asset)?;
        self.configurations.insert(
            asset.id.clone(),
            ImportSettings {
                preset: asset.preset,
                importer: asset.importer.clone(),
                version: asset.version.clone(),
                settings: asset.settings.clone(),
                dependencies,
            },
        );
        self.sources.insert(asset.id.clone(), asset);
        Ok(())
    }

    /// Stops requesting an undone/deleted import. Published generations remain cached
    /// for existing players and redo, but no late job can publish a deleted source.
    pub fn unregister(&mut self, id: &str) {
        self.sources.remove(id);
        self.configurations.remove(id);
        if let Some(job) = self.snapshots.remove(id) {
            job.cancel();
        }
        self.reload.cancel(id);
        *self.epochs.entry(id.to_owned()).or_default() += 1;
    }
    /// Schedules an immutable snapshot. Existing slots stay live during the cook.
    pub fn request(&mut self, id: &str) -> Result<()> {
        let mut asset = self
            .sources
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("unregistered source {id}")))?;
        if let Some(job) = self.snapshots.remove(id) {
            job.cancel();
        }
        self.reload.cancel(id);
        let epoch = self.epochs.entry(id.to_owned()).or_default();
        *epoch += 1;
        let epoch = *epoch;
        let cancellation = Cancellation::default();
        self.snapshots.insert(id.to_owned(), cancellation.clone());
        let tx = self.tx.clone();
        let root = self.root.clone();
        let worker = self.worker.clone();
        let schema = Arc::clone(&self.schema);
        let configuration = self.configurations.get(id).cloned();
        let known_assets: BTreeMap<_, _> = self
            .sources
            .iter()
            .map(|(id, s)| {
                (
                    id.clone(),
                    (
                        s.path.clone(),
                        self.reload
                            .slot(id)
                            .and_then(|slot| slot.current.as_ref())
                            .map(|c| c.key),
                    ),
                )
            })
            .collect();
        thread::spawn(move || {
            crate::worker::background_priority();
            let result = (|| {
                let sidecar_path = root.join(format!("{}.import.json", asset.path));
                if let Ok(resolved) = sidecar_path.canonicalize()
                    && !resolved.starts_with(&root)
                {
                    return Err(Error::Invalid("sidecar escapes project root".into()));
                }
                let settings = match configuration {
                    Some(settings) => Some(settings),
                    None => match std::fs::read_to_string(&sidecar_path) {
                        Ok(text) => {
                            let sidecar =
                                Sidecar::read(&text, &sidecar_path.to_string_lossy(), &schema)?;
                            if sidecar.document().id.as_str() != asset.id {
                                return Err(Error::Invalid(
                                    "sidecar identity changed; reindex the source before importing"
                                        .into(),
                                ));
                            }
                            Some(sidecar.settings()?)
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                        Err(e) => return Err(e.into()),
                    },
                };
                if let Some(settings) = settings {
                    asset.importer = settings.importer;
                    asset.version = settings.version;
                    asset.settings = settings.settings;
                    asset.preset = settings.preset;
                    for reference in settings.dependencies {
                        let parts =
                            ara_schema::grammar::parse_ref(&reference.0).map_err(Error::Invalid)?;
                        let known = parts.id.and_then(|id| known_assets.get(id));
                        let path = known
                            .map(|(path, _)| path.as_str())
                            .or(parts.path)
                            .ok_or_else(|| {
                                Error::Invalid("sidecar dependency is unresolved".into())
                            })?;
                        let digest = match known.and_then(|(_, hash)| *hash) {
                            Some(hash) => hash,
                            None => Digest::of(&std::fs::read(checked_path(&root, &root, path)?)?),
                        };
                        asset.asset_dependencies.insert(path.to_owned(), digest);
                        if let Some(id) = parts.id {
                            asset.asset_dependencies.insert(id.to_owned(), digest);
                        }
                    }
                }
                snapshot(&root, &asset, &worker, &cancellation)
            })();
            let _ = tx.send(SnapshotResult {
                asset: asset.id,
                epoch,
                result,
            });
        });
        Ok(())
    }
    /// Drains filesystem and worker events. No parsing, hashing or disk reads run
    /// here. Publication remains the caller's explicit `reload.apply_boundary`.
    pub fn poll(&mut self) -> Result<Vec<ReloadEvent>> {
        let mut events = Vec::new();
        for completed in self.rx.try_iter().take(64).collect::<Vec<_>>() {
            if self.epochs.get(&completed.asset) != Some(&completed.epoch) {
                continue;
            }
            self.snapshots.remove(&completed.asset);
            match completed.result {
                Ok(input) => self.reload.request(completed.asset, input),
                Err(error) => {
                    error.diagnostic(&completed.asset).emit();
                    events.push(ReloadEvent::Failed {
                        asset: completed.asset,
                        error,
                    })
                }
            }
        }
        self.snapshot_changes.extend(self.watcher.changed()?);
        let mut invalidated = BTreeSet::new();
        let mut handled = Vec::new();
        for path in &self.snapshot_changes {
            let users = self.reload.graph.invalidated_by(path);
            let mut known = !users.is_empty();
            invalidated.extend(users);
            for (id, asset) in &self.sources {
                if *path == asset.path || *path == format!("{}.import.json", asset.path) {
                    invalidated.insert(id.clone());
                    known = true;
                }
            }
            if known {
                handled.push(path.clone());
            }
        }
        for path in handled {
            self.snapshot_changes.remove(&path);
        }
        if self.snapshots.is_empty() {
            self.snapshot_changes.clear();
        }
        for id in invalidated {
            if self.sources.contains_key(&id) {
                self.request(&id)?;
            }
        }
        events.extend(self.reload.poll());
        Ok(events)
    }
    /// Whether source snapshots or native cooks remain in flight.
    pub fn is_importing(&self) -> bool {
        !self.snapshots.is_empty() || self.reload.is_importing()
    }

    /// Whether the requested generation of one source is still being prepared.
    pub fn is_importing_asset(&self, id: &str) -> bool {
        self.snapshots.contains_key(id) || self.reload.is_importing_asset(id)
    }
    /// Applies a human-permitted boundary, then invalidates consumers of published
    /// cooked inputs. Dependent cooks pick up the new dependency action hashes.
    pub fn apply_boundary(
        &mut self,
        idle: bool,
        playing: bool,
        approved: &BTreeSet<String>,
    ) -> Result<Vec<AssetChanged>> {
        let changed = self.reload.apply_boundary(idle, playing, approved);
        let mut affected = BTreeSet::new();
        for event in &changed {
            affected.extend(self.reload.graph.invalidated_by(&event.asset));
            for source in self.sources.values_mut() {
                if let Some(hash) = source.asset_dependencies.get_mut(&event.asset) {
                    *hash = event.current;
                }
            }
        }
        for id in affected {
            if self.sources.contains_key(&id) {
                self.request(&id)?;
            }
        }
        Ok(changed)
    }
}
impl Drop for AssetPipeline {
    fn drop(&mut self) {
        for token in self.snapshots.values() {
            token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_sources_name_their_path() {
        let root = std::env::temp_dir();
        let error = checked_path(&root, &root, "assets/kit/no-such-model.gltf")
            .expect_err("the file does not exist");
        assert!(
            error
                .to_string()
                .contains("`assets/kit/no-such-model.gltf`"),
            "{error}"
        );
    }
}
