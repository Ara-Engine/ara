//! The same executable is a real protocol worker or the cold-batch test driver.
use ara_asset::*;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    path::Path,
    process::Command,
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

// Keep the completed import completely unpolled while synchronous discovery
// needs the same admission slot. The parent watchdog bounds a deadlock failure.
fn unpolled_admission() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Exceed the pipe buffer so completion also requires the background reader
    // to drain the reply while the ImportJob remains completely unpolled.
    const SOURCE_SIZE: usize = 128 * 1024;
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-asset/worker-limit");
    fs::create_dir_all(&output)?;
    let temporary = tempfile::tempdir_in(output)?;
    let worker = ProcessWorker::new(std::env::current_exe()?).with_arguments([
        std::ffi::OsStr::new("--worker-fixture"),
        temporary.path().as_os_str(),
        std::ffi::OsStr::new("--publish-after-observed"),
    ]);
    let job = worker.start(ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 1,
        source_path: "unpolled.bin".into(),
        source: vec![31; SOURCE_SIZE],
        importer: "fixture".into(),
        version: "1".into(),
        settings: serde_json::json!({}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    })?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let marker = loop {
        if let Some(entry) = fs::read_dir(temporary.path())?.next() {
            break entry?;
        }
        assert!(Instant::now() < deadline, "worker never created its marker");
        thread::sleep(Duration::from_millis(2));
    };
    // Force enumeration before the child publishes readiness. Otherwise this
    // ordering depends on scheduling between File::create and set_len.
    assert_eq!(marker.metadata()?.len(), 0);
    // Windows DirEntry caches enumeration-time metadata. Poll the path instead
    // of repeatedly seeing the empty size captured before readiness publication.
    let marker = marker.path();
    File::create(temporary.path().join("observed"))?;
    while fs::metadata(&marker)?.len() == 0 || active(temporary.path())? != 0 {
        assert!(
            Instant::now() < deadline,
            "worker did not publish readiness and release its lock"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(worker.versions()?.get("fixture"), Some(&"1".to_owned()));
    assert!(
        worker
            .discover("fixture", "1", vec![], &Cancellation::default())?
            .is_empty()
    );
    let result = job.wait(&Cancellation::default(), |_| {})?;
    assert_eq!(result.artifacts[0].bytes, vec![31; SOURCE_SIZE]);
    println!("unpolled completed import permits versions/discovery and retains its result");
    Ok(())
}

fn check_unpolled_admission() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--unpolled-admission")
        .env("ARA_IMPORT_WORKERS", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success(), "unpolled result admission regression");
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            panic!("completed unpolled import blocked synchronous worker admission");
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--worker-fixture")) {
        let root = std::env::args_os()
            .nth(2)
            .ok_or("worker fixture directory")?;
        // An OS lock counts live child processes and is released even on kill.
        let marker = File::create(Path::new(&root).join(format!("{}.lock", std::process::id())))?;
        marker.lock()?;
        if std::env::args_os().nth(3).as_deref()
            == Some(std::ffi::OsStr::new("--publish-after-observed"))
        {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !Path::new(&root).join("observed").exists() {
                assert!(Instant::now() < deadline, "host never observed the marker");
                thread::sleep(Duration::from_millis(2));
            }
        }
        // Publish readiness after acquiring the lock, so exit checks cannot race startup.
        marker.set_len(1)?;
        thread::sleep(Duration::from_millis(200));
        let mut registry = ImporterRegistry::default();
        registry.register(Fixture)?;
        serve_worker(&registry)?;
        return Ok(());
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--unpolled-admission")) {
        return unpolled_admission();
    }
    check_unpolled_admission()?;
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
