//! Import settings are commands; native importers run only in disposable workers.
mod commands;
mod material_preview;
pub(crate) mod thumbnails;
mod vfx_preview;
pub(crate) mod vfx_stream;
use ara_asset::{
    CacheBackend, Cancellation, FileCache, ImportSettings, JobEvent, ProcessWorker, SourceAsset,
    TargetPlatform,
};
use ara_host::{Cx, Method, Service, parse};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use commands::register;

/// Install schemas before the host loads project documents.
pub(crate) fn prepare(project: &mut ara_host::Project) -> anyhow::Result<()> {
    ara_asset::register_schema(&mut project.db);
    ara_importers::registry()?.register_schemas(&mut project.db);
    register(&mut project.registry, &mut project.db);
    ara_graph::register(&mut project.db).map_err(|d| anyhow::anyhow!(d.to_string()))?;
    ara_graph::commands::register(&mut project.registry, &mut project.db);
    // Material graph commands use graph records. The separate material-instance
    // schema currently shares `i_` with importer sidecars and cannot coexist yet.
    ara_material::commands::register(&mut project.registry, &mut project.db);
    ara_vfx::commands::register(&mut project.registry, &mut project.db);
    ara_anim::document::register(&mut project.db)?;
    let mut files = Vec::new();
    ara_host::project::walk_documents(&project.root, &project.db, &mut files);
    import_files(&project.root, &mut files)?;
    let editor_layouts = project.root.join(".ara/editor");
    if editor_layouts.is_dir() {
        ara_host::project::walk_documents(&editor_layouts, &project.db, &mut files);
    }
    files.sort();
    files.dedup();
    for file in files {
        if !project.docs.contains(&file) {
            if let Ok(text) = std::fs::read_to_string(&file)
                && let Some((kind, id)) = ara_doc::read_header(&text, &project.db)
            {
                project.index.insert(
                    id.to_string(),
                    if kind == "import" {
                        project
                            .rel(&file)
                            .trim_end_matches(".import.json")
                            .to_owned()
                    } else {
                        project.rel(&file)
                    },
                    Some(kind),
                );
            }
            project.docs.push(file);
        }
    }
    Ok(())
}
fn import_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules" | "spikes") {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            import_files(&entry.path(), out)?;
        } else if name.ends_with(".import.json") {
            out.push(entry.path());
        }
    }
    Ok(())
}

