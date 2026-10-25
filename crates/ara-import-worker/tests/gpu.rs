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
    let actual = worker.encode_with_fallback(input, &cancellation, |_| {})?;
    assert_eq!(actual.0, key, "no silent CPU fallback");
    assert_eq!(actual.1, expected);
    Ok(())
}
