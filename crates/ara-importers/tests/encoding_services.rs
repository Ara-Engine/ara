//! The optional backend sees one packed mip chain; normals retain qualified ISPC.
use ara_asset::{
    BlockEncoder, Cancellation, CookPreset, ImportContext, ImportInput, ImportServices, Importer,
    TargetPlatform,
};
use ara_asset_format::TextureFormat;
use image::ImageEncoder;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

struct MaskEncoder(AtomicUsize);
impl BlockEncoder for MaskEncoder {
    fn backend(&self) -> &str {
        "fixture-cpu-bc4"
    }
    fn encode(
        &self,
        format: TextureFormat,
        _: CookPreset,
        blocks: &[u32],
        _: &Cancellation,
    ) -> ara_asset::Result<Vec<u8>> {
        assert_eq!(format, TextureFormat::Bc4);
        self.0.fetch_add(1, Ordering::Relaxed);
        let mut encoded = Vec::new();
        for block in blocks.as_chunks::<16>().0 {
            let pixels: Vec<_> = block.iter().map(|word| *word as u8).collect();
            encoded.extend(intel_tex_2::bc4::compress_blocks(&intel_tex_2::RSurface {
                data: &pixels,
                width: 4,
                height: 4,
                stride: 4,
            }));
        }
        Ok(encoded)
    }
}
fn input(usage: &str) -> ImportInput {
    let image = image::RgbaImage::from_fn(17, 9, |x, y| {
        image::Rgba([
            (x * 11) as u8,
            (y * 23) as u8,
            211,
            ((x * 7 + y * 13) % 256) as u8,
        ])
    });
    let mut source = Vec::new();
    image::codecs::png::PngEncoder::new(&mut source)
        .write_image(&image, 17, 9, image::ExtendedColorType::Rgba8)
        .expect("png");
    ImportInput {
        preset: CookPreset::HighQuality,
        thread_budget: 2,
        source_path: "image.png".into(),
        source,
        importer: ara_importers::TextureImporter.id().into(),
        version: ara_importers::TextureImporter.version().into(),
        settings: serde_json::json!({"usage":usage,"maskChannel":3}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    }
}
#[test]
fn packed_mask_mips_match_ispc_and_normals_never_use_the_unqualified_backend() {
    let registry = ara_importers::registry().expect("registry");
    let encoder = MaskEncoder(AtomicUsize::new(0));
    let cancel = Cancellation::default();
    for usage in ["mask", "normal"] {
        let input = input(usage);
        let mut context = ImportContext {
            metrics: None,
            cancellation: &cancel,
            progress: &mut |_| {},
        };
        let expected = registry.import(&input, &mut context).expect("ISPC");
        let actual = registry
            .import_with_services(
                &input,
                &mut context,
                ImportServices {
                    memo: None,
                    encoder: Some(&encoder),
                },
            )
            .expect("optional service");
        assert_eq!(actual, expected, "{usage} bytes and mip order");
    }
    assert_eq!(
        encoder.0.load(Ordering::Relaxed),
        1,
        "one call for all mask mips, none for normals"
    );
}
