//! Preserve sample bits while expanding eight-bit color and normals once.
use super::{TextureSettings, TextureUsage, invalid, srgb_to_linear};
use ara_asset::{Cancellation, Result};
use image::{ColorType, DynamicImage};
use rayon::prelude::*;

/// HDR/data and wider samples retain their path. Tiny images avoid table setup.
pub(super) fn prepare(
    image: DynamicImage,
    usage: TextureUsage,
    settings: &TextureSettings,
    cancellation: &Cancellation,
) -> Result<Vec<[f32; 4]>> {
    cancellation.check()?;
    if image.width().saturating_mul(image.height()) >= 256 {
        let optimized = match usage {
            TextureUsage::Color => color8(&image, cancellation),
            TextureUsage::Normal => normal8(&image, settings, cancellation),
            _ => None,
        };
        if let Some(pixels) = optimized {
            return pixels;
        }
    }
    let mut pixels: Vec<[f32; 4]> =
        bytemuck::allocation::try_cast_vec(image.into_rgba32f().into_raw())
            .unwrap_or_else(|(_, data)| data.as_chunks::<4>().0.to_vec());
    pixels.par_iter_mut().try_for_each(|p| -> Result<()> {
        cancellation.check()?;
        if p.iter().any(|x| !x.is_finite()) {
            return Err(invalid("nonfinite image sample"));
        }
        match usage {
            TextureUsage::Color => {
                for c in &mut p[..3] {
                    *c = srgb_to_linear(*c);
                }
            }
            TextureUsage::Normal => {
                let n = crate::mesh::normalize([
                    p[0] * 2.0 - 1.0,
                    (p[1] * 2.0 - 1.0) * if settings.flip_normal_y { -1.0 } else { 1.0 },
                    p[2] * 2.0 - 1.0,
                ]);
                p[..3].copy_from_slice(&n);
            }
            TextureUsage::Mask => p[0] = p[settings.mask_channel as usize],
            TextureUsage::Hdr | TextureUsage::Linear | TextureUsage::Lut => {}
        }
        Ok(())
    })?;
    Ok(pixels)
}

fn color8(image: &DynamicImage, cancellation: &Cancellation) -> Option<Result<Vec<[f32; 4]>>> {
    let (channels, ramp) = ramp8(image)?;
    let linear: [f32; 256] = std::array::from_fn(|i| srgb_to_linear(ramp.as_raw()[i * 4]));
    let alpha: [f32; 256] = std::array::from_fn(|i| ramp.as_raw()[i * 4 + 3]);
    // Keep an indexed destination: collecting Result<Vec<_>> would discard the
    // known length and add temporary vectors/copies for large images.
    let mut pixels = vec![[0.0; 4]; image.as_bytes().len() / channels];
    let result = pixels
        .par_iter_mut()
        .zip(image.as_bytes().par_chunks_exact(channels))
        .try_for_each(|(output, p)| {
            cancellation.check()?;
            let rgb = if channels <= 2 {
                [linear[p[0] as usize]; 3]
            } else {
                [
                    linear[p[0] as usize],
                    linear[p[1] as usize],
                    linear[p[2] as usize],
                ]
            };
            let a = if channels == 2 || channels == 4 {
                alpha[p[channels - 1] as usize]
            } else {
                1.0
            };
            *output = [rgb[0], rgb[1], rgb[2], a];
            Ok(())
        });
    Some(result.map(|()| pixels))
}

fn normal8(
    image: &DynamicImage,
    settings: &TextureSettings,
    cancellation: &Cancellation,
) -> Option<Result<Vec<[f32; 4]>>> {
    let (channels, ramp) = ramp8(image)?;
    let component: [f32; 256] = std::array::from_fn(|i| ramp.as_raw()[i * 4] * 2.0 - 1.0);
    let y = component.map(|v| v * if settings.flip_normal_y { -1.0 } else { 1.0 });
    let alpha: [f32; 256] = std::array::from_fn(|i| ramp.as_raw()[i * 4 + 3]);
    let mut pixels = vec![[0.0; 4]; image.as_bytes().len() / channels];
    let result = pixels
        .par_iter_mut()
        .zip(image.as_bytes().par_chunks_exact(channels))
        .try_for_each(|(output, p)| -> Result<()> {
            cancellation.check()?;
            let v = if channels <= 2 {
                [
                    component[p[0] as usize],
                    y[p[0] as usize],
                    component[p[0] as usize],
                ]
            } else {
                [
                    component[p[0] as usize],
                    y[p[1] as usize],
                    component[p[2] as usize],
                ]
            };
            // Keep the original sum, sqrt and divisions: reassociation changes
            // normal bits and therefore mip chains and BC5 artifacts.
            let n = crate::mesh::normalize(v);
            let a = if channels == 2 || channels == 4 {
                alpha[p[channels - 1] as usize]
            } else {
                1.0
            };
            *output = [n[0], n[1], n[2], a];
            Ok(())
        });
    Some(result.map(|()| pixels))
}

fn ramp8(image: &DynamicImage) -> Option<(usize, image::Rgba32FImage)> {
    // Use image's actual conversion for each layout, not an assumed u8/255
    // formula: the compiled grayscale path rounds differently from RGB/RGBA.
    let (channels, ramp) = match image.color() {
        ColorType::L8 => (
            1,
            DynamicImage::ImageLuma8(image::GrayImage::from_fn(256, 1, |x, _| {
                image::Luma([x as u8])
            })),
        ),
        ColorType::La8 => (
            2,
            DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_fn(256, 1, |x, _| {
                image::LumaA([x as u8; 2])
            })),
        ),
        ColorType::Rgb8 => (
            3,
            DynamicImage::ImageRgb8(image::RgbImage::from_fn(256, 1, |x, _| {
                image::Rgb([x as u8; 3])
            })),
        ),
        ColorType::Rgba8 => (
            4,
            DynamicImage::ImageRgba8(image::RgbaImage::from_fn(256, 1, |x, _| {
                image::Rgba([x as u8; 4])
            })),
        ),
        _ => return None,
    };
    Some((channels, ramp.into_rgba32f()))
}

#[cfg(test)]
mod normal_tests;
#[cfg(test)]
mod tests;
