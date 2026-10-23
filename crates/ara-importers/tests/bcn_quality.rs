#![allow(clippy::unwrap_used)]
use ara_asset::{Cancellation, CookPreset, ImportContext, TargetPlatform};
use ara_asset_format::{Texture, TextureFormat};
use ara_importers::{TextureSettings, TextureUsage, cook_texture_with_preset};
use image::ImageEncoder;

fn cook(source: &[u8], usage: TextureUsage) -> Vec<u8> {
    cook_preset(source, usage, CookPreset::HighQuality)
}
fn cook_preset(source: &[u8], usage: TextureUsage, preset: CookPreset) -> Vec<u8> {
    cook_texture_with_preset(
        source,
        &TextureSettings {
            usage,
            ..Default::default()
        },
        TargetPlatform::Windows,
        preset,
        &mut ImportContext {
            metrics: None,
            cancellation: &Cancellation::default(),
            progress: &mut |_| {},
        },
    )
    .unwrap()
}
fn psnr(square: f64, count: usize, peak: f64) -> f64 {
    10.0 * (peak * peak / (square / count as f64)).log10()
}

#[test]
fn independent_float_decoders_measure_bc4_bc5_and_hdr_bc6h_quality() {
    let n = 64u32;
    for usage in [TextureUsage::Mask, TextureUsage::Normal] {
        let image = image::RgbaImage::from_fn(n, n, |x, y| {
            if usage == TextureUsage::Mask {
                image::Rgba([((x + y) * 2) as u8, 0, 0, 255])
            } else {
                let nx = (x as f32 - 31.5) / 48.0;
                let ny = (y as f32 - 31.5) / 48.0;
                let nz = (1.0 - nx * nx - ny * ny).sqrt();
                let rgb = [nx, ny, nz].map(|v| ((v * 0.5 + 0.5) * 255.0 + 0.5) as u8);
                image::Rgba([rgb[0], rgb[1], rgb[2], 255])
            }
        });
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&image, n, n, image::ExtendedColorType::Rgba8)
            .unwrap();
        let bytes = cook(&png, usage);
        let texture = Texture::read(&bytes).unwrap();
        let channels = if usage == TextureUsage::Mask { 1 } else { 2 };
        let block_size = texture.format().block_bytes();
        let mut square = 0.0;
        for (block_index, block) in texture
            .levels()
            .next()
            .unwrap()
            .chunks_exact(block_size)
            .enumerate()
        {
            let mut decoded = [0.0f32; 32];
            if usage == TextureUsage::Mask {
                bcdec_rs::bc4_float(block, &mut decoded, 4, false);
            } else {
                bcdec_rs::bc5_float(block, &mut decoded, 8, false);
            }
            let bx = block_index % (n as usize / 4);
            let by = block_index / (n as usize / 4);
            for y in 0..4 {
                for x in 0..4 {
                    let p = image.get_pixel((bx * 4 + x) as u32, (by * 4 + y) as u32);
                    let v = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0 * 2.0 - 1.0);
                    let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                    for c in 0..channels {
                        let reference = if usage == TextureUsage::Mask {
                            p[0] as f64 / 255.0
                        } else {
                            (v[c] / length * 0.5 + 0.5) as f64
                        };
                        square += (reference - decoded[(y * 4 + x) * channels + c] as f64).powi(2);
                    }
                }
            }
        }
        let quality = psnr(square, (n * n) as usize * channels, 1.0);
        println!(
            "{:?}: {quality:.3} dB {}-channel UNORM PSNR",
            texture.format(),
            channels
        );
        assert!(quality > 35.0);
    }
    let image = image::ImageBuffer::from_fn(n, n, |x, y| {
        image::Rgba([
            x as f32 / 8.0,
            y as f32 / 8.0,
            (x + y) as f32 / 16.0,
            1.0f32,
        ])
    });
    let reference = image.clone();
    let mut exr = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba32F(image)
        .write_to(&mut exr, image::ImageFormat::OpenExr)
        .unwrap();
    let exr = exr.into_inner();
    for preset in [
        CookPreset::Fast,
        CookPreset::Balanced,
        CookPreset::HighQuality,
    ] {
        let bytes = cook_preset(&exr, TextureUsage::Hdr, preset);
        let texture = Texture::read(&bytes).unwrap();
        assert_eq!(texture.format(), TextureFormat::Bc6h);
        let mut square = 0.0;
        for (block_index, block) in texture
            .levels()
            .next()
            .unwrap()
            .as_chunks::<16>()
            .0
            .iter()
            .enumerate()
        {
            let mut decoded = [0.0; 48];
            bcdec_rs::bc6h_float(block, &mut decoded, 12, false);
            let bx = block_index % (n as usize / 4);
            let by = block_index / (n as usize / 4);
            for y in 0..4 {
                for x in 0..4 {
                    let p = reference.get_pixel((bx * 4 + x) as u32, (by * 4 + y) as u32);
                    for c in 0..3 {
                        square += (p[c] as f64 - decoded[(y * 4 + x) * 3 + c] as f64).powi(2);
                    }
                }
            }
        }
        let quality = psnr(square, (n * n * 3) as usize, 8.0);
        println!("BC6H {preset:?}: {quality:.3} dB linear RGB HDR PSNR, peak=8");
        assert!(quality > 40.0);
    }
}
