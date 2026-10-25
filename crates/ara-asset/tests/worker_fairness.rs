//! Real child processes keep queued CPU work from trailing an encoder wave.
use ara_asset::{
    Artifact, ArtifactKind, BlockEncoder, Cancellation, CookPreset, ImportContext, ImportInput,
    ImportOutput, Importer, ImporterRegistry, ProcessWorker, Result, TargetPlatform,
};
use ara_asset_format::TextureFormat;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

struct Fixture;
impl Importer for Fixture {
    fn id(&self) -> &str {
        "fixture.queue"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn import(&self, input: &ImportInput, _: &mut ImportContext<'_>) -> Result<ImportOutput> {
        let root = Path::new(
            input.settings["root"]
                .as_str()
                .expect("fixture scratch path"),
        );
        std::fs::write(root.join(format!("started-{}", input.source_path)), [])?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.join(format!("release-{}", input.source_path)).exists() {
            if Instant::now() >= deadline {
                return Err(ara_asset::Error::Worker("fixture gate timed out".into()));
            }
            thread::sleep(Duration::from_millis(2));
        }
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "fixture.bin".into(),
                kind: ArtifactKind::Custom,
                bytes: input.source.clone(),
            }],
        })
    }
}

// Encoded requests exercise the GPU-candidate admission class without a device.
struct FakeEncoder;
impl BlockEncoder for FakeEncoder {
    fn backend(&self) -> &str {
        "queue-fixture-v1"
    }
    fn encode(
        &self,
        _: TextureFormat,
        _: CookPreset,
        _: &[u32],
        _: &Cancellation,
    ) -> Result<Vec<u8>> {
        Ok(vec![0; 8])
    }
}
fn encoder(_: Option<&Path>) -> Result<Box<dyn BlockEncoder>> {
    Ok(Box::new(FakeEncoder))
}
fn input(root: &Path, label: &str) -> ImportInput {
    ImportInput {
        preset: CookPreset::Balanced,
        thread_budget: 1,
        source_path: label.into(),
        source: label.as_bytes().to_vec(),
        importer: Fixture.id().into(),
        version: Fixture.version().into(),
        settings: serde_json::json!({"root":root}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    }
}
fn wait_for_marker(root: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !root.join(format!("started-{label}")).exists() {
        assert!(Instant::now() < deadline, "worker {label} was not admitted");
        thread::sleep(Duration::from_millis(2));
    }
}
fn release(root: &Path, label: &str) -> std::io::Result<()> {
    std::fs::write(root.join(format!("release-{label}")), [])
}

fn exercise() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let capacity = 4.min(thread::available_parallelism()?.get());
    if capacity < 2 {
        println!("single-core admission is covered by the unit tests");
        return Ok(());
    }
    let output =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-asset/worker-fairness");
    std::fs::create_dir_all(&output)?;
    let scratch = tempfile::tempdir_in(output)?;
    let root = scratch.path();
    let worker = ProcessWorker::new(std::env::current_exe()?).with_arguments(["--worker"]);
    let mut blockers = Vec::new();
    for index in 0..capacity {
        let label = format!("encoded-{index}");
        blockers.push(
            worker.start_encoded(Arc::new(input(root, &label)), Some(FakeEncoder.backend()))?,
        );
        wait_for_marker(root, &label);
    }
    let mut queued = worker.start_encoded(
        Arc::new(input(root, "encoded-late")),
        Some(FakeEncoder.backend()),
    )?;
    let mut cpu = worker.start(input(root, "cpu"))?;
    // The first completed import remains completely unpolled. Its supervisor
    // must release admission independently of consuming the terminal reply.
    release(root, "encoded-0")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        // A frequently polled encoded job must not steal the CPU-reserved slot.
        let _ = queued.poll()?;
        let _ = cpu.poll()?;
        assert!(
            !root.join("started-encoded-late").exists(),
            "queued encoder stole the slot while a CPU cook was waiting"
        );
        if root.join("started-cpu").exists() {
            break;
        }
        assert!(Instant::now() < deadline, "queued CPU cook was starved");
        thread::sleep(Duration::from_millis(2));
    }
    drop(queued);
    release(root, "cpu")?;
    let output = cpu.wait(&Cancellation::default(), |_| {})?;
    assert_eq!(output.artifacts[0].bytes, b"cpu");
    assert_eq!(
        worker.versions()?.get(Fixture.id()).map(String::as_str),
        Some(Fixture.version())
    );
    for index in 0..capacity {
        release(root, &format!("encoded-{index}"))?;
    }
    for job in blockers {
        let _ = job.wait(&Cancellation::default(), |_| {})?;
    }
    // Queued jobs launch only when polled. A ticket at the head of a strict
    // FIFO would deadlock this later job even though a native slot is free.
    let mut blockers = Vec::new();
    for index in 0..capacity {
        let label = format!("cpu-{index}");
        blockers.push(worker.start(input(root, &label))?);
        wait_for_marker(root, &label);
    }
    let older = worker.start(input(root, "unpolled"))?;
    let mut later = worker.start(input(root, "polled"))?;
    release(root, "cpu-0")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !root.join("started-polled").exists() {
        let _ = later.poll()?;
        assert!(
            Instant::now() < deadline,
            "unpolled pending job blocked a later job"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!root.join("started-unpolled").exists());
    drop(older);
    release(root, "polled")?;
    let _ = later.wait(&Cancellation::default(), |_| {})?;
    for index in 0..capacity {
        release(root, &format!("cpu-{index}"))?;
    }
    for job in blockers {
        let _ = job.wait(&Cancellation::default(), |_| {})?;
    }
    println!(
        "CPU admission shared {capacity} slots with encoded work; metadata and unpolled exit passed"
    );
    Ok(())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--worker") {
        let mut registry = ImporterRegistry::default();
        registry.register(Fixture)?;
        ara_asset::serve_worker_with_encoder(&registry, encoder)?;
    } else if args.iter().any(|arg| arg == "--driver") {
        exercise()?;
    } else {
        let status = std::process::Command::new(std::env::current_exe()?)
            .arg("--driver")
            .env("ARA_IMPORT_WORKERS", "4")
            .status()?;
        assert!(status.success(), "worker fairness fixture failed: {status}");
    }
    Ok(())
}