#[derive(Schema, Deserialize)]
#[ara(params, name = "EditorAssetParams", module = "editor")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Typed importer authoring request.
struct AssetParams {
    /// Calling editor or agent session.
    session: String,
    /// Source media path, relative to the project.
    path: String,
}
enum Event {
    Progress(f32, String),
    Complete(Result<Value, String>),
}
struct Job {
    rx: Receiver<Event>,
    cancel: Cancellation,
}
/// Authority-side import job service. IO, hashing and native code never run in tick.
pub(crate) struct AssetService {
    jobs: BTreeMap<String, Job>,
}
impl AssetService {
    pub fn new() -> Self {
        Self {
            jobs: BTreeMap::new(),
        }
    }
}
impl Service for AssetService {
    fn name(&self) -> &'static str {
        "editor-assets"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<AssetParams>(
                db,
                "editor.asset.reimport",
                "Reimport source media with the current undoable import settings.",
            )
            .action("ara_session", "editor.asset.reimport"),
            Method::new::<AssetParams>(
                db,
                "editor.asset.import",
                "Create typed import settings and start an isolated source cook.",
            )
            .action("ara_session", "editor.asset.import"),
        ]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<ara_diag::Diagnostic>> {
        let p: AssetParams = parse(params)?;
        let _ = &p.session;
        let relative = Path::new(&p.path);
        if p.path.contains(':')
            || p.path.contains('\\')
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(vec![crate::invalid("source path must stay in the project")]);
        }
        let sidecar = format!("{}.import.json", p.path);
        if method == "editor.asset.import"
            && !cx
                .store()
                .workspace()
                .docs()
                .any(|(path, _)| path == sidecar)
        {
            let registry =
                ara_importers::registry().map_err(|e| vec![crate::invalid(e.to_string())])?;
            let extension = relative.extension().and_then(|s| s.to_str()).unwrap_or("");
            let importer = registry
                .get(match extension {
                    "glb" | "gltf" => "ara.gltf",
                    "fbx" => "ara.fbx",
                    _ => "ara.texture",
                })
                .map_err(|e| vec![crate::invalid(e.to_string())])?;
            let settings = ImportSettings {
                importer: importer.id().into(),
                version: importer.version().into(),
                preset: ara_asset::CookPreset::Fast,
                settings: json!({}),
                dependencies: Vec::new(),
            };
            let doc = cx
                .create_document(&sidecar, "import")
                .map_err(|d| vec![d])?;
            let batch = parse(
                json!({"session":p.session,"doc":doc,"why":"Import source media","commands":[{"cmd":"editor.import.initialize","args":{"settings":settings}}]}),
            )?;
            cx.apply(batch)?;
        }
        let doc = cx
            .store()
            .workspace()
            .docs()
            .find_map(|(path, doc)| (path == sidecar).then_some(doc));
        let settings = doc
            .and_then(|d| d.record("i_settings"))
            .map(|r| serde_json::from_value::<ImportSettings>(Value::Object(r.clone())))
            .transpose()
            .map_err(|e| vec![crate::invalid(e.to_string())])?;
        let settings = settings.ok_or_else(|| {
            vec![crate::invalid(
                "Import settings are missing. Create an import settings document first.",
            )]
        })?;
        let id = doc
            .map(|d| d.id.to_string())
            .ok_or_else(|| vec![crate::invalid("missing import document")])?;
        let source = SourceAsset {
            id,
            path: p.path.clone(),
            importer: settings.importer,
            version: settings.version,
            settings: settings.settings,
            preset: settings.preset,
            thread_budget: 2,
            target: TargetPlatform::Windows,
            asset_dependencies: BTreeMap::new(),
        };
        let root = cx.root().to_owned();
        let job = cx.start_job(&format!("Import {}", p.path));
        let (tx, rx) = mpsc::channel();
        let cancel = Cancellation::default();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Value> {
                let worker = ProcessWorker::new(std::env::current_exe()?)
                    .with_arguments(["--import-worker"]);
                let input = ara_asset::snapshot(&root, &source, &worker, &worker_cancel)?;
                let key = input.key()?;
                let cache = FileCache::new(&root)?;
                let output = if let Some(hit) = cache.load(key)? {
                    hit.output
                } else {
                    let mut cook = worker.start(input)?;
                    loop {
                        if worker_cancel.is_cancelled() {
                            cook.cancel();
                            anyhow::bail!("import cancelled");
                        }
                        match cook.poll()? {
                            Some(JobEvent::Complete(out)) => break out,
                            Some(JobEvent::Progress(p)) => {
                                let _ = tx.send(Event::Progress(p.fraction, p.message));
                            }
                            _ => std::thread::sleep(Duration::from_millis(4)),
                        }
                    }
                };
                cache.store(key, &output)?;
                Ok(
                    json!({"source":source.path,"key":key,"artifacts":output.artifacts.iter().map(|a|json!({"name":a.name,"bytes":a.bytes.len()})).collect::<Vec<_>>()}),
                )
            })();
            let _ = tx.send(Event::Complete(result.map_err(|e| e.to_string())));
        });
        self.jobs.insert(job.clone(), Job { rx, cancel });
        Ok(json!({"job":job}))
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        self.jobs.retain(|id, job| {
            if cx.job_cancelled(id) {
                job.cancel.cancel();
                return false;
            }
            for event in job.rx.try_iter().take(32) {
                match event {
                    Event::Progress(f, m) => cx.job_progress(id, f, &m),
                    Event::Complete(result) => {
                        cx.finish_job(id, result.map_err(|e| vec![crate::invalid(e)]));
                        return false;
                    }
                }
            }
            true
        });
    }
}
impl Drop for AssetService {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.cancel();
        }
    }
}
