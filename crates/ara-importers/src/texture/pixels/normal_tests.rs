use super::*;

fn reference(image: DynamicImage, flip_y: bool) -> Vec<[f32; 4]> {
    image
        .into_rgba32f()
        .pixels()
        .map(|p| {
            let n = crate::mesh::normalize([
                p[0] * 2.0 - 1.0,
                (p[1] * 2.0 - 1.0) * if flip_y { -1.0 } else { 1.0 },
                p[2] * 2.0 - 1.0,
            ]);
            [n[0], n[1], n[2], p[3]]
        })
        .collect()
}

#[test]
fn normal_lookup_keeps_layout_tail_y_and_thread_bits() {
    let cancellation = Cancellation::default();
    let pools = [1, 4].map(|threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("private test pool")
    });
    for width in [1, 3, 257] {
        let rgba = image::RgbaImage::from_fn(width, 257, |x, y| {
            image::Rgba([x as u8, (x + y) as u8, (x ^ y) as u8, (x * 37 + y) as u8])
        });
        let images = [
            DynamicImage::ImageRgba8(rgba.clone()),
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba.clone()).into_rgb8()),
            DynamicImage::ImageLuma8(image::GrayImage::from_fn(width, 257, |x, y| {
                image::Luma([(x + y) as u8])
            })),
            DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_fn(width, 257, |x, y| {
                image::LumaA([(x + y) as u8, y as u8])
            })),
            DynamicImage::ImageRgba16(DynamicImage::ImageRgba8(rgba).into_rgba16()),
        ];
        for image in images {
            for flip_normal_y in [false, true] {
                let expected = reference(image.clone(), flip_normal_y);
                let settings = TextureSettings {
                    flip_normal_y,
                    usage: TextureUsage::Normal,
                    ..Default::default()
                };
                for pool in &pools {
                    let actual = pool
                        .install(|| {
                            prepare(
                                image.clone(),
                                TextureUsage::Normal,
                                &settings,
                                &cancellation,
                            )
                        })
                        .expect("finite normal image");
                    assert_eq!(actual.len(), expected.len());
                    assert!(
                        actual
                            .iter()
                            .zip(&expected)
                            .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)),
                        "{:?}, {width}, flip={flip_normal_y}",
                        image.color()
                    );
                }
            }
        }
    }
}

#[test]
fn normal_lookup_keeps_bc5_mips_cancellation_and_float_validation() {
    use super::super::{DecodedTexture, cook_texture_with_preset, encode_texture, generate_mips};
    use ara_asset::{CookPreset, ImportContext, TargetPlatform};
    use ara_asset_format::TextureFormat;
    use image::ImageEncoder;

    let image = image::RgbImage::from_fn(33, 65, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 3) as u8, 128])
    });
    let mut source = Vec::new();
    image::codecs::png::PngEncoder::new(&mut source)
        .write_image(&image, 33, 65, image::ExtendedColorType::Rgb8)
        .expect("PNG fixture");
    let cancellation = Cancellation::default();
    for flip_normal_y in [false, true] {
        let settings = TextureSettings {
            usage: TextureUsage::Normal,
            flip_normal_y,
            ..Default::default()
        };
        for preset in [CookPreset::Balanced, CookPreset::HighQuality] {
            let original = generate_mips(
                DecodedTexture {
                    width: 33,
                    height: 65,
                    usage: TextureUsage::Normal,
                    format: TextureFormat::Bc5,
                    pixels: reference(DynamicImage::ImageRgb8(image.clone()), flip_normal_y),
                    mipmaps: true,
                },
                &cancellation,
            )
            .expect("original normal mips");
            let expected = encode_texture(original, preset, &cancellation, None, None)
                .expect("original BC5 KTX2");
            let actual = cook_texture_with_preset(
                &source,
                &settings,
                TargetPlatform::Windows,
                preset,
                &mut ImportContext {
                    metrics: None,
                    cancellation: &cancellation,
                    progress: &mut |_| {},
                },
            )
            .expect("lookup BC5 KTX2");
            assert_eq!(actual, expected);
        }
    }
    cancellation.cancel();
    assert!(
        prepare(
            DynamicImage::ImageRgb8(image),
            TextureUsage::Normal,
            &TextureSettings::default(),
            &cancellation
        )
        .is_err()
    );
    assert!(
        prepare(
            DynamicImage::ImageRgb32F(image::Rgb32FImage::from_pixel(
                17,
                17,
                image::Rgb([f32::NAN, 0.5, 1.0])
            )),
            TextureUsage::Normal,
            &TextureSettings::default(),
            &Cancellation::default()
        )
        .is_err()
    );
}
