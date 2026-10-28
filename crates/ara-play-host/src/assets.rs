//! Project assets for play: every `*.import.json` sidecar is registered with an
//! `ara_asset::AssetPipeline` (importers run in disposable worker processes; D20).
//! Play pins the published generation of each asset (`as_…` → action digest); players
//! load those from the shared content-addressed cache and never import. Imports run
//! non-progressively, so every published generation is already persisted and is the
//! requested preset (never a preview that a later pass replaces).
//! Generations published during Play reach players only when allowed or approved.
//! A sidecar recorded by an older importer is upgraded (one typed
//! `SidecarCommand::Version` transaction, an info diagnostic) and reimported; only an
//! importer older than the sidecar's, or a failed import, produces a diagnostic and
//! keeps the last good generation (a placeholder when none exists).
use ara_asset::{
    AssetChanged, AssetPipeline, FileCache, ImportSettings, ProcessWorker, ReloadEvent, Sidecar,
    SidecarCommand, SourceAsset, TargetPlatform, VersionCheck,
};
use ara_diag::Diagnostic;
use ara_schema::SchemaDb;
mod progress;
pub use progress::CookProgress;
use progress::CookWatch;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// An import document as [`Assets::synchronize_imports`] reads it: the sidecar path,
/// the asset ID and the settings record.
pub type ImportDoc = (String, String, ara_doc::Record);

/// The import documents of `workspace`, collected on the host thread so a pipeline
/// built elsewhere can apply them ([`Assets::synchronize_imports`]).
pub fn import_docs(workspace: &ara_doc::Workspace) -> Result<Vec<ImportDoc>, String> {
    workspace
        .docs()
        .filter(|(_, doc)| {
            doc.kind == "import"
                || (doc.kind == "imports" && workspace.is_project_document(doc.id.as_str()))
        })
        .map(|(path, doc)| {
            let record = doc
                .record(if doc.kind == "imports" {
                    "pi_settings"
                } else {
                    "i_settings"
                })
                .ok_or_else(|| format!("{path}: missing import settings"))?;
            Ok((
                if doc.kind == "import" {
                    workspace.reference_path(path)
                } else {
                    path
                }
                .to_owned(),
                doc.id.to_string(),
                record.clone(),
            ))
        })
        .collect()
}

fn project_preset(imports: &[ImportDoc]) -> Result<ara_asset::CookPreset, String> {
    imports
        .iter()
        .find(|(path, _, _)| path == ara_asset::PROJECT_IMPORT_SETTINGS_FILE)
        .or_else(|| {
            imports
                .iter()
                .find(|(path, _, _)| path == ara_asset::LEGACY_PROJECT_IMPORT_SETTINGS_FILE)
        })
        .map(|(_, _, record)| {
            serde_json::from_value::<ara_asset::ProjectImportSettings>(serde_json::Value::Object(
                record.clone(),
            ))
            .map(|settings| settings.preset)
        })
        .transpose()
        .map_err(|error| error.to_string())
        .map(Option::unwrap_or_default)
}

/// The owner's import pipeline and the latest published generation per asset.
pub struct Assets {
    pipeline: AssetPipeline,
    sources: BTreeSet<String>,
    failures: BTreeMap<String, String>,
    notes: Vec<Diagnostic>,
    installed: BTreeMap<String, String>,
    configurations: BTreeMap<String, (String, ara_doc::Record)>,
    revision: Option<u64>,
    published: crate::published::Publisher,
    /// Sources were registered or removed since progress was last published.
    progress_stale: bool,
    /// The pipeline was importing at the last poll.
    busy: bool,
    /// Cache directory shared with players.
    pub cache: PathBuf,
}

