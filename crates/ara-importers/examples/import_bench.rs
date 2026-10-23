//! Release fixture benchmark. Run each fixture/preset in a fresh process for peak RSS.
mod support;
use ara_asset::{
    CacheBackend, Cancellation, CookPreset, FileCache, ImportContext, ImportInput, ImportMetrics,
    Importer, TargetPlatform,
};
use ara_importers::GltfImporter;
use std::{collections::BTreeMap, path::PathBuf, time::Instant};
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let path = PathBuf::from(
        args.get(1)
            .ok_or("expected GLB path [fast|balanced|hq] [threads] [--verify]")?,
    );
    let preset = match args.get(2).map(String::as_str) {
        Some("fast") => CookPreset::Fast,
        Some("balanced") => CookPreset::Balanced,
        Some("hq") | None => CookPreset::HighQuality,
        _ => return Err("unknown preset".into()),
    };
    let mut input = ImportInput {
        preset,
        thread_budget: args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(16),
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
    let dir = tempfile::tempdir()?;
    let cache = FileCache::at(dir.path())?;
    let metrics = ImportMetrics::default();
    let registry = ara_importers::registry()?;
    let cpu = support::process_cpu_ms()?;
    let cold = Instant::now();
    let key = input.key()?;
    assert!(cache.load(key)?.is_none());
    let start = Instant::now();
    let output = registry.import(
        &input,
        &mut ImportContext {
            metrics: Some(&metrics),
            cancellation: &Cancellation::default(),
            progress: &mut |_| {},
        },
    )?;
    let cook_ms = ms(start);
    let cpu_ms = support::process_cpu_ms()? - cpu;
    let start = Instant::now();
    cache.store(key, &output)?;
    let store_ms = ms(start);
    let cold_ms = ms(cold);
    let mut warm = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let hit = cache.load(key)?.ok_or("cache miss after store")?;
        warm.push(ms(start));
        assert_eq!(hit.output, output);
    }
    warm.sort_by(f64::total_cmp);
    let peak = support::peak_memory_bytes()?;
    let quality = support::quality(&input.source, &output)?;
    if args.iter().any(|a| a == "--verify") {
        input.thread_budget = if input.thread_budget == 1 { 16 } else { 1 };
        let repeated = registry.import(
            &input,
            &mut ImportContext {
                metrics: None,
                cancellation: &Cancellation::default(),
                progress: &mut |_| {},
            },
        )?;
        assert_eq!(output, repeated, "preset output changed with thread budget");
        assert_eq!(key, input.key()?);
    }
    println!(
        "{}",
        ara_json::to_canonical_string(&ara_json::sort_keys(
            &serde_json::json!({"fixture":path.file_name().map(|s|s.to_string_lossy()),"preset":preset,"threads":args.get(3).map(String::as_str).unwrap_or("16"),"cookMs":cook_ms,"processCpuMs":cpu_ms,"coldMs":cold_ms,"cacheStoreMs":store_ms,"warmMedianMs":warm[5],"stages":metrics.snapshot(),"peakWorkingSetBytes":peak,"quality":quality,"bytes":output.artifacts.iter().map(|a|a.bytes.len()).sum::<usize>()})
        ))
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
