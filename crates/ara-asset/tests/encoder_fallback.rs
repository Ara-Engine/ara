//! A real protocol worker proves backend negotiation and CPU retry cache identity.
use ara_asset::{
    Artifact, ArtifactKind, BlockEncoder, CacheBackend, Cancellation, CookPreset, Error, FileCache,
    ImportContext, ImportInput, ImportOutput, ImportServices, Importer, ImporterRegistry,
    ProcessWorker, TargetPlatform,
};
use ara_asset_format::TextureFormat;
use std::{collections::BTreeMap, sync::Arc};

struct Fixture;
impl Importer for Fixture {
    fn id(&self) -> &str {
        "ara.texture"
    }
    fn version(&self) -> &str {
        "fixture-encoder-1"
    }
    fn import(
        &self,
        input: &ImportInput,
        _: &mut ImportContext<'_>,
    ) -> ara_asset::Result<ImportOutput> {
        Ok(output(input.source.clone()))
    }
    fn import_with_services(
        &self,
        input: &ImportInput,
        context: &mut ImportContext<'_>,
        services: ImportServices<'_>,
    ) -> ara_asset::Result<ImportOutput> {
        match services.encoder {
            Some(encoder) => Ok(output(encoder.encode(
                TextureFormat::Bc4,
                input.preset,
                &[0; 16],
                context.cancellation,
            )?)),
            None => self.import(input, context),
        }
    }
}
fn output(bytes: Vec<u8>) -> ImportOutput {
    ImportOutput {
        artifacts: vec![Artifact {
            name: "fixture.bin".into(),
            kind: ArtifactKind::Custom,
            bytes,
        }],
    }
}
struct FakeEncoder {
    fail: bool,
    memo: Option<std::path::PathBuf>,
}
impl BlockEncoder for FakeEncoder {
    fn backend(&self) -> &str {
        "fixture-gpu-v1"
    }
    fn prepare(&self) -> ara_asset::Result<()> {
        if let Some(root) = &self.memo {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("prepares"))?;
            writeln!(file, "prepared")?;
        }
        Ok(())
    }
    fn encode(
        &self,
        _: TextureFormat,
        _: CookPreset,
        _: &[u32],
        _: &Cancellation,
    ) -> ara_asset::Result<Vec<u8>> {
        if self.fail {
            Err(Error::EncoderUnavailable("simulated device loss".into()))
        } else {
            Ok(vec![0x47; 8])
        }
    }
}
fn factory(memo: Option<&std::path::Path>) -> ara_asset::Result<Box<dyn BlockEncoder>> {
    Ok(Box::new(FakeEncoder {
        fail: std::env::args().any(|arg| arg == "--fail"),
        memo: memo.map(std::path::Path::to_path_buf),
    }))
}
fn hint(memo: Option<&std::path::Path>) -> ara_asset::Result<Option<String>> {
    if let Some(root) = memo {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("hint-queries"))?;
        writeln!(file, "query")?;
    }
    Ok(memo.and_then(|root| std::fs::read_to_string(root.join("capability")).ok()))
}
fn prepares(root: &std::path::Path) -> usize {
    std::fs::read_to_string(root.join("prepares")).map_or(0, |s| s.lines().count())
}
fn cached_identity(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let cache = Arc::new(FileCache::at(root)?);
    std::fs::write(root.join("capability"), "fixture-gpu-v1")?;
    let worker = ProcessWorker::new(std::env::current_exe()?)
        .with_arguments(["--worker"])
        .with_memo(Some(root));
    let input = input();
    assert_eq!(
        worker.versions_with_encoder_hint()?.get(Fixture.id()),
        Some(&Fixture.version().to_owned())
    );
    let key = input.key_with_backend(Some("fixture-gpu-v1"))?;
    cache.store(key, &output(vec![0x47; 8]))?;
    let mut reload = ara_asset::HotReload::new(worker.clone(), cache.clone());
    reload.progressive = false;
    reload.request("asset", input.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let events = reload.poll();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ara_asset::ReloadEvent::Failed { .. })),
            "cache-hit reload failed"
        );
        if events.iter().any(|e| {
            matches!(
                e,
                ara_asset::ReloadEvent::Ready {
                    cache_hit: true,
                    final_pass: true,
                    ..
                }
            )
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cached reload stalled"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(prepares(root), 0, "cache hit prepared the device");
    assert_eq!(
        std::fs::read_to_string(root.join("hint-queries"))?
            .lines()
            .count(),
        1,
        "version metadata did not share its cached identity with imports"
    );
    let mut changed = input;
    changed.source.push(1);
    let cancel = Cancellation::default();
    let (actual, bytes) =
        worker.encode_with_fallback(Arc::new(changed.clone()), &cancel, |_| {})?;
    assert_eq!(actual, changed.key_with_backend(Some("fixture-gpu-v1"))?);
    assert_eq!(bytes, output(vec![0x47; 8]));
    assert_eq!(prepares(root), 1, "first miss must prepare once");
    worker.encode_with_fallback(Arc::new(changed.clone()), &cancel, |_| {})?;
    assert_eq!(prepares(root), 1, "repeat miss renegotiated the device");
    std::fs::write(root.join("capability"), "stale-gpu")?;
    let stale = ProcessWorker::new(std::env::current_exe()?)
        .with_arguments(["--worker"])
        .with_memo(Some(root));
    assert_eq!(stale.texture_encoder(&changed), Some("stale-gpu"));
    let (actual, bytes) = stale.encode_with_fallback(Arc::new(changed.clone()), &cancel, |_| {})?;
    assert_eq!(actual, changed.key()?, "mismatched hint retained stale key");
    assert_eq!(bytes, output(changed.source.clone()));
    assert_eq!(stale.texture_encoder(&changed), None);
    Ok(())
}
fn input() -> ImportInput {
    ImportInput {
        preset: CookPreset::Balanced,
        thread_budget: 1,
        source_path: "fixture.bin".into(),
        source: b"complete original input".to_vec(),
        importer: Fixture.id().into(),
        version: Fixture.version().into(),
        settings: serde_json::Value::Null,
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::from([("external.bin".into(), vec![137; 4096])]),
        asset_dependencies: BTreeMap::new(),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--worker") {
        let mut registry = ImporterRegistry::default();
        registry.register(Fixture)?;
        ara_asset::serve_worker_with_encoder_hint(&registry, factory, hint)?;
        return Ok(());
    }
    let executable = std::env::current_exe()?;
    let worker = ProcessWorker::new(&executable).with_arguments(["--worker"]);
    let input = Arc::new(input());
    let cancel = Cancellation::default();
    let cpu_key = input.key()?;
    let gpu_key = input.key_with_backend(Some("fixture-gpu-v1"))?;
    assert_ne!(cpu_key, gpu_key);
    assert_eq!(worker.texture_encoder(&input), Some("fixture-gpu-v1"));
    let cpu_only = worker.clone().with_texture_encoder(false);
    assert_eq!(cpu_only.texture_encoder(&input), None);
    assert_eq!(worker.texture_encoder(&input), Some("fixture-gpu-v1"));
    assert_eq!(
        cpu_only.encode_with_fallback(Arc::clone(&input), &cancel, |_| {})?,
        (cpu_key, output(input.source.clone()))
    );
    let (key, encoded) = worker.encode_with_fallback(Arc::clone(&input), &cancel, |_| {})?;
    assert_eq!(key, gpu_key);
    assert_eq!(encoded, output(vec![0x47; 8]));
    let failed = ProcessWorker::new(executable).with_arguments(["--worker", "--fail"]);
    assert_eq!(failed.texture_encoder(&input), Some("fixture-gpu-v1"));
    let (key, fallback) = failed.encode_with_fallback(Arc::clone(&input), &cancel, |_| {})?;
    assert_eq!(
        key, cpu_key,
        "failed GPU output must never retain its GPU action key"
    );
    assert_eq!(
        fallback,
        output(input.source.clone()),
        "CPU retry retains the complete input"
    );
    assert_eq!(
        failed.texture_encoder(&input),
        None,
        "failed backend is disabled for this worker session"
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-asset/encoder-fallback");
    std::fs::create_dir_all(&root)?;
    let directory = tempfile::tempdir_in(root)?;
    let cache = FileCache::at(directory.path())?;
    cache.store(key, &fallback)?;
    assert!(cache.load(cpu_key)?.is_some());
    assert!(cache.load(gpu_key)?.is_none());
    let repeated = failed.encode_with_fallback(input, &cancel, |_| {})?;
    assert_eq!(repeated.0, cpu_key);
    assert_eq!(repeated.1, fallback);
    let warm = tempfile::tempdir_in(directory.path())?;
    cached_identity(warm.path())?;
    println!("backend negotiation, zero-copy retry and CPU-only cache publication passed");
    Ok(())
}
