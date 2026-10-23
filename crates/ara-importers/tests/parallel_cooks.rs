#![allow(clippy::unwrap_used)]
use ara_asset::*;
use ara_importers::{TextureImporter, TextureUsage};
use image::ImageEncoder;
use std::collections::BTreeMap;

#[test]
fn tiled_presets_are_identical_at_one_and_sixteen_threads_including_alpha_and_npot() {
    let image = image::RgbaImage::from_fn(33, 65, |x, y| {
        image::Rgba([
            (x * 7) as u8,
            (y * 3) as u8,
            128,
            if x > 20 && y > 40 { 128 } else { 255 },
        ])
    });
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&image, 33, 65, image::ExtendedColorType::Rgba8)
        .unwrap();
    let mut opaque = image.clone();
    for p in opaque.pixels_mut() {
        p[3] = 255;
    }
    let mut hdr_png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut hdr_png)
        .write_image(&opaque, 33, 65, image::ExtendedColorType::Rgba8)
        .unwrap();
    let registry = ara_importers::registry().unwrap();
    for preset in [
        CookPreset::Fast,
        CookPreset::Balanced,
        CookPreset::HighQuality,
    ] {
        for usage in [
            TextureUsage::Color,
            TextureUsage::Linear,
            TextureUsage::Normal,
            TextureUsage::Mask,
            TextureUsage::Hdr,
        ] {
            let mut input = ImportInput {
                preset,
                thread_budget: 1,
                source_path: "parallel.png".into(),
                source: if usage == TextureUsage::Hdr {
                    hdr_png.clone()
                } else {
                    png.clone()
                },
                importer: TextureImporter.id().into(),
                version: TextureImporter.version().into(),
                settings: serde_json::json!({"usage":usage}),
                target: TargetPlatform::Windows,
                dependencies: BTreeMap::new(),
                asset_dependencies: BTreeMap::new(),
            };
            let cook = |input: &ImportInput| {
                registry
                    .import(
                        input,
                        &mut ImportContext {
                            metrics: None,
                            cancellation: &Cancellation::default(),
                            progress: &mut |_| {},
                        },
                    )
                    .unwrap()
            };
            let first = cook(&input);
            input.thread_budget = 16;
            assert_eq!(first, cook(&input), "{preset:?} {usage:?}");
        }
    }
    let mut input = ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 0,
        source_path: "parallel.png".into(),
        source: png,
        importer: TextureImporter.id().into(),
        version: TextureImporter.version().into(),
        settings: serde_json::json!({}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    };
    assert!(
        registry
            .import(
                &input,
                &mut ImportContext {
                    metrics: None,
                    cancellation: &Cancellation::default(),
                    progress: &mut |_| {}
                }
            )
            .is_err()
    );
    input.thread_budget = 257;
    assert!(
        registry
            .import(
                &input,
                &mut ImportContext {
                    metrics: None,
                    cancellation: &Cancellation::default(),
                    progress: &mut |_| {}
                }
            )
            .is_err()
    );
}
