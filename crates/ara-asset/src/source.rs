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
    /// Source quality override; None inherits the reflected project import policy.
    #[serde(default)]
    pub preset: Option<crate::CookPreset>,
    /// Maximum import worker threads; does not affect artifact identity.
    #[serde(default = "crate::preset::default_thread_budget")]
    pub thread_budget: u16,
    /// Stable ID from the import sidecar.
    pub id: String,
    /// Authored-root-relative media path (the asset reference path).
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

/// Resolves the authored root once for background source I/O. A root already
/// pointing at authored content (dependency inventory) remains unchanged.
fn authored_root(project: &Path) -> Result<PathBuf> {
    let bytes = match std::fs::read(project.join("ara.project.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(project.to_owned()),
        Err(error) => return Err(error.into()),
    };
    let marker = serde_json::from_slice(&bytes)?;
    if !ara_doc::detect_project_layout(&marker).is_current() {
        return Err(Error::ProjectLayout(ara_diag::Diagnostic::error(
            ara_diag::codes::PROJECT_MIGRATION_REQUIRED,
            "this project uses an older layout; migrate with `ara project migrate` or open in the Hub/editor to migrate before importing",
        ).file(project.join("ara.project.json").display().to_string())));
    }
    Ok(project.join(ara_doc::AUTHORING_ROOT))
}
/// Snapshots source and dependencies on a background/CLI thread. All native source
/// parsing, including dependency discovery, runs in a disposable process.
pub fn snapshot(
    root: &Path,
    asset: &SourceAsset,
    worker: &ProcessWorker,
    cancellation: &Cancellation,
) -> Result<ImportInput> {
    let mut db = SchemaDb::new();
    crate::register_schema(&mut db);
    let policy = crate::ProjectImportSettings::read(root, &db)?;
    snapshot_with_policy(root, asset, worker, cancellation, &policy)
}
fn snapshot_with_policy(
    root: &Path,
    asset: &SourceAsset,
    worker: &ProcessWorker,
    cancellation: &Cancellation,
    policy: &crate::ProjectImportSettings,
) -> Result<ImportInput> {
    let _trace = crate::trace::Scope::new("source.snapshot", &asset.path);
    let (settings, preset) =
        policy.resolved_settings(&asset.importer, asset.preset, &asset.settings)?;
    let project_root = root
        .canonicalize()
        .map_err(|e| io_at(e, &root.display().to_string()))?;
    let root = authored_root(&project_root)?
        .canonicalize()
        .map_err(|e| io_at(e, &project_root.display().to_string()))?;
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
        settings.clone(),
        cancellation,
    )? {
        cancellation.check()?;
        let path = checked_path(&root, parent, &uri)?;
        let bytes = std::fs::read(path).map_err(|e| io_at(e, &uri))?;
        dependencies.insert(uri, bytes);
    }
    cancellation.check()?;
    Ok(ImportInput {
        preset,
        thread_budget: asset.thread_budget,
        source_path: asset.path.replace('\\', "/"),
        source,
        importer: asset.importer.clone(),
        version: asset.version.clone(),
        settings,
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
    authored_root: PathBuf,
    worker: ProcessWorker,
    schema: Arc<SchemaDb>,
    watcher: FileWatcher,
    sources: BTreeMap<String, SourceAsset>,
    configurations: BTreeMap<String, ImportSettings>,
    project_policy: Option<crate::ProjectImportSettings>,
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
        let authored_root = authored_root(&root)?;
        let watcher = FileWatcher::new(&root)?;
        let project_policy = Some(crate::ProjectImportSettings::read(&root, &schema)?);
        let worker = worker.with_memo(cache.worker_memo_root());
        let (tx, rx) = mpsc::channel();
        Ok(Self {
            root,
            authored_root,
            worker: worker.clone(),
            schema,
            watcher,
            sources: BTreeMap::new(),
            configurations: BTreeMap::new(),
            project_policy,
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

    /// Update the global quality while retaining every typed per-importer policy.
    /// Existing callers can keep using this preset-only compatibility entry point.
    pub fn configure_project(&mut self, preset: crate::CookPreset) -> Result<bool> {
        let mut policy = self.project_policy.clone().unwrap_or_default();
        policy.preset = preset;
        self.configure_project_settings(policy)
    }
    /// Apply an authoritative in-memory policy before its debounced file save.
    /// Re-request only sources whose effective settings or quality actually change.
    pub fn configure_project_settings(
        &mut self,
        policy: crate::ProjectImportSettings,
    ) -> Result<bool> {
        if self.project_policy.as_ref() == Some(&policy) {
            return Ok(false);
        }
        let previous = self.project_policy.as_ref();
        let mut affected = Vec::new();
        for (id, source) in &self.sources {
            let next =
                policy.resolved_settings(&source.importer, source.preset, &source.settings)?;
            let changed = match previous {
                None => true,
                Some(previous) => {
                    previous.resolved_settings(&source.importer, source.preset, &source.settings)?
                        != next
                }
            };
            if changed {
                affected.push(id.clone());
            }
        }
        self.project_policy = Some(policy);
        for id in affected {
            self.request(&id)?;
        }
        Ok(true)
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
        let authored_root = self.authored_root.clone();
        let worker = self.worker.clone();
        let schema = Arc::clone(&self.schema);
        let configuration = self.configurations.get(id).cloned();
        let project_policy = self.project_policy.clone();
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
                let sidecar_path = authored_root.join(format!("{}.import.json", asset.path));
                if let Ok(resolved) = sidecar_path.canonicalize()
                    && !resolved.starts_with(&authored_root)
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
                            asset.preset = sidecar.preset_override()?;
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
                            None => Digest::of(
                                &std::fs::read(checked_path(&authored_root, &authored_root, path)?)
                                    .map_err(|e| io_at(e, path))?,
                            ),
                        };
                        asset.asset_dependencies.insert(path.to_owned(), digest);
                        if let Some(id) = parts.id {
                            asset.asset_dependencies.insert(id.to_owned(), digest);
                        }
                    }
                }
                match project_policy {
                    Some(policy) => {
                        snapshot_with_policy(&root, &asset, &worker, &cancellation, &policy)
                    }
                    None => snapshot(&root, &asset, &worker, &cancellation),
                }
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
        let prefix =
            (self.authored_root != self.root).then(|| format!("{}/", ara_doc::AUTHORING_ROOT));
        let mut policy_changed = false;
        for path in self.watcher.changed()? {
            if ara_doc::file_state::same_path(&path, crate::PROJECT_IMPORT_SETTINGS_FILE)
                || ara_doc::file_state::same_path(&path, crate::LEGACY_PROJECT_IMPORT_SETTINGS_FILE)
            {
                policy_changed = true;
            } else if let Some(prefix) = &prefix {
                if let Some(relative) = path.strip_prefix(prefix) {
                    self.snapshot_changes.insert(relative.to_owned());
                }
            } else {
                self.snapshot_changes.insert(path);
            }
        }
        let mut invalidated = BTreeSet::new();
        if policy_changed {
            self.project_policy = None;
            invalidated.extend(self.sources.keys().cloned());
        }
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
    fn authoritative_type_default_change_requeues_only_inheriting_sources_before_file_save() {
        let directory = tempfile::tempdir().expect("pipeline scratch");
        let root = directory.path();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
            ),
        )
        .expect("marker");
        std::fs::create_dir(root.join(ara_doc::AUTHORING_ROOT)).expect("authored root");
        let mut db = SchemaDb::new();
        crate::register_schema(&mut db);
        let cache = Arc::new(crate::FileCache::new(root).expect("private cache"));
        let mut pipeline = AssetPipeline::new(
            root,
            ProcessWorker::new(root.join("never-launched-worker")),
            cache,
            Arc::new(db),
        )
        .expect("pipeline");
        for (id, settings) in [
            ("as_inherit", serde_json::json!({})),
            (
                "as_explicit",
                serde_json::to_value(crate::TextureSettings::default()).expect("explicit settings"),
            ),
        ] {
            pipeline
                .register(SourceAsset {
                    preset: Some(crate::CookPreset::Fast),
                    thread_budget: 1,
                    id: id.into(),
                    path: format!("{id}.png"),
                    importer: "ara.texture".into(),
                    version: "fixture".into(),
                    settings,
                    target: crate::TargetPlatform::Linux,
                    asset_dependencies: Default::default(),
                })
                .expect("source");
        }
        let mut policy = crate::ProjectImportSettings::default();
        policy.defaults.texture = Some(crate::TextureSettings {
            usage: crate::TextureUsage::Normal,
            ..Default::default()
        });
        assert!(
            pipeline
                .configure_project_settings(policy.clone())
                .expect("policy")
        );
        assert_eq!(pipeline.epochs.get("as_inherit"), Some(&1));
        assert_eq!(pipeline.epochs.get("as_explicit"), None);
        assert!(
            !pipeline
                .configure_project_settings(policy)
                .expect("same policy")
        );
        // Missing fixture media stops before dependency discovery or process launch.
        // Drain its owned snapshot thread before deleting the private test directory.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !pipeline.snapshots.is_empty() {
            pipeline.poll().expect("poll");
            assert!(
                std::time::Instant::now() < deadline,
                "snapshot failed to retire"
            );
            std::thread::yield_now();
        }
    }
    #[test]
    fn source_open_rejects_old_projects_and_accepts_explicit_authored_roots() {
        let directory = tempfile::tempdir().expect("source scratch");
        let root = directory.path();
        for marker in ["{}", r#"{"layoutVersion":3}"#] {
            std::fs::write(root.join("ara.project.json"), marker).expect("old marker");
            let diagnostic = authored_root(root)
                .expect_err("old layouts cannot become empty import inventories")
                .diagnostic("source.bin");
            assert_eq!(diagnostic.code, ara_diag::codes::PROJECT_MIGRATION_REQUIRED);
            assert!(!root.join(ara_doc::AUTHORING_ROOT).exists());
            assert!(!root.join(".ara").exists());
        }
        std::fs::write(root.join("ara.project.json"), r#"{"layoutVersion":4}"#)
            .expect("current marker");
        assert_eq!(
            authored_root(root).expect("current root"),
            root.join(ara_doc::AUTHORING_ROOT)
        );
        let explicit = root.join("prepared-sources");
        std::fs::create_dir(&explicit).expect("explicit authored root");
        assert_eq!(
            authored_root(&explicit).expect("caller-owned source boundary"),
            explicit
        );
    }
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
