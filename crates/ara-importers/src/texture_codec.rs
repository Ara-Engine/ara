//! Fixed block-row tiles keep ISPC SIMD lanes and output ordering independent of
//! Rayon scheduling and thread count. Alpha mode is selected for the entire mip.
use crate::TextureUsage;
use ara_asset::{Cancellation, CookPreset, Result};
use ara_asset_format::TextureFormat;
use rayon::prelude::*;

fn quantize(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}
fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
pub(crate) fn compress(
    pixels: &[[f32; 4]],
    width: u32,
    height: u32,
    usage: TextureUsage,
    format: TextureFormat,
    preset: CookPreset,
    cancellation: &Cancellation,
) -> Result<Vec<u8>> {
    cancellation.check()?;
    if matches!(
        format,
        TextureFormat::Rgba16Float | TextureFormat::Rgba32Float
    ) {
        let mut bytes = vec![0; pixels.len() * format.block_bytes()];
        bytes
            .par_chunks_mut(format.block_bytes())
            .zip(pixels)
            .for_each(|(out, p)| {
                for (c, dst) in p.iter().zip(out.chunks_exact_mut(format.block_bytes() / 4)) {
                    if format == TextureFormat::Rgba16Float {
                        dst.copy_from_slice(&half::f16::from_f32(*c).to_bits().to_le_bytes());
                    } else {
                        dst.copy_from_slice(&c.to_le_bytes());
                    }
                }
            });
        return Ok(bytes);
    }
    let w = width.next_multiple_of(4);
    let h = height.next_multiple_of(4);
    let channels = match usage {
        TextureUsage::Mask => 1,
        TextureUsage::Normal => 2,
        _ => 4,
    };
    let sample_bytes = if usage == TextureUsage::Hdr { 2 } else { 1 };
    let pixel_bytes = channels * sample_bytes;
    let stride = w as usize * pixel_bytes;
    let mut data = vec![0; stride * h as usize];
    data.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.chunks_exact_mut(pixel_bytes).enumerate() {
                let p =
                    pixels[y.min(height as usize - 1) * width as usize + x.min(width as usize - 1)];
                if usage == TextureUsage::Hdr {
                    for (c, dst) in p.into_iter().zip(out.as_chunks_mut::<2>().0) {
                        dst.copy_from_slice(&half::f16::from_f32(c).to_bits().to_le_bytes());
                    }
                } else {
                    for (i, dst) in out.iter_mut().enumerate() {
                        *dst = quantize(match usage {
                            TextureUsage::Color if i < 3 => linear_to_srgb(p[i]),
                            TextureUsage::Normal => p[i] * 0.5 + 0.5,
                            _ => p[i],
                        });
                    }
                }
            }
        });
    let alpha = pixels.par_iter().any(|p| p[3] < 1.0);
    let bc7 = match (preset, alpha) {
        (CookPreset::Fast, false) => intel_tex_2::bc7::opaque_ultra_fast_settings(),
        (CookPreset::Fast, true) => intel_tex_2::bc7::alpha_ultra_fast_settings(),
        (CookPreset::Balanced, false) => intel_tex_2::bc7::opaque_basic_settings(),
        (CookPreset::Balanced, true) => intel_tex_2::bc7::alpha_basic_settings(),
        (CookPreset::HighQuality, false) => intel_tex_2::bc7::opaque_slow_settings(),
        (CookPreset::HighQuality, true) => intel_tex_2::bc7::alpha_slow_settings(),
    };
    let bc6h = match preset {
        CookPreset::Fast => intel_tex_2::bc6h::fast_settings(),
        CookPreset::Balanced => intel_tex_2::bc6h::basic_settings(),
        CookPreset::HighQuality => intel_tex_2::bc6h::very_slow_settings(),
    };
    if matches!(format, TextureFormat::Bc7 | TextureFormat::Bc7Srgb)
        && let Some(blocks) = reuse_solid_blocks(&data, w, h, &bc7, cancellation)?
    {
        return Ok(blocks);
    }
    const TILE_ROWS: usize = 4;
    let block_row_bytes = (w / 4) as usize * format.block_bytes();
    let tile_bytes = block_row_bytes * (TILE_ROWS / 4);
    let mut blocks = vec![0; block_row_bytes * (h / 4) as usize];
    blocks
        .par_chunks_mut(tile_bytes)
        .with_max_len(1)
        .enumerate()
        .try_for_each(|(tile, out)| -> Result<()> {
            cancellation.check()?;
            let y = tile * TILE_ROWS;
            let rows = TILE_ROWS.min(h as usize - y);
            let data = &data[y * stride..(y + rows) * stride];
            match usage {
                TextureUsage::Mask => intel_tex_2::bc4::compress_blocks_into(
                    &intel_tex_2::RSurface {
                        data,
                        width: w,
                        height: rows as u32,
                        stride: stride as u32,
                    },
                    out,
                ),
                TextureUsage::Normal => intel_tex_2::bc5::compress_blocks_into(
                    &intel_tex_2::RgSurface {
                        data,
                        width: w,
                        height: rows as u32,
                        stride: stride as u32,
                    },
                    out,
                ),
                TextureUsage::Hdr => intel_tex_2::bc6h::compress_blocks_into(
                    &bc6h,
                    &intel_tex_2::RgbaSurface {
                        data,
                        width: w,
                        height: rows as u32,
                        stride: stride as u32,
                    },
                    out,
                ),
                _ => intel_tex_2::bc7::compress_blocks_into(
                    &bc7,
                    &intel_tex_2::RgbaSurface {
                        data,
                        width: w,
                        height: rows as u32,
                        stride: stride as u32,
                    },
                    out,
                ),
            }
            Ok(())
        })?;
    Ok(blocks)
}