impl Assets {
    pub(crate) fn import_revision(&self) -> u64 {
        self.published.import_revision()
    }
    pub(crate) fn import_state(&self) -> ara_play::ImportState {
        self.published.import_state()
    }
    /// Snapshot completed generations immediately. A live Play world can start
    /// with placeholders while this same owner continues importing/publishing.
    pub fn ready_pins(&mut self, wanted: &BTreeSet<String>) -> BTreeMap<String, String> {
        self.poll(false, true);
        self.finished_pins(wanted)
    }
    /// Waits while requested cooks make progress, without imposing a total export
    /// deadline. Completed generations reset `stall_timeout`; queue/admission and
    /// importer progress messages do not. Reports changes and five-second heartbeats.
    pub fn pins_with_progress(
        &mut self,
        wanted: &BTreeSet<String>,
        stall_timeout: Duration,
        mut report: impl FnMut(CookProgress) -> Result<(), String>,
    ) -> Result<BTreeMap<String, String>, String> {
        let start = Instant::now();
        let mut watch = CookWatch::new();
        let mut last_report = None;
        loop {
            self.poll(false, true);
            let requested: Vec<_> = wanted
                .iter()
                .filter(|id| self.sources.contains(*id))
                .collect();
            let pending: Vec<_> = requested
                .iter()
                .copied()
                .filter(|id| {
                    !self.failures.contains_key(*id)
                        && (self.digest(id).is_none() || self.pipeline.is_importing_asset(id))
                })
                .collect();
            let completed = requested
                .iter()
                .filter(|id| self.digest(id).is_some() && !self.pipeline.is_importing_asset(id))
                .count();
            let state = watch.observe(completed, requested.len(), start.elapsed());
            if last_report.is_none_or(|(count, elapsed)| {
                count != completed
                    || state.elapsed.saturating_sub(elapsed) >= Duration::from_secs(5)
            }) {
                report(state)?;
                last_report = Some((completed, state.elapsed));
            }
            if pending.is_empty() {
                return Ok(self.finished_pins(wanted));
            }
            if state.stalled >= stall_timeout {
                return Err(format!(
                    "ARA-EXP-COOK-001: no requested cook completed for {stall_timeout:?} ({completed}/{} complete, {:?} total); pending assets: {pending:?}",
                    requested.len(),
                    state.elapsed,
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Registers every sidecar under `root` and requests imports in the background.
    /// `worker` is the import worker executable (`ara-import-worker`).
    pub fn new(root: &Path, worker: &Path) -> Result<Self, String> {
        let mut schema = ara_schema::SchemaDb::new();
        ara_doc::register(&mut schema);
        ara_asset::register_schema(&mut schema);
        let cache = FileCache::new(root).map_err(|e| e.to_string())?;
        let cache_dir = cache.root().to_path_buf();
        let published = crate::published::Publisher::new(root, cache.clone())?;
        let worker = ProcessWorker::new(worker).with_memo(Some(&cache_dir));
        let mut pipeline = AssetPipeline::new(
            root,
            worker.clone(),
            Arc::new(cache),
            Arc::new(schema.clone()),
        )
        .map_err(|e| e.to_string())?;
        // Players load generations from the disk cache in another process, so only a
        // persisted generation is playable. Progressive previews are published before
        // they are persisted (an in-process speedup), and pinning one would also make
        // Play depend on import timing; play cooks the requested preset directly.
        pipeline.reload.progressive = false;
        let authored_root = ara_host::project::authored_root(root);
        let files = ara_host::project::authored_files(root, &schema)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|file| {
                file.scope == ara_host::project::AuthoredScope::Authored
                    && file.role == ara_host::project::AuthoredRole::ImportSidecar
            })
            .map(|file| root.join(file.storage_path))
            .collect::<Vec<_>>();
        // An older worker without the query keeps every recorded version.
        let installed = worker.versions_with_encoder_hint().unwrap_or_default();
        let mut sources = BTreeSet::new();
        let mut failures = BTreeMap::new();
        let mut notes = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            let sidecar = Sidecar::read(&text, &file.to_string_lossy(), &schema)
                .map_err(|e| e.to_string())?;
            let mut settings = sidecar.settings().map_err(|e| e.to_string())?;
            let rel = file
                .strip_prefix(&authored_root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let id = sidecar.document().id.to_string();
            if let Some(version) = installed.get(&settings.importer) {
                match settings.check_version(version) {
                    VersionCheck::Current => {}
                    VersionCheck::Upgrade => {
                        upgrade(&file, sidecar, version, &schema)?;
                        notes.push(Diagnostic::info(
                            crate::codes::ASSET_UPGRADED,
                            format!(
                                "{rel} was imported with {} {}; reimporting with {version} and recording it",
                                settings.importer, settings.version
                            ),
                        ));
                        settings.version = version.clone();
                    }
                    VersionCheck::Newer => {
                        failures.insert(
                            id.clone(),
                            format!(
                                "{rel} needs {} {}, newer than this engine's {version}; update the engine",
                                settings.importer, settings.version
                            ),
                        );
                        sources.insert(id);
                        continue;
                    }
                }
            }
            pipeline
                .register(SourceAsset {
                    preset: settings.preset,
                    thread_budget: 4,
                    id: id.clone(),
                    path: rel.trim_end_matches(".import.json").to_owned(),
                    importer: settings.importer,
                    version: settings.version,
                    settings: settings.settings,
                    target: if cfg!(target_os = "macos") {
                        TargetPlatform::MacOs
                    } else if cfg!(target_os = "linux") {
                        TargetPlatform::Linux
                    } else {
                        TargetPlatform::Windows
                    },
                    asset_dependencies: BTreeMap::new(),
                })
                .map_err(|e| e.to_string())?;
            if let Err(error) = pipeline.request(&id) {
                failures.insert(id.clone(), error.to_string());
            }
            sources.insert(id);
        }
        let mut assets = Self {
            pipeline,
            sources,
            failures,
            notes,
            installed,
            configurations: BTreeMap::new(),
            revision: None,
            published,
            progress_stale: false,
            busy: true,
            cache: cache_dir,
        };
        // Views show every requested source as pending from the start.
        assets.published.progress(assets.progress());
        Ok(assets)
    }

    /// Synchronizes command-authored sidecars before Play or hot reload. In-memory
    /// settings are authoritative even before the host's canonical debounced save.
    pub fn synchronize(
        &mut self,
        workspace: &ara_doc::Workspace,
        revision: u64,
    ) -> Result<(), String> {
        if self.revision == Some(revision) {
            return Ok(());
        }
        self.synchronize_imports(import_docs(workspace)?, revision)
    }

    /// [`Self::synchronize`] from import documents collected at `revision` (the
    /// pipeline's build thread applies the first set).
    pub fn synchronize_imports(
        &mut self,
        imports: Vec<ImportDoc>,
        revision: u64,
    ) -> Result<(), String> {
        let project_preset = project_preset(&imports)?;
        let project_changed = self
            .pipeline
            .configure_project(project_preset)
            .map_err(|error| error.to_string())?;
        self.progress_stale |= project_changed;
        let mut current = BTreeMap::new();
        for (path, id, record) in imports {
            if path == ara_asset::PROJECT_IMPORT_SETTINGS_FILE
                || path == ara_asset::LEGACY_PROJECT_IMPORT_SETTINGS_FILE
            {
                continue;
            }
            let source = path
                .strip_suffix(".import.json")
                .ok_or_else(|| format!("{path}: import document must name a sidecar"))?;
            let configuration = (source.to_owned(), record);
            let inherits_changed_project = project_changed
                && !configuration.1.contains_key("preset")
                && configuration
                    .1
                    .get("importer")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| matches!(id, "ara.texture" | "ara.gltf" | "ara.fbx"));
            if self.configurations.get(&id) != Some(&configuration) || inherits_changed_project {
                let mut settings: ImportSettings =
                    serde_json::from_value(serde_json::Value::Object(configuration.1.clone()))
                        .map_err(|e| format!("{path}: {e}"))?;
                self.sources.insert(id.clone());
                if let Some(installed) = self.installed.get(&settings.importer) {
                    match settings.check_version(installed) {
                        VersionCheck::Current => {}
                        VersionCheck::Upgrade => settings.version = installed.clone(),
                        VersionCheck::Newer => {
                            self.pipeline.unregister(&id);
                            self.failures.insert(
                                id.clone(),
                                format!(
                                    "{path} needs {} {}, newer than this engine's {installed}",
                                    settings.importer, settings.version
                                ),
                            );
                            current.insert(id, configuration);
                            continue;
                        }
                    }
                }
                self.pipeline
                    .configure(
                        SourceAsset {
                            id: id.clone(),
                            path: source.to_owned(),
                            preset: settings.preset,
                            thread_budget: 4,
                            importer: settings.importer,
                            version: settings.version,
                            settings: settings.settings,
                            target: if cfg!(target_os = "macos") {
                                TargetPlatform::MacOs
                            } else if cfg!(target_os = "linux") {
                                TargetPlatform::Linux
                            } else {
                                TargetPlatform::Windows
                            },
                            asset_dependencies: BTreeMap::new(),
                        },
                        settings.dependencies,
                    )
                    .map_err(|e| e.to_string())?;
                self.failures.remove(&id);
                if let Err(error) = self.pipeline.request(&id) {
                    self.failures.insert(id.clone(), error.to_string());
                }
                self.progress_stale = true;
            }
            current.insert(id, configuration);
        }
        for id in self
            .configurations
            .keys()
            .filter(|id| !current.contains_key(*id))
        {
            self.pipeline.unregister(id);
            self.sources.remove(id);
            self.failures.remove(id);
            self.progress_stale = true;
        }
        self.configurations = current;
        self.published.retain(&self.sources);
        self.revision = Some(revision);
        Ok(())
    }

    /// Importer upgrades and failed-import placeholder diagnostics not yet reported.
    pub fn take_notes(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.notes)
    }

    /// Drains import events and publishes ready generations. `playing` blocks
    /// publication unless `allow` (the human's policy for agent/asset changes in Play).
    /// Import progress is republished only after activity that can move a source
    /// between pending, done and failed (not on worker progress messages or idle polls).
    pub fn poll(&mut self, playing: bool, allow: bool) -> Vec<AssetChanged> {
        let mut settled = false;
        match self.pipeline.poll() {
            Ok(events) => {
                for event in events {
                    settled |= !matches!(event, ReloadEvent::Progress { .. });
                    if let ReloadEvent::Failed { asset, error } = event {
                        self.failures.insert(asset, error.to_string());
                    }
                }
            }
            Err(e) => {
                self.failures.insert("pipeline".into(), e.to_string());
            }
        }
        self.pipeline.reload.allow_during_play = allow;
        let changes = self
            .pipeline
            .apply_boundary(true, playing, &BTreeSet::new())
            .unwrap_or_default();
        // A newly published generation means that asset's latest import succeeded.
        for change in &changes {
            self.failures.remove(&change.asset);
        }
        self.published.complete(&changes, &self.pipeline.reload);
        let busy = self.pipeline.is_importing();
        if self.progress_stale || settled || !changes.is_empty() || busy != self.busy {
            self.published.progress(self.progress());
            self.progress_stale = false;
        }
        self.busy = busy;
        // Deferred writes and a feed taken over from a dropped owner publish now.
        self.published.tick();
        changes
    }

    /// Where each registered source stands ([`crate::ImportProgress`]).
    fn progress(&self) -> crate::ImportProgress {
        let mut progress = crate::ImportProgress {
            total: self.sources.len(),
            ..Default::default()
        };
        for id in &self.sources {
            if self.pipeline.is_importing_asset(id) {
                progress.pending.insert(id.clone());
            } else if let Some(error) = self.failures.get(id) {
                progress.failed.insert(id.clone(), error.clone());
            } else if self
                .pipeline
                .reload
                .slot(id)
                .is_some_and(|slot| slot.current.is_some())
            {
                progress.done += 1;
            } else {
                progress.pending.insert(id.clone());
            }
        }
        progress
    }

    /// Published source generations (hex digests), waiting up to `timeout` for
    /// `wanted`. Failed imports keep a last-good generation or a diagnosed placeholder.
    pub fn pins(
        &mut self,
        wanted: &BTreeSet<String>,
        timeout: Duration,
    ) -> Result<BTreeMap<String, String>, String> {
        let start = Instant::now();
        loop {
            self.poll(false, true);
            let missing: Vec<&String> = wanted
                .iter()
                .filter(|id| {
                    self.sources.contains(*id)
                        && !self.failures.contains_key(*id)
                        && (self.digest(id).is_none() || self.pipeline.is_importing_asset(id))
                })
                .collect();
            if missing.is_empty() {
                break;
            }
            if start.elapsed() > timeout {
                return Err(format!(
                    "assets still importing after {timeout:?}: {missing:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(self.finished_pins(wanted))
    }

    fn finished_pins(&mut self, wanted: &BTreeSet<String>) -> BTreeMap<String, String> {
        for (asset, error) in self
            .failures
            .iter()
            .filter(|(asset, _)| wanted.contains(*asset))
        {
            self.notes.push(Diagnostic::warning(
                crate::codes::ASSETS,
                format!(
                    "import of {asset} failed: {error}; Play will use a placeholder where no previous generation exists"
                ),
            ));
        }
        self.sources
            .iter()
            .filter_map(|id| Some((id.clone(), self.digest(id)?)))
            .collect()
    }

    /// The live generation digest of `asset` as hex.
    pub fn digest(&self, asset: &str) -> Option<String> {
        let slot = self.pipeline.reload.slot(asset)?;
        slot.current.as_ref().map(|c| ara_play::hex(&c.key.0))
    }

    /// Whether imports are still running.
    pub fn importing(&self) -> bool {
        self.pipeline.is_importing()
    }
}

/// Records the installed importer `version` in the sidecar at `file`: one typed
/// `SidecarCommand::Version` transaction, persisted through the canonical writer.
fn upgrade(file: &Path, sidecar: Sidecar, version: &str, schema: &SchemaDb) -> Result<(), String> {
    let id = sidecar.document().id.clone();
    let mut workspace = ara_doc::Workspace::new(ara_doc::AssetIndex::new());
    workspace.insert(file.to_string_lossy(), sidecar.into_document());
    let mut store = ara_ops::Store::new(schema.clone(), workspace);
    let mut meta = ara_ops::TxnMeta::by(ara_ops::Author::Agent("ara.assets".into()));
    meta.why = Some(format!("Importer upgraded to {version}"));
    let edit = SidecarCommand::Version(version.to_owned())
        .edit(id.clone())
        .map_err(|e| e.to_string())?;
    store.transact(meta, vec![edit]).map_err(|d| {
        d.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let doc = store
        .workspace()
        .doc(id.as_str())
        .ok_or_else(|| format!("{} vanished during the upgrade", file.display()))?;
    std::fs::write(file, ara_doc::write(doc, store.schema())).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn scoped_import_policy_takes_precedence_without_changing_the_legacy_default() {
        let record = |preset| {
            serde_json::json!({"preset":preset})
                .as_object()
                .expect("record")
                .clone()
        };
        let imports = vec![
            (
                ara_asset::LEGACY_PROJECT_IMPORT_SETTINGS_FILE.into(),
                "ip_legacy".into(),
                record("highQuality"),
            ),
            (
                ara_asset::PROJECT_IMPORT_SETTINGS_FILE.into(),
                "ip_scoped".into(),
                record("balanced"),
            ),
        ];
        assert_eq!(
            super::project_preset(&imports).expect("scoped policy"),
            ara_asset::CookPreset::Balanced
        );
        assert_eq!(
            super::project_preset(&imports[..1]).expect("legacy policy"),
            ara_asset::CookPreset::HighQuality
        );
    }

    #[test]
    fn authored_imports_documents_do_not_become_global_project_policy() {
        let mut workspace = ara_doc::Workspace::default();
        workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        for (id, path, project) in [
            ("ip_project", "ara.imports".to_owned(), true),
            (
                "ip_feature",
                format!("{}/settings/ara.imports", ara_doc::AUTHORING_ROOT),
                false,
            ),
        ] {
            let mut document =
                ara_doc::Document::new("imports", ara_doc::AssetId::parse(id).expect("policy ID"));
            document.insert_unchecked(
                ara_doc::RecordId::parse("pi_settings").expect("record ID"),
                serde_json::json!({"preset":"balanced"})
                    .as_object()
                    .expect("record")
                    .clone(),
            );
            if project {
                workspace.insert_project_document(path, document);
            } else {
                workspace.insert(path, document);
            }
        }
        let imports = super::import_docs(&workspace).expect("project imports");
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].0, "ara.imports");
    }

    #[test]
    fn project_quality_is_loaded_as_a_host_document_before_import_synchronization() {
        let root = tempfile::tempdir().expect("project");
        std::fs::write(
            root.path().join("ara.project.json"),
            ara_json::to_canonical_string(
                &serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
            ),
        )
        .expect("marker");
        std::fs::create_dir_all(root.path().join("settings")).expect("settings");
        let policy = ara_asset::ProjectImportSettings {
            preset: ara_asset::CookPreset::HighQuality,
        };
        std::fs::write(
            root.path().join(ara_asset::PROJECT_IMPORT_SETTINGS_FILE),
            policy.canonical_text().expect("canonical"),
        )
        .expect("policy");
        let project = ara_host::Project::open_with(root.path(), |db, _| {
            ara_asset::register_schema(db);
            Vec::new()
        })
        .expect("open");
        let (workspace, diagnostics) = project.load(&[]);
        assert!(
            diagnostics.values().flatten().all(|d| !d.is_error()),
            "{diagnostics:?}"
        );
        let documents = super::import_docs(&workspace).expect("import documents");
        let (path, _, record) = documents
            .iter()
            .find(|(path, _, _)| path == ara_asset::PROJECT_IMPORT_SETTINGS_FILE)
            .expect("project policy is loaded");
        assert_eq!(path, ara_asset::PROJECT_IMPORT_SETTINGS_FILE);
        assert_eq!(record["preset"], "highQuality");
    }
}
