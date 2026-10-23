#![allow(clippy::unwrap_used)]
use ara_asset::*;
use ara_importers::TextureImporter;
use image::ImageEncoder;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, OnceLock},
    thread,
    time::{Duration, Instant},
};

fn worker() -> ProcessWorker {
    ProcessWorker::new(env!("CARGO_BIN_EXE_ara-import-worker"))
}
fn png(red: u8) -> Vec<u8> {
    let pixels = [red, 64, 128, 255].repeat(64);
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, 8, 8, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}
fn input(red: u8) -> ImportInput {
    ImportInput {
        preset: Default::default(),
        thread_budget: 16,
        source_path: "color.png".into(),
        source: png(red),
        importer: TextureImporter.id().into(),
        version: TextureImporter.version().into(),
        settings: json!({}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    }
}
fn fault_worker() -> PathBuf {
    static EXE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    EXE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir
            .path()
            .join(format!("fault-worker{}", std::env::consts::EXE_SUFFIX));
        let status = std::process::Command::new("rustc")
            .arg("--edition=2024")
            .arg("-C")
            .arg("panic=abort")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fault_worker.rs"))
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap();
        assert!(status.success());
        (dir, exe)
    })
    .1
    .clone()
}
#[test]
fn worker_panics_native_abort_timeout_and_cancellation_leave_host_alive() {
    for mode in ["abort", "panic"] {
        let result = ProcessWorker::new(fault_worker())
            .with_arguments([mode])
            .start(input(1))
            .unwrap()
            .wait(&Cancellation::default(), |_| {});
        assert!(result.is_err(), "{mode}");
        assert_eq!(
            worker()
                .start(input(2))
                .unwrap()
                .wait(&Cancellation::default(), |_| {})
                .unwrap()
                .artifacts
                .len(),
            1
        );
    }
    let deadline = Instant::now();
    let result = ProcessWorker::new(fault_worker())
        .with_arguments(["hang"])
        .with_timeout(Duration::from_millis(200))
        .start(input(1))
        .unwrap()
        .wait(&Cancellation::default(), |_| {});
    assert!(result.unwrap_err().to_string().contains("deadline"));
    assert!(deadline.elapsed() < Duration::from_secs(5));
    let token = Cancellation::default();
    let job = ProcessWorker::new(fault_worker())
        .with_arguments(["hang"])
        .start(input(1))
        .unwrap();
    token.cancel();
    assert!(matches!(job.wait(&token, |_| {}), Err(Error::Cancelled)));
    let mut completed = worker().start(input(2)).unwrap();
    let start = Instant::now();
    loop {
        if matches!(completed.poll().unwrap(), Some(JobEvent::Complete(_))) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(2));
    }
    assert!(completed.wait(&Cancellation::default(), |_| {}).is_err());
}
fn wait_reload(reload: &mut HotReload) -> Vec<ReloadEvent> {
    let start = Instant::now();
    loop {
        let events = reload.poll();
        if events
            .iter()
            .any(|e| matches!(e, ReloadEvent::Ready { .. } | ReloadEvent::Failed { .. }))
        {
            return events;
        }
        assert!(start.elapsed() < Duration::from_secs(15));
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn cache_hits_last_good_fallback_supersession_and_human_application_policy() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FileCache::new(dir.path()).unwrap());
    let mut reload = HotReload::new(worker(), cache);
    reload.progressive = false;
    reload.request("as_color", input(10));
    assert!(wait_reload(&mut reload).iter().any(|e| matches!(
        e,
        ReloadEvent::Ready {
            cache_hit: false,
            ..
        }
    )));
    assert!(reload.slot("as_color").is_none());
    assert!(
        reload
            .apply_boundary(true, true, &BTreeSet::new())
            .is_empty()
    );
    assert_eq!(
        reload.apply_boundary(true, false, &BTreeSet::new()).len(),
        1
    );
    let first = reload
        .slot("as_color")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .key;
    let mut broken = input(20);
    broken.source = b"broken image".to_vec();
    reload.request("as_color", broken);
    assert!(
        wait_reload(&mut reload)
            .iter()
            .any(|e| matches!(e, ReloadEvent::Failed { .. }))
    );
    assert_eq!(
        reload
            .slot("as_color")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .key,
        first
    );
    reload.request("as_color", input(10));
    assert!(wait_reload(&mut reload).iter().any(|e| matches!(
        e,
        ReloadEvent::Ready {
            cache_hit: true,
            ..
        }
    )));
    assert!(
        reload
            .apply_boundary(true, false, &BTreeSet::new())
            .is_empty()
    );
    reload.policy = ApplyPolicy::OnApproval;
    reload.request("as_color", input(30));
    reload.request("as_color", input(40));
    wait_reload(&mut reload);
    assert!(
        reload
            .apply_boundary(true, false, &BTreeSet::new())
            .is_empty()
    );
    let expected = input(40).key().unwrap();
    let events = reload.apply_boundary(true, false, &BTreeSet::from(["as_color".into()]));
    assert_eq!(events[0].current, expected);
    assert_eq!(events[0].generation, 2);
    reload.policy = ApplyPolicy::WhenIdle;
    reload.request("as_color", input(50));
    wait_reload(&mut reload);
    assert!(
        reload
            .apply_boundary(false, false, &BTreeSet::new())
            .is_empty()
    );
    assert_eq!(
        reload.apply_boundary(true, false, &BTreeSet::new()).len(),
        1
    );
}
fn wait_pipeline(pipeline: &mut AssetPipeline, success: bool) {
    let start = Instant::now();
    loop {
        let events = pipeline.poll().unwrap();
        if events.iter().any(|e| {
            if success {
                matches!(e, ReloadEvent::Ready { .. })
            } else {
                matches!(e, ReloadEvent::Failed { .. })
            }
        }) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "pipeline never emitted terminal event"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn real_file_watcher_reimports_source_and_sidecar_and_keeps_last_good_on_error() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("color.png"), png(10)).unwrap();
    let mut db = ara_schema::SchemaDb::new();
    register_schema(&mut db);
    let cache = Arc::new(FileCache::new(root.path()).unwrap());
    let mut pipeline = AssetPipeline::new(root.path(), worker(), cache, Arc::new(db)).unwrap();
    pipeline
        .register(SourceAsset {
            preset: Default::default(),
            thread_budget: 16,
            id: "as_color".into(),
            path: "color.png".into(),
            importer: TextureImporter.id().into(),
            version: TextureImporter.version().into(),
            settings: json!({}),
            target: TargetPlatform::Windows,
            asset_dependencies: BTreeMap::new(),
        })
        .unwrap();
    pipeline.reload.progressive = false;
    pipeline.request("as_color").unwrap();
    wait_pipeline(&mut pipeline, true);
    pipeline
        .reload
        .apply_boundary(true, false, &BTreeSet::new());
    let first = pipeline
        .reload
        .slot("as_color")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .key;
    std::fs::write(root.path().join("color.png"), png(200)).unwrap();
    wait_pipeline(&mut pipeline, true);
    assert_eq!(
        pipeline
            .reload
            .slot("as_color")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .key,
        first
    );
    pipeline
        .reload
        .apply_boundary(true, false, &BTreeSet::new());
    let second = pipeline
        .reload
        .slot("as_color")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .key;
    assert_ne!(first, second);
    std::fs::write(root.path().join("color.png"), b"corrupt").unwrap();
    wait_pipeline(&mut pipeline, false);
    assert_eq!(
        pipeline
            .reload
            .slot("as_color")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .key,
        second
    );
    std::fs::write(
        root.path().join("color.png.import.json"),
        b"invalid sidecar",
    )
    .unwrap();
    wait_pipeline(&mut pipeline, false);
    assert_eq!(
        pipeline
            .reload
            .slot("as_color")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .key,
        second
    );
}

