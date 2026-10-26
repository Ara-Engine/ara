use super::*;
use image::ImageEncoder;

// The pre-lookup path is the byte-compatibility oracle, including image's own
// layout-specific sample conversion and the existing transfer function.
fn reference(image: DynamicImage) -> Vec<[f32; 4]> {
    image
        .into_rgba32f()
        .pixels()
        .map(|p| {
            [
                srgb_to_linear(p[0]),
                srgb_to_linear(p[1]),
                srgb_to_linear(p[2]),
                p[3],
            ]
        })
        .collect()
}

#[test]
fn all_color_codes_layouts_and_tails_keep_the_original_float_bits() {
    let settings = TextureSettings::default();
    let cancel = Cancellation::default();
    let pools = [1, 4].map(|threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("private test pool")
    });
    for width in [1, 2, 3, 5, 17, 255, 256, 257] {
        let rgba = image::RgbaImage::from_fn(width, 257, |x, y| {
            image::Rgba([x as u8, 255 - x as u8, (x ^ y) as u8, (x * 37 + y) as u8])
        });
        let images = [
            DynamicImage::ImageRgba8(rgba.clone()),
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).into_rgb8()),
            DynamicImage::ImageLuma8(image::GrayImage::from_fn(width, 257, |x, y| {
                image::Luma([(x + y) as u8])
            })),
            DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_fn(width, 257, |x, y| {
                image::LumaA([(x + y) as u8, y as u8])
            })),
        ];
        for image in images {
            let expected = reference(image.clone());
            for pool in &pools {
                let actual = pool
                    .install(|| prepare(image.clone(), TextureUsage::Color, &settings, &cancel))
                    .expect("finite eight-bit color conversion");
                assert_eq!(actual.len(), expected.len());
                assert!(
                    actual
                        .iter()
                        .zip(&expected)
                        .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)),
                    "{:?} width={width}",
                    image.color()
                );
            }
        }
    }
}

#[test]
fn lookup_preserves_complete_color_ktx2_mips_and_cancellation() {
    use super::super::{DecodedTexture, cook_texture, encode_texture, generate_mips};
    use ara_asset::{CookPreset, ImportContext, TargetPlatform};
    use ara_asset_format::TextureFormat;
    let image = image::RgbaImage::from_fn(33, 65, |x, y| {
        image::Rgba([(x * 7) as u8, (y * 3) as u8, 128, (x * 13 + y) as u8])
    });
    let mut source = Vec::new();
    image::codecs::png::PngEncoder::new(&mut source)
        .write_image(&image, 33, 65, image::ExtendedColorType::Rgba8)
        .expect("PNG fixture");
    let cancellation = Cancellation::default();
    let pixels = reference(DynamicImage::ImageRgba8(image.clone()));
    let prepared = generate_mips(
        DecodedTexture {
            width: 33,
            height: 65,
            usage: TextureUsage::Color,
            format: TextureFormat::Bc7Srgb,
            pixels,
            mipmaps: true,
        },
        &cancellation,
    )
    .expect("original mip chain");
    let expected = encode_texture(prepared, CookPreset::default(), &cancellation, None, None)
        .expect("original color KTX2");
    let actual = cook_texture(
        &source,
        &TextureSettings::default(),
        TargetPlatform::Windows,
        &mut ImportContext {
            metrics: None,
            cancellation: &cancellation,
            progress: &mut |_| {},
        },
    )
    .expect("lookup color KTX2");
    assert_eq!(actual, expected);
    cancellation.cancel();
    assert!(
        prepare(
            DynamicImage::ImageRgba8(image),
            TextureUsage::Color,
            &TextureSettings::default(),
            &cancellation
        )
        .is_err()
    );
}
