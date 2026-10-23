//! Measures real worker preview/final publication, including IPC and cache writes.
use ara_asset::{
    CacheBackend, CookPreset, FileCache, HotReload, ImportInput, Importer, ProcessWorker,
    ReloadEvent, TargetPlatform,
};
use ara_importers::GltfImporter;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::args().nth(1).ok_or("expected GLB path")?);
    let executable = PathBuf::from(
        std::env::args()
            .nth(2)
            .ok_or("expected release worker executable")?,
    );
    let input = ImportInput {
        preset: CookPreset::HighQuality,
        thread_budget: 16,
        source_path: path
            .file_name()
            .ok_or("source name")?
            .to_string_lossy()
            .into_owned(),
        source: std::fs::read(&path)?,
        importer: GltfImporter.id().into(),
        version: GltfImporter.version().into(),
        settings: serde_json::json!({}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    };
    let final_key = input.key()?;
    let mut preview = input.clone();
    preview.preset = CookPreset::Fast;
    let fast_key = preview.key()?;
    let dir = tempfile::tempdir()?;
    let cache = Arc::new(FileCache::new(dir.path())?);
    let mut reload = HotReload::new(ProcessWorker::new(executable), cache.clone());
    let start = Instant::now();
    reload.request("as_bench", input);
    let mut fast_ms = None;
    let mut final_ms = None;
    while final_ms.is_none() {
        if start.elapsed() > Duration::from_secs(120) {
            return Err("benchmark deadline".into());
        }
        for event in reload.poll() {
            match event {
                ReloadEvent::Ready { final_pass, .. } => {
                    let time = start.elapsed().as_secs_f64() * 1000.0;
                    if final_pass {
                        final_ms = Some(time);
                    } else {
                        fast_ms = Some(time);
                    }
                }
                ReloadEvent::Failed { error, .. } => return Err(error.into()),
                _ => {}
            }
        }
        reload.apply_boundary(true, false, &BTreeSet::new());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(cache.load(fast_key)?.is_some());
    assert!(cache.load(final_key)?.is_some());
    assert_eq!(
        reload
            .slot("as_bench")
            .and_then(|s| s.current.as_ref())
            .map(|c| c.key),
        Some(final_key)
    );
    println!(
        "{}",
        serde_json::json!({"fixture":path.file_name().map(|s|s.to_string_lossy()),"previewReadyMs":fast_ms,"finalReadyMs":final_ms})
    );
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