#[test]
fn external_buffer_changes_invalidate_and_reimport_only_the_consumer() {
    use ara_importers::GltfImporter;
    let root = tempfile::tempdir().unwrap();
    let source = json!({"asset":{"version":"2.0"},"buffers":[{"uri":"vertices.bin","byteLength":36}],"bufferViews":[{"buffer":0,"byteLength":36}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[2,1,0]}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0});
    std::fs::write(
        root.path().join("mesh.gltf"),
        serde_json::to_vec(&source).unwrap(),
    )
    .unwrap();
    let positions = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let bytes: Vec<_> = positions.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(root.path().join("vertices.bin"), &bytes).unwrap();
    let mut db = ara_schema::SchemaDb::new();
    register_schema(&mut db);
    let mut pipeline = AssetPipeline::new(
        root.path(),
        worker(),
        Arc::new(FileCache::new(root.path()).unwrap()),
        Arc::new(db),
    )
    .unwrap();
    pipeline
        .register(SourceAsset {
            preset: Default::default(),
            thread_budget: 16,
            id: "as_mesh".into(),
            path: "mesh.gltf".into(),
            importer: GltfImporter.id().into(),
            version: GltfImporter.version().into(),
            settings: json!({}),
            target: TargetPlatform::Windows,
            asset_dependencies: BTreeMap::new(),
        })
        .unwrap();
    pipeline.reload.progressive = false;
    pipeline.request("as_mesh").unwrap();
    wait_pipeline(&mut pipeline, true);
    pipeline
        .apply_boundary(true, false, &BTreeSet::new())
        .unwrap();
    assert_eq!(
        pipeline
            .reload
            .graph
            .users("vertices.bin")
            .collect::<Vec<_>>(),
        ["as_mesh"]
    );
    let before = pipeline
        .reload
        .slot("as_mesh")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .key;
    let mut modified = bytes;
    modified[12..16].copy_from_slice(&2.0f32.to_le_bytes());
    std::fs::write(root.path().join("vertices.bin"), modified).unwrap();
    wait_pipeline(&mut pipeline, true);
    let events = pipeline
        .apply_boundary(true, false, &BTreeSet::new())
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_ne!(events[0].current, before);
}

