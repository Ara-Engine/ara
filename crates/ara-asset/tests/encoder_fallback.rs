//! A real protocol worker proves backend negotiation and CPU retry cache identity.
use ara_asset::{Artifact, ArtifactKind, BlockEncoder, CacheBackend, Cancellation, CookPreset, Error, FileCache, ImportContext, ImportInput, ImportOutput, ImportServices, Importer, ImporterRegistry, ProcessWorker, TargetPlatform};
use ara_asset_format::TextureFormat;
use std::{collections::BTreeMap, sync::Arc};

struct Fixture;
impl Importer for Fixture {
    fn id(&self) -> &str { "ara.texture" }
    fn version(&self) -> &str { "fixture-encoder-1" }
    fn import(&self, input: &ImportInput, _: &mut ImportContext<'_>) -> ara_asset::Result<ImportOutput> {
        Ok(output(input.source.clone()))
    }
    fn import_with_services(&self, input: &ImportInput, context: &mut ImportContext<'_>, services: ImportServices<'_>) -> ara_asset::Result<ImportOutput> {
        match services.encoder {
            Some(encoder) => Ok(output(encoder.encode(TextureFormat::Bc4, input.preset, &[0; 16], context.cancellation)?)),
            None => self.import(input, context),
        }
    }
}
fn output(bytes: Vec<u8>) -> ImportOutput {
    ImportOutput { artifacts: vec![Artifact { name: "fixture.bin".into(), kind: ArtifactKind::Custom, bytes }] }
}
struct FakeEncoder { fail: bool }
impl BlockEncoder for FakeEncoder {
    fn backend(&self) -> &str { "fixture-gpu-v1" }
    fn encode(&self, _: TextureFormat, _: CookPreset, _: &[u32], _: &Cancellation) -> ara_asset::Result<Vec<u8>> {
        if self.fail { Err(Error::EncoderUnavailable("simulated device loss".into())) }
        else { Ok(vec![0x47; 8]) }
    }
}
fn factory(_: Option<&std::path::Path>) -> ara_asset::Result<Box<dyn BlockEncoder>> {
    Ok(Box::new(FakeEncoder { fail: std::env::args().any(|arg| arg == "--fail") }))
}
fn input() -> ImportInput {
    ImportInput { preset: CookPreset::Balanced, thread_budget: 1, source_path: "fixture.bin".into(), source: b"complete original input".to_vec(), importer: Fixture.id().into(), version: Fixture.version().into(), settings: serde_json::Value::Null, target: TargetPlatform::Windows, dependencies: BTreeMap::from([("external.bin".into(), vec![137; 4096])]), asset_dependencies: BTreeMap::new() }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--worker") {
        let mut registry = ImporterRegistry::default();
        registry.register(Fixture)?;
        ara_asset::serve_worker_with_encoder(&registry, factory)?;
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
    let (key, encoded) = worker.encode_with_fallback(Arc::clone(&input), &cancel, |_| {})?;
    assert_eq!(key, gpu_key);
    assert_eq!(encoded, output(vec![0x47; 8]));
    let failed = ProcessWorker::new(executable).with_arguments(["--worker", "--fail"]);
    assert_eq!(failed.texture_encoder(&input), Some("fixture-gpu-v1"));
    let (key, fallback) = failed.encode_with_fallback(Arc::clone(&input), &cancel, |_| {})?;
    assert_eq!(key, cpu_key, "failed GPU output must never retain its GPU action key");
    assert_eq!(fallback, output(input.source.clone()), "CPU retry retains the complete input");
    assert_eq!(failed.texture_encoder(&input), None, "failed backend is disabled for this worker session");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-asset/encoder-fallback");
    std::fs::create_dir_all(&root)?;
    let directory = tempfile::tempdir_in(root)?;
    let cache = FileCache::at(directory.path())?;
    cache.store(key, &fallback)?;
    assert!(cache.load(cpu_key)?.is_some());
    assert!(cache.load(gpu_key)?.is_none());
    let repeated = failed.encode_with_fallback(input, &cancel, |_| {})?;
    assert_eq!(repeated.0, cpu_key);
    assert_eq!(repeated.1, fallback);
    println!("backend negotiation, zero-copy retry and CPU-only cache publication passed");
    Ok(())
}
