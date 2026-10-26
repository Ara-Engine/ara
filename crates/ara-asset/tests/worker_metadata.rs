//! Real metadata children: batch cold queries and launch none for warm hits.
use ara_asset::{
    Cancellation, Error, ImportContext, ImportInput, ImportOutput, Importer, ImporterRegistry,
    ProcessWorker, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Barrier,
    time::Duration,
};

struct Fixture;
impl Importer for Fixture {
    fn id(&self) -> &str {
        "fixture.metadata"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn dependencies_with_settings(
        &self,
        source: &[u8],
        settings: &serde_json::Value,
    ) -> Result<Vec<String>> {
        let mut dependencies =
            vec![String::from_utf8(source.to_vec()).map_err(|e| Error::Invalid(e.to_string()))?];
        if let Some(companion) = settings.get("companion").and_then(|v| v.as_str()) {
            dependencies.push(companion.into());
        }
        Ok(dependencies)
    }
    fn import(&self, _: &ImportInput, _: &mut ImportContext<'_>) -> Result<ImportOutput> {
        Err(Error::Invalid("metadata fixture cannot cook".into()))
    }
}
#[derive(Deserialize)]
enum LegacyRequest {
    Discover {
        importer: String,
        version: String,
        #[serde(with = "serde_bytes")]
        source: Vec<u8>,
        settings: serde_json::Value,
    },
    Versions,
}
#[derive(Serialize)]
enum LegacyReply {
    Dependencies(Vec<String>),
    Versions(BTreeMap<String, String>),
    Failed(String),
}
fn legacy() -> Result<()> {
    let mut stdin = std::io::stdin().lock();
    let mut len = [0; 8];
    stdin.read_exact(&mut len)?;
    let len = u64::from_le_bytes(len);
    if len > 512 * 1024 * 1024 {
        return Err(Error::Invalid("frame too large".into()));
    }
    let mut bytes = vec![0; len as usize];
    stdin.read_exact(&mut bytes)?;
    let request: LegacyRequest =
        rmp_serde::from_slice(&bytes).map_err(|e| Error::Worker(e.to_string()))?;
    let reply = match request {
        LegacyRequest::Versions => {
            LegacyReply::Versions(BTreeMap::from([("fixture.metadata".into(), "1".into())]))
        }
        LegacyRequest::Discover {
            importer,
            version,
            source,
            settings,
        } => {
            if importer != Fixture.id() || version != Fixture.version() {
                LegacyReply::Failed("version mismatch".into())
            } else {
                LegacyReply::Dependencies(Fixture.dependencies_with_settings(&source, &settings)?)
            }
        }
    };
    let bytes = rmp_serde::to_vec_named(&reply).map_err(|e| Error::Worker(e.to_string()))?;
    let mut out = std::io::stdout().lock();
    out.write_all(&(bytes.len() as u64).to_le_bytes())?;
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}
fn starts(root: &Path) -> usize {
    std::fs::read_to_string(root.join("starts")).map_or(0, |text| text.lines().count())
}
fn configured_worker(root: &Path, legacy: bool) -> ProcessWorker {
    ProcessWorker::new(std::env::current_exe().expect("owned executable"))
        .with_arguments([
            "--worker".into(),
            root.as_os_str().to_owned(),
            if legacy { "legacy" } else { "current" }.into(),
        ])
        .with_memo(Some(&root.join("cache")))
        .with_timeout(Duration::from_secs(20))
        .with_texture_encoder(false)
}
fn parent() -> Result<()> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-asset/worker-metadata");
    std::fs::create_dir_all(&base)?;
    let root = tempfile::tempdir_in(&base)?;
    let worker = configured_worker(root.path(), false);
    let barrier = Barrier::new(33);
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for i in 0..32 {
            let worker = &worker;
            let barrier = &barrier;
            threads.push(scope.spawn(move || {
                barrier.wait();
                let source = format!("dep-{i}.bin");
                let result = worker.discover_with_settings(
                    Fixture.id(),
                    Fixture.version(),
                    source.as_bytes().to_vec(),
                    serde_json::Value::Null,
                    &Cancellation::default(),
                )?;
                assert_eq!(result, [source]);
                Ok::<_, Error>(())
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().expect("query thread").expect("discovery");
        }
    });
    let cold = starts(root.path());
    assert!(
        cold < 32,
        "concurrent queries were not batched: {cold} children"
    );
    for i in 0..32 {
        let source = format!("dep-{i}.bin");
        assert_eq!(
            worker.discover(
                Fixture.id(),
                Fixture.version(),
                source.as_bytes().to_vec(),
                &Cancellation::default()
            )?,
            [source]
        );
    }
    assert_eq!(starts(root.path()), cold, "warm lookup launched a child");
    let changed = worker.discover_with_settings(
        Fixture.id(),
        Fixture.version(),
        b"dep-0.bin".to_vec(),
        serde_json::json!({"companion":"changed.bin"}),
        &Cancellation::default(),
    )?;
    assert_eq!(changed, ["dep-0.bin", "changed.bin"]);
    assert!(
        starts(root.path()) > cold,
        "settings did not invalidate discovery"
    );
    let objects = root.path().join("cache/discovery/objects");
    for entry in std::fs::read_dir(objects)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::write(entry.path(), b"torn")?;
        }
    }
    assert_eq!(
        worker.discover(
            Fixture.id(),
            Fixture.version(),
            b"dep-0.bin".to_vec(),
            &Cancellation::default()
        )?,
        ["dep-0.bin"]
    );
    let legacy_root = tempfile::tempdir_in(&base)?;
    let legacy = configured_worker(legacy_root.path(), true);
    assert_eq!(
        legacy
            .clone()
            .with_texture_encoder(true)
            .versions_with_encoder_hint()?
            .get(Fixture.id()),
        Some(&Fixture.version().to_owned())
    );
    assert_eq!(
        legacy.discover(
            Fixture.id(),
            Fixture.version(),
            b"legacy.bin".to_vec(),
            &Cancellation::default()
        )?,
        ["legacy.bin"]
    );
    let before = starts(legacy_root.path());
    assert_eq!(
        legacy.discover(
            Fixture.id(),
            Fixture.version(),
            b"legacy.bin".to_vec(),
            &Cancellation::default()
        )?,
        ["legacy.bin"]
    );
    assert_eq!(starts(legacy_root.path()), before);
    println!(
        "32 cold queries shared {cold} children; warm, settings, torn memo and legacy fallback passed"
    );
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let result = if args.get(1).is_some_and(|arg| arg == "--worker") {
        let root = PathBuf::from(&args[2]);
        let record = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("starts"))?;
            writeln!(file, "{}", std::process::id())?;
            Ok(())
        })();
        record.and_then(|()| {
            if args.get(3).is_some_and(|arg| arg == "legacy") {
                return legacy();
            }
            let mut registry = ImporterRegistry::default();
            registry.register(Fixture)?;
            ara_asset::serve_worker(&registry)
        })
    } else {
        parent()
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
