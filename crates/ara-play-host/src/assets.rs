//! Project assets for play: every `*.import.json` sidecar is registered with an
//! `ara_asset::AssetPipeline` (importers run in disposable worker processes; D20).
//! Play pins the published generation of each asset (`as_…` → action digest); players
//! load those from the shared content-addressed cache and never import.
//! Generations published during Play reach players only when allowed or approved.
//! A sidecar recorded by an older importer is upgraded (one typed
//! `SidecarCommand::Version` transaction, an info diagnostic) and reimported; only an
//! importer older than the sidecar's, or a failed import, is an error.
use ara_asset::{
    AssetChanged, AssetPipeline, FileCache, ProcessWorker, ReloadEvent, Sidecar, SidecarCommand,
    SourceAsset, TargetPlatform, VersionCheck,
};
use ara_diag::Diagnostic;
use ara_schema::SchemaDb;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// The owner's import pipeline and the latest published generation per asset.
pub struct Assets {
    pipeline: AssetPipeline,
    sources: BTreeSet<String>,
    failures: BTreeMap<String, String>,
    notes: Vec<Diagnostic>,
    /// Cache directory shared with players.
    pub cache: PathBuf,
}

fn sidecars(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !(name.starts_with('.') || name == "target" || name == "node_modules")
                && !(path != root && path.join("ara.project.json").exists())
            {
                sidecars(root, &path, out);
            }
        } else if name.ends_with(".import.json") {
            out.push(path);
        }
    }
}

impl Assets {
    /// Registers every sidecar under `root` and requests imports in the background.
    /// `worker` is the import worker executable (`ara-import-worker`).
    pub fn new(root: &Path, worker: &Path) -> Result<Self, String> {
        let mut schema = ara_schema::SchemaDb::new();
        ara_doc::register(&mut schema);
        ara_asset::register_schema(&mut schema);
        let cache = FileCache::new(root).map_err(|e| e.to_string())?;
        let cache_dir = cache.root().to_path_buf();
        let mut pipeline = AssetPipeline::new(
            root,
            ProcessWorker::new(worker),
            Arc::new(cache),
            Arc::new(schema.clone()),
        )
        .map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        sidecars(root, root, &mut files);
        files.sort();
        // An older worker without the query keeps every recorded version.
        let installed = ProcessWorker::new(worker).versions().unwrap_or_default();
        let mut sources = BTreeSet::new();
        let mut failures = BTreeMap::new();
        let mut notes = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            let sidecar = Sidecar::read(&text, &file.to_string_lossy(), &schema)
                .map_err(|e| e.to_string())?;
            let mut settings = sidecar.settings().map_err(|e| e.to_string())?;
            let rel = file
                .strip_prefix(root)
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
            pipeline.request(&id).map_err(|e| e.to_string())?;
            sources.insert(id);
        }
        Ok(Self {
            pipeline,
            sources,
            failures,
            notes,
            cache: cache_dir,
        })
    }

    /// Info diagnostics (importer upgrades) not yet reported.
    pub fn take_notes(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.notes)
    }

    /// Drains import events and publishes ready generations. `playing` blocks
    /// publication unless `allow` (the human's policy for agent/asset changes in Play).
    pub fn poll(&mut self, playing: bool, allow: bool) -> Vec<AssetChanged> {
        match self.pipeline.poll() {
            Ok(events) => {
                for event in events {
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
        self.pipeline
            .apply_boundary(true, playing, &BTreeSet::new())
            .unwrap_or_default()
    }

    /// The published generation of every source (hex digest), when all of `wanted` are
    /// published; waits up to `timeout` (first cooks of a fresh clone take seconds).
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
                .filter(|id| self.sources.contains(*id) && self.digest(id).is_none())
                .collect();
            if missing.is_empty() {
                break;
            }
            if let Some((asset, error)) = self.failures.iter().find(|(a, _)| wanted.contains(*a)) {
                return Err(format!("import of {asset} failed: {error}"));
            }
            if start.elapsed() > timeout {
                return Err(format!(
                    "assets still importing after {timeout:?}: {missing:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(self
            .sources
            .iter()
            .filter_map(|id| Some((id.clone(), self.digest(id)?)))
            .collect())
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