#[test]
#[ignore = "requires gitignored Khronos fixtures; see ara-importers/MEASUREMENTS.md"]
fn real_khronos_scenes_are_deterministic_and_cacheable() {
    for filename in ["DamagedHelmet.glb", "Fox.glb"] {
        if std::env::var("ARA_FIXTURE").is_ok_and(|filter| filter != filename) {
            continue;
        }
        use ara_importers::GltfImporter;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../ara-importers/.fixtures")
            .join(filename);
        let input = ImportInput {
            preset: Default::default(),
            thread_budget: 16,
            source_path: filename.into(),
            source: std::fs::read(path).unwrap(),
            importer: GltfImporter.id().into(),
            version: GltfImporter.version().into(),
            settings: json!({}),
            target: TargetPlatform::Windows,
            dependencies: BTreeMap::new(),
            asset_dependencies: BTreeMap::new(),
        };
        let start = Instant::now();
        let output = worker()
            .start(input.clone())
            .unwrap()
            .wait(&Cancellation::default(), |_| {})
            .unwrap();
        let elapsed = start.elapsed();
        let gltf = gltf::Gltf::from_slice(&input.source).unwrap();
        for image in gltf.images() {
            let gltf::image::Source::View { view, .. } = image.source() else {
                continue;
            };
            let source = &gltf.blob.as_ref().unwrap()[view.offset()..view.offset() + view.length()];
            let reference = image::load_from_memory(source).unwrap().into_rgba8();
            for usage in ["Color", "Linear"] {
                let name = format!("textures/{}-{usage}.ktx2", image.index());
                let Some(artifact) = output.artifacts.iter().find(|a| a.name == name) else {
                    continue;
                };
                let texture = ara_asset_format::Texture::read(&artifact.bytes).unwrap();
                let [w, h] = texture.dimensions();
                let mut decoded = vec![0; w as usize * h as usize];
                texture2ddecoder::decode_bc7(
                    texture.levels().next().unwrap(),
                    w as usize,
                    h as usize,
                    &mut decoded,
                )
                .unwrap();
                let mut square = 0.0;
                for (p, &d) in reference.pixels().zip(&decoded) {
                    let channels = d.to_le_bytes();
                    for (a, b) in p.0[..3].iter().zip([channels[2], channels[1], channels[0]]) {
                        square += (*a as f64 - b as f64).powi(2);
                    }
                }
                let mse = square / (decoded.len() * 3) as f64;
                let psnr = 10.0 * (255.0f64.powi(2) / mse).log10();
                println!(
                    "{filename} image {} {usage}: {psnr:.3} dB RGB PSNR at {w}x{h}",
                    image.index()
                );
                assert!(psnr > 25.0);
            }
        }
        let repeated = worker()
            .start(input.clone())
            .unwrap()
            .wait(&Cancellation::default(), |_| {})
            .unwrap();
        assert_eq!(output, repeated);
        let dir = tempfile::tempdir().unwrap();
        let cache = FileCache::new(dir.path()).unwrap();
        let key = input.key().unwrap();
        cache.store(key, &output).unwrap();
        let start = Instant::now();
        for _ in 0..10 {
            assert_eq!(cache.load(key).unwrap().unwrap().output, output);
        }
        let cache_ms = start.elapsed().as_secs_f64() * 100.0;
        let count = |kind| output.artifacts.iter().filter(|a| a.kind == kind).count();
        let size: usize = output.artifacts.iter().map(|a| a.bytes.len()).sum();
        println!(
            "{filename}: {:.3} ms process cook, {:.3} ms validated cache hit, {size} cooked bytes; {} meshes, {} textures, {} skins, {} clips",
            elapsed.as_secs_f64() * 1000.0,
            cache_ms,
            count(ArtifactKind::Mesh),
            count(ArtifactKind::Texture),
            count(ArtifactKind::Skeleton),
            count(ArtifactKind::Clip)
        );
        assert!(count(ArtifactKind::Mesh) > 0);
        assert!(count(ArtifactKind::Texture) > 0);
    }
}
