//! Quantize exactly once on the CPU; GPU vendors receive identical integer data.
use super::{CookPreset, Result, TextureUsage};
use ara_asset::{BlockEncoder, Cancellation};
use ara_asset_format::TextureFormat;
use rayon::prelude::*;

pub(super) fn encode(
    mips: &[(u32, u32, Vec<[f32; 4]>)],
    usage: TextureUsage,
    format: TextureFormat,
    preset: CookPreset,
    cancellation: &Cancellation,
    encoder: &dyn BlockEncoder,
) -> Result<Vec<Vec<u8>>> {
    let words_per_block = if format == TextureFormat::Bc6h {
        32
    } else {
        16
    };
    let counts: Vec<_> = mips
        .iter()
        .map(|(w, h, _)| w.div_ceil(4) as usize * h.div_ceil(4) as usize)
        .collect();
    let mut words = vec![0u32; counts.iter().sum::<usize>() * words_per_block];
    let mut remaining = words.as_mut_slice();
    for ((width, height, pixels), &count) in mips.iter().zip(&counts) {
        cancellation.check()?;
        let (level, rest) = remaining.split_at_mut(count * words_per_block);
        remaining = rest;
        let blocks_wide = width.div_ceil(4) as usize;
        level
            .par_chunks_mut(words_per_block)
            .enumerate()
            .try_for_each(|(index, block)| {
                cancellation.check()?;
                for pixel in 0..16 {
                    let x = ((index % blocks_wide) * 4 + pixel % 4).min(*width as usize - 1);
                    let y = ((index / blocks_wide) * 4 + pixel / 4).min(*height as usize - 1);
                    let p = pixels[y * *width as usize + x];
                    if format == TextureFormat::Bc6h {
                        let bits = p.map(|v| half::f16::from_f32(v.clamp(0.0, 65504.0)).to_bits());
                        block[pixel * 2] = u32::from(bits[0]) | (u32::from(bits[1]) << 16);
                        block[pixel * 2 + 1] = u32::from(bits[2]);
                    } else {
                        block[pixel] =
                            p.into_iter().enumerate().fold(0, |word, (channel, value)| {
                                let value = match usage {
                                    TextureUsage::Color if channel < 3 => {
                                        crate::texture_codec::linear_to_srgb(value)
                                    }
                                    TextureUsage::Normal => value * 0.5 + 0.5,
                                    _ => value,
                                };
                                word | (u32::from(crate::texture_codec::quantize(value))
                                    << (channel * 8))
                            });
                    }
                }
                Ok::<(), ara_asset::Error>(())
            })?;
    }
    // One call for the entire mip chain; no separate device setup/fence per mip.
    let encoded = encoder.encode(format, preset, &words, cancellation)?;
    let expected = counts.iter().sum::<usize>() * format.block_bytes();
    if encoded.len() != expected {
        return Err(ara_asset::Error::EncoderUnavailable(format!(
            "encoder returned {} bytes, expected {expected}",
            encoded.len()
        )));
    }
    let mut offset = 0;
    Ok(counts
        .into_iter()
        .map(|count| {
            let bytes = count * format.block_bytes();
            let level = encoded[offset..offset + bytes].to_vec();
            offset += bytes;
            level
        })
        .collect())
}
