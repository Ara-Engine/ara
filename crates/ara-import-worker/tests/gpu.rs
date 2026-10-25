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
    let expected = ara_importers::registry()?.import_with_services(
        &input,
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
    let worker = ProcessWorker::new(env!("CARGO_BIN_EXE_ara-import-worker"));
    assert_eq!(
        worker.texture_encoder(&input),
        Some(encoder.backend()),
        "a usable device must negotiate through framed stdout"
    );
    let key = input.key_with_backend(Some(encoder.backend()))?;
    for _ in 0..3 {
        let actual = worker.encode_with_fallback(Arc::clone(&input), &cancellation, |_| {})?;
        assert_eq!(actual.0, key, "no silent CPU fallback");
        assert_eq!(actual.1, expected);
    }
    // These are separate disposable imports. Their service identity and device
    // creation counter prove actual headless-device reuse, not just equal bytes.
    if let Some(trace) = std::env::var_os("ARA_IMPORT_TRACE_DIR") {
        let mut services = std::collections::BTreeSet::new();
        let mut native = std::collections::BTreeSet::new();
        let mut initialized = 0.0_f64;
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
                }
            }
        }
        assert_eq!(services.len(), 1, "one service survives separate workers");
        assert_eq!(native.len(), 3, "three actual disposable imports");
        assert!(
            (1.0..=2.0).contains(&initialized),
            "bounded reusable devices"
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
    }
    Ok(())
}
