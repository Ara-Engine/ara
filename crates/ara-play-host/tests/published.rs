//! A real protocol worker holds one import pending while the reader consumes others.
use ara_asset::{
    Artifact, ArtifactKind, ImportContext, ImportInput, ImportOutput, Importer, ImporterRegistry,
};
use ara_runtime::assets::AssetSource;
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

struct Fixture;
impl Importer for Fixture {
    fn id(&self) -> &str {
        "published-fixture"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn import(
        &self,
        input: &ImportInput,
        _: &mut ImportContext<'_>,
    ) -> ara_asset::Result<ImportOutput> {
        if input.source == b"pending" {
            let gate = input.settings["gate"].as_str().expect("fixture gate");
            while !Path::new(gate).exists() {
                thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "fixture".into(),
                kind: ArtifactKind::Custom,
                bytes: input.source.clone(),
            }],
        })
    }
}

fn driver() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-play-host/published")
        .join(std::process::id().to_string());
    fs::create_dir_all(&root)?;
    let release = root.join("release");
    fs::write(root.join("ready.bin"), b"ready")?;
    let sidecar = serde_json::json!({"$schema":"ara://schema/import/1","id":"as_ready",
        "records":{"i_settings":{"importer":"published-fixture","version":"1","settings":{"gate":release}}}});
    fs::write(
        root.join("ready.bin.import.json"),
        ara_json::to_canonical_string(&ara_json::sort_keys(&sidecar)),
    )?;
    // The read-only attachment is valid before the owner starts any asset work.
    let reader = ara_play_host::PublishedAssets::for_project(&root)?;
    assert_eq!(reader.revision(), 0);
    assert!(reader.snapshot().is_none());
    let mut assets = ara_play_host::assets::Assets::new(&root, &std::env::current_exe()?)?;
    let mut schema = ara_schema::SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_asset::register_schema(&mut schema);
    let mut workspace = ara_doc::Workspace::default();
    let ready = ara_asset::Sidecar::read(
        &fs::read_to_string(root.join("ready.bin.import.json"))?,
        "ready.bin.import.json",
        &schema,
    )?;
    workspace.insert("ready.bin.import.json", ready.into_document());
    assets.synchronize(&workspace, 1)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let first = loop {
        assets.poll(false, true);
        if let Some(snapshot) = reader.snapshot()
            && snapshot.generation("as_ready").is_some()
            && !assets.importing()
        {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "ready import must publish while another job is pending"
        );
        thread::sleep(Duration::from_millis(2));
    };
    // Start pending work only after the first cook, so the proof also works on a
    // one-core host whose importer admission cap is one.
    fs::write(root.join("pending.bin"), b"pending")?;
    let value = serde_json::json!({"$schema":"ara://schema/import/1","id":"as_pending",
        "records":{"i_settings":{"importer":"published-fixture","version":"1","settings":{"gate":release}}}});
    let text = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
    fs::write(root.join("pending.bin.import.json"), &text)?;
    workspace.insert(
        "pending.bin.import.json",
        ara_asset::Sidecar::read(&text, "pending.bin.import.json", &schema)?.into_document(),
    );
    assets.synchronize(&workspace, 2)?;
    assert!(assets.importing(), "fixture must have real pending work");
    assert!(first.generation("as_pending").is_none());
    assert_eq!(
        first.generation("as_ready").expect("ready").kinds(),
        [ArtifactKind::Custom]
    );
    let (_, loaded) = first
        .load("as_ready")?
        .expect("published file-backed generation");
    assert_eq!(loaded.output.artifacts[0].bytes, b"ready");
    assert_eq!(first.cache_path(), assets.cache);
    let revision = reader.revision();
    let start = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(reader.revision(), revision);
    }
    assert!(
        reader
            .snapshot()
            .expect("read during pending import")
            .generation("as_pending")
            .is_none()
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "publication reads must not wait for pending imports"
    );
    fs::write(&release, [])?;
    let second = loop {
        assets.poll(false, true);
        if let Some(snapshot) = reader.snapshot()
            && snapshot.generation("as_pending").is_some()
        {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "released import must publish");
        thread::sleep(Duration::from_millis(2));
    };
    assert!(second.revision() > first.revision());
    assert!(
        first.generation("as_pending").is_none(),
        "retained snapshots must remain immutable"
    );
    let stable = reader.revision();
    assets.poll(false, true);
    assert_eq!(
        reader.revision(),
        stable,
        "empty polls do not refresh views"
    );
    println!("completed imports publish immutable snapshots; pending imports never block readers");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ProcessWorker invokes this executable without arguments.
    if std::env::args().nth(1).is_none() && std::env::var_os("ARA_PUBLISHED_DRIVER").is_some() {
        let mut registry = ImporterRegistry::default();
        registry.register(Fixture)?;
        ara_asset::serve_worker(&registry)?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--driver") {
        return driver();
    }
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("--driver").env("ARA_PUBLISHED_DRIVER", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success(), "published generation regression");
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            panic!("snapshot read blocked on pending imports");
        }
        thread::sleep(Duration::from_millis(5));
    }
}
