//! The same executable is a real protocol worker or the cold-batch test driver.
use ara_asset::*;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    path::Path,
    thread,
    time::{Duration, Instant},
};

struct Fixture;
impl Importer for Fixture {
    fn id(&self) -> &str {
        "fixture"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn import(&self, input: &ImportInput, _: &mut ImportContext<'_>) -> Result<ImportOutput> {
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "fixture".into(),
                kind: ArtifactKind::Custom,
                bytes: input.source.clone(),
            }],
        })
    }
}

fn active(root: &Path) -> std::io::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(root)? {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry?.path())?;
        match file.try_lock() {
            Err(TryLockError::WouldBlock) => count += 1,
            Err(TryLockError::Error(error)) => return Err(error),
            Ok(()) => {}
        }
    }
    Ok(count)
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--worker-fixture")) {
        let root = std::env::args_os()
            .nth(2)
            .ok_or("worker fixture directory")?;
        // An OS lock counts live child processes and is released even on kill.
        let marker = File::create(Path::new(&root).join(format!("{}.lock", std::process::id())))?;
        marker.lock()?;
        thread::sleep(Duration::from_millis(200));
        let mut registry = ImporterRegistry::default();
        registry.register(Fixture)?;
        serve_worker(&registry)?;
        return Ok(());
    }
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-asset/worker-limit");
    fs::create_dir_all(&output)?;
    let temporary = tempfile::tempdir_in(output)?;
    let cap = std::env::var("ARA_IMPORT_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(4)
        .min(thread::available_parallelism().map_or(1, usize::from));
    let count = cap + 24;
    let mut jobs = Vec::new();
    for index in 0..count {
        // Separate handles (as in snapshot/cook callers) share one admission cap.
        let worker = ProcessWorker::new(std::env::current_exe()?).with_arguments([
            std::ffi::OsStr::new("--worker-fixture"),
            temporary.path().as_os_str(),
        ]);
        let job = worker.start(ImportInput {
            preset: CookPreset::Fast,
            thread_budget: 1,
            source_path: format!("cold-{index}.bin"),
            source: index.to_le_bytes().to_vec(),
            importer: "fixture".into(),
            version: "1".into(),
            settings: serde_json::json!({}),
            target: TargetPlatform::Windows,
            dependencies: BTreeMap::new(),
            asset_dependencies: BTreeMap::new(),
        })?;
        jobs.push(Some(job));
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut completed = 0;
    let mut peak = 0;
    while completed < count {
        assert!(Instant::now() < deadline, "every queued import must run");
        peak = peak.max(active(temporary.path())?);
        assert!(peak <= cap, "cold batch ran {peak} workers; cap is {cap}");
        for (index, slot) in jobs.iter_mut().enumerate() {
            if let Some(job) = slot
                && let Some(JobEvent::Complete(result)) = job.poll()?
            {
                assert_eq!(result.artifacts[0].bytes, index.to_le_bytes());
                *slot = None;
                completed += 1;
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(peak > 0, "the test must observe actual native workers");
    let worker = ProcessWorker::new(std::env::current_exe()?).with_arguments([
        std::ffi::OsStr::new("--worker-fixture"),
        temporary.path().as_os_str(),
    ]);
    assert_eq!(worker.versions()?.get("fixture"), Some(&"1".to_owned()));
    assert!(
        worker
            .discover("fixture", "1", vec![], &Cancellation::default())?
            .is_empty()
    );
    println!("{completed} cold imports completed; peak {peak}, cap {cap}");
    Ok(())
}
