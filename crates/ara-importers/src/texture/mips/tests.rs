use super::*;

fn chain(
    mut pixels: Vec<[f32; 4]>,
    mut w: u32,
    mut h: u32,
    usage: TextureUsage,
    fast: bool,
) -> Vec<Vec<[f32; 4]>> {
    let mut levels = Vec::new();
    while w != 1 || h != 1 {
        let nw = (w / 2).max(1);
        let nh = (h / 2).max(1);
        pixels = if fast {
            downsample(&pixels, w, h, nw, nh, usage)
        } else {
            area_weighted(&pixels, w, h, nw, nh, usage)
        };
        levels.push(pixels.clone());
        w = nw;
        h = nh;
    }
    levels
}

#[test]
fn complete_chains_keep_weighted_filter_bits_at_each_thread_budget() {
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("private mip test pool");
        pool.install(|| {
            for (w, h) in [
                (1, 1),
                (1, 256),
                (2, 2),
                (4, 8),
                (512, 256),
                (16384, 2),
                (2, 16384),
                (3, 7),
                (33, 65),
                (126, 62),
            ] {
                let pixels: Vec<_> = (0..w * h)
                    .map(|i| {
                        [
                            i as f32 / 65535.,
                            (i % 256) as f32 / 255.,
                            (i % 17) as f32 / 16.,
                            [0., -0., 1e-22, 0.25, 0.75, 1.][i as usize % 6],
                        ]
                    })
                    .collect();
                for usage in [
                    TextureUsage::Color,
                    TextureUsage::Linear,
                    TextureUsage::Normal,
                    TextureUsage::Mask,
                    TextureUsage::Hdr,
                ] {
                    let expected = chain(pixels.clone(), w, h, usage, false);
                    let actual = chain(pixels.clone(), w, h, usage, true);
                    assert_eq!(actual.len(), expected.len());
                    for (a, b) in actual.iter().zip(&expected) {
                        assert_eq!(a.len(), b.len());
                        assert!(
                            a.iter()
                                .zip(b)
                                .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)),
                            "{w}x{h} {usage:?}, threads={threads}"
                        );
                    }
                }
            }
        });
    }
}

#[test]
fn non_half_size_requests_retain_area_weights() {
    let pixels: Vec<_> = (0..64 * 32)
        .map(|i| [i as f32 / 2048., 0.25, 0.5, 0.75])
        .collect();
    let actual = downsample(&pixels, 64, 32, 17, 9, TextureUsage::Color);
    let expected = area_weighted(&pixels, 64, 32, 17, 9, TextureUsage::Color);
    assert!(
        actual
            .iter()
            .zip(&expected)
            .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits))
    );
}

#[test]
fn color_ktx2_bytes_match_the_original_filter_for_both_presets() {
    use super::super::{
        MippedTexture, TextureSettings, cook_texture_with_preset, decode_texture, encode_texture,
    };
    use ara_asset::{Cancellation, CookPreset, ImportContext, TargetPlatform};
    use image::ImageEncoder;
    for (w, h) in [(64, 32), (63, 31)] {
        let image = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 7) as u8, (y * 3) as u8, 128, (x * 13 + y) as u8])
        });
        let mut source = Vec::new();
        image::codecs::png::PngEncoder::new(&mut source)
            .write_image(&image, w, h, image::ExtendedColorType::Rgba8)
            .expect("PNG fixture");
        for preset in [CookPreset::Balanced, CookPreset::HighQuality] {
            let cancellation = Cancellation::default();
            let decoded = decode_texture(
                &source,
                &TextureSettings::default(),
                TargetPlatform::Windows,
                &cancellation,
            )
            .expect("fixture decode");
            let mut pixels = decoded.pixels;
            let (mut width, mut height) = (w, h);
            let mut mips = Vec::new();
            loop {
                let next = if width != 1 || height != 1 {
                    Some(area_weighted(
                        &pixels,
                        width,
                        height,
                        (width / 2).max(1),
                        (height / 2).max(1),
                        decoded.usage,
                    ))
                } else {
                    None
                };
                mips.push((width, height, pixels));
                let Some(next) = next else { break };
                pixels = next;
                width = (width / 2).max(1);
                height = (height / 2).max(1);
            }
            let expected = encode_texture(
                MippedTexture {
                    width: w,
                    height: h,
                    usage: decoded.usage,
                    format: decoded.format,
                    mips,
                },
                preset,
                &cancellation,
                None,
                None,
            )
            .expect("weighted-filter KTX2");
            let actual = cook_texture_with_preset(
                &source,
                &TextureSettings::default(),
                TargetPlatform::Windows,
                preset,
                &mut ImportContext {
                    metrics: None,
                    cancellation: &cancellation,
                    progress: &mut |_| {},
                },
            )
            .expect("specialized-filter KTX2");
            assert_eq!(actual, expected, "{w}x{h} {preset:?}");
        }
    }
}
