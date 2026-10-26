//! The production GPU worker must keep library/device logging off protocol stdout.
use ara_asset::{
    BlockEncoder, Cancellation, CookPreset, ImportContext, ImportInput, ImportServices, Importer,
    ProcessWorker, TargetPlatform,
};
use image::ImageEncoder;
use std::{collections::BTreeMap, sync::Arc};

#[test]
#[ignore = "requires Vulkan; run through ara-dev gpu with a private profile"]
fn gpu_worker_negotiates_and_encodes_without_contaminating_protocol_stdout()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = root.join("../../target/ara-import-worker/gpu-protocol");
    std::fs::create_dir_all(&scratch)?;
    let cache = tempfile::tempdir_in(scratch)?;
    let encoder =
        ara_texture_gpu::Encoder::new(&root.join("../ara-texture-gpu/cooked"), cache.path())?;
    let mut source = Vec::new();
    image::codecs::png::PngEncoder::new(&mut source).write_image(
        &[45, 87, 91, 255].repeat(64),
        8,
        8,
        image::ExtendedColorType::Rgba8,
    )?;
    let input = Arc::new(ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 2,
        source_path: "color.png".into(),
        source,
        importer: ara_importers::TextureImporter.id().into(),
        version: ara_importers::TextureImporter.version().into(),
        settings: serde_json::json!({}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    });
    let cancellation = Cancellation::default();
    let configured = std::env::var_os("ARA_TEST_IMPORT_WORKER")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_ara-import-worker")));
    let owned_binary = tempfile::tempdir_in(configured.parent().expect("worker binary directory"))?;
    let worker_binary = owned_binary.path().join("ara-import-worker.exe");
    let original_bytes = std::fs::read(&configured)?;
    std::fs::write(&worker_binary, &original_bytes)?;
    std::fs::set_permissions(
        &worker_binary,
        std::fs::metadata(&configured)?.permissions(),
    )?;
    let worker = ProcessWorker::new(&worker_binary).with_memo(Some(cache.path()));
    assert_eq!(
        worker.texture_encoder(&input),
        Some(encoder.backend()),
        "a usable device must negotiate through framed stdout"
    );
    // Keep the negotiated lease/capability memo, but force each disposable job
    // to encode rather than hit the pure texture memo. This probe counts devices.
    let worker = worker.with_memo(None);
    for preset in [
        CookPreset::Fast,
        CookPreset::Balanced,
        CookPreset::HighQuality,
    ] {
        let mut request = (*input).clone();
        request.preset = preset;
        let request = Arc::new(request);
        let expected = ara_importers::registry()?.import_with_services(
            &request,
            &mut ImportContext {
                metrics: None,
                cancellation: &cancellation,
                progress: &mut |_| {},
            },
            ImportServices {
                memo: None,
                encoder: Some(&encoder),
            },
        )?;
        let key = request.key_with_backend(Some(encoder.backend()))?;
        for _ in 0..2 {
            let actual =
                worker.encode_with_fallback(Arc::clone(&request), &cancellation, |_| {})?;
            assert_eq!(actual.0, key, "no silent CPU fallback");
            assert_eq!(actual.1, expected);
        }
    }
    // Cargo must be able to replace its regular worker while the leased helper
    // stays alive. Overwriting a mapped PE image fails on Windows; use our own
    // private copy so this test never races Cargo's actual output file.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match std::fs::write(&worker_binary, &original_bytes) {
            Ok(()) => break,
            Err(error) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "live GPU helper locked its source worker executable: {error}"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    // These are separate disposable imports. Their service identity and device
    // creation counter prove actual headless-device reuse, not just equal bytes.
    if let Some(trace) = std::env::var_os("ARA_IMPORT_TRACE_DIR") {
        let mut services = std::collections::BTreeSet::new();
        let mut native = std::collections::BTreeSet::new();
        let mut initialized = 0.0_f64;
        let mut helper_executables = Vec::new();
        for file in std::fs::read_dir(trace)? {
            for block in std::fs::read_to_string(file?.path())?.split("\n\n") {
                let Ok(event) = serde_json::from_str::<serde_json::Value>(block) else {
                    continue;
                };
                if event["stage"] == "texture.gpu" {
                    services
                        .insert(event["data"]["service.pid"].as_f64().expect("service PID") as u64);
                    native.insert(event["pid"].as_u64().expect("native worker PID"));
                    initialized = initialized.max(
                        event["data"]["service.devices_created"]
                            .as_f64()
                            .expect("device count"),
                    );
                } else if event["stage"] == "texture.service.start" {
                    helper_executables.push(std::path::PathBuf::from(
                        event["data"]["executable"]
                            .as_str()
                            .expect("helper executable path"),
                    ));
                }
            }
        }
        assert_eq!(services.len(), 1, "one service survives separate workers");
        assert_eq!(
            native.len(),
            6,
            "six actual disposable imports across presets"
        );
        assert!(
            (1.0..=2.0).contains(&initialized),
            "bounded reusable devices"
        );
        assert_eq!(helper_executables.len(), 1);
        assert_ne!(
            helper_executables[0], worker_binary,
            "helper runs an immutable runtime copy"
        );
        drop(worker);
        let service_pid = *services.first().expect("one service");
        let trace = std::path::PathBuf::from(
            std::env::var_os("ARA_IMPORT_TRACE_DIR").expect("trace selected above"),
        );
        let path = trace.join(format!("trace-{service_pid}.jsonl"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !std::fs::read_to_string(&path)?.contains("texture.service.exit") {
            assert!(
                std::time::Instant::now() < deadline,
                "GPU service survived its host lease"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let warm = ProcessWorker::new(&worker_binary).with_memo(Some(cache.path()));
        assert_eq!(warm.texture_encoder(&input), Some(encoder.backend()));
        let mut starts = 0;
        let mut hits = 0;
        for file in std::fs::read_dir(&trace)? {
            let text = std::fs::read_to_string(file?.path())?;
            starts += text.matches("\"stage\": \"texture.service.start\"").count();
            hits += text
                .matches("\"stage\": \"texture.identity_cache_hit\"")
                .count();
        }
        assert_eq!(starts, 1, "warm identity lookup recreated a device service");
        assert_eq!(
            hits, 1,
            "warm identity was not recovered from its verified memo"
        );
    }
    Ok(())
}