// Flat areas are frequent in authored albedo/emissive maps (Fox is 94% solid
// blocks). Encode each solid RGBA value once with the exact same ISPC preset,
// rather than spending the slow search on thousands of identical blocks.
// Representatives are assigned in source order; hash table iteration is unused.
fn reuse_solid_blocks(
    data: &[u8],
    w: u32,
    h: u32,
    settings: &intel_tex_2::bc7::EncodeSettings,
    cancellation: &Cancellation,
) -> Result<Option<Vec<u8>>> {
    let columns = w as usize / 4;
    let count = columns * h as usize / 4;
    if count < 64 {
        return Ok(None);
    }
    let stride = w as usize * 4;
    let mut palette = std::collections::HashMap::<[u8; 4], usize>::new();
    let mut representatives = Vec::with_capacity(count);
    let mut remap = Vec::with_capacity(count);
    for block in 0..count {
        if block % columns == 0 {
            cancellation.check()?;
        }
        let offset = block / columns * 4 * stride + block % columns * 16;
        let color = [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ];
        let solid = (0..4).all(|y| {
            data[offset + y * stride..offset + y * stride + 16]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == color)
        });
        let index = if solid {
            *palette.entry(color).or_insert_with(|| {
                let i = representatives.len();
                representatives.push(block);
                i
            })
        } else {
            let i = representatives.len();
            representatives.push(block);
            i
        };
        remap.push(index);
    }
    if count - representatives.len() < count / 4 {
        return Ok(None);
    }
    let unique = representatives.len();
    let packed_columns = columns.next_multiple_of(8).min(128);
    let packed_width = packed_columns * 4;
    let packed_stride = packed_width * 4;
    let packed_height = unique.div_ceil(packed_columns) * 4;
    let mut packed = vec![0; packed_height * packed_stride];
    for (i, block) in representatives.into_iter().enumerate() {
        let src = block / columns * 4 * stride + block % columns * 16;
        let dst = i / packed_columns * 4 * packed_stride + i % packed_columns * 16;
        for y in 0..4 {
            packed[dst + y * packed_stride..dst + y * packed_stride + 16]
                .copy_from_slice(&data[src + y * stride..src + y * stride + 16]);
        }
    }
    let mut encoded = vec![0; unique.next_multiple_of(packed_columns) * 16];
    // Full-width block rows preserve coherent SIMD groups and amortize setup.
    encoded
        .par_chunks_mut(packed_columns * 4 * 16)
        .with_max_len(1)
        .enumerate()
        .try_for_each(|(tile, out)| -> Result<()> {
            cancellation.check()?;
            let rows = out.len() / 16 / packed_columns * 4;
            let start = tile * 16 * packed_stride;
            intel_tex_2::bc7::compress_blocks_into(
                settings,
                &intel_tex_2::RgbaSurface {
                    data: &packed[start..start + rows * packed_stride],
                    width: packed_width as u32,
                    height: rows as u32,
                    stride: packed_stride as u32,
                },
                out,
            );
            Ok(())
        })?;
    let mut result = vec![0; count * 16];
    result
        .par_chunks_mut(16)
        .zip(remap)
        .for_each(|(dst, index)| dst.copy_from_slice(&encoded[index * 16..index * 16 + 16]));
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn solid_reuse_matches_unmodified_ispc_bytes_including_alpha_and_simd_tails() {
        let (w, h) = (36, 68);
        let mut data = vec![128u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = ((y * w + x) * 4) as usize;
                data[p + 3] = if y < 36 { 255 } else { 127 };
                if x < 4 {
                    data[p] = (y * 3) as u8;
                }
            }
        }
        for settings in [
            intel_tex_2::bc7::alpha_ultra_fast_settings(),
            intel_tex_2::bc7::alpha_basic_settings(),
            intel_tex_2::bc7::alpha_slow_settings(),
            intel_tex_2::bc7::opaque_slow_settings(),
        ] {
            let reference = intel_tex_2::bc7::compress_blocks(
                &settings,
                &intel_tex_2::RgbaSurface {
                    data: &data,
                    width: w,
                    height: h,
                    stride: w * 4,
                },
            );
            let reused = reuse_solid_blocks(&data, w, h, &settings, &Cancellation::default())
                .expect("valid encode")
                .expect("many repeated blocks");
            assert_eq!(reused, reference);
        }
    }
}
