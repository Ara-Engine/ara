use super::TextureUsage;
use rayon::prelude::*;

/// Filters one level without changing the original weighted accumulation order.
pub(super) fn downsample(
    pixels: &[[f32; 4]],
    w: u32,
    h: u32,
    nw: u32,
    nh: u32,
    usage: TextureUsage,
) -> Vec<[f32; 4]> {
    // Power-of-two half-size coordinates are exact in the weighted filter, with
    // four unit weights. Other sizes (even some even NPOT sizes) can round its
    // x*w/nw bounds, so keep their original area calculation.
    if usage != TextureUsage::Color
        || w < 2
        || h < 2
        || !w.is_power_of_two()
        || !h.is_power_of_two()
        || nw != w / 2
        || nh != h / 2
    {
        return area_weighted(pixels, w, h, nw, nh, usage);
    }
    let mut output = vec![[0.; 4]; (nw * nh) as usize];
    output
        .par_chunks_exact_mut(nw as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let a = y * 2 * w as usize;
            for (x, out) in row.iter_mut().enumerate() {
                let a = a + x * 2;
                *out = box_color([
                    pixels[a],
                    pixels[a + 1],
                    pixels[a + w as usize],
                    pixels[a + w as usize + 1],
                ]);
            }
        });
    output
}

/// Share of `pixels` whose alpha passes an alpha test at `cutoff`.
pub(super) fn coverage(pixels: &[[f32; 4]], cutoff: f32) -> f32 {
    let passing = pixels.iter().filter(|p| p[3] >= cutoff).count();
    passing as f32 / pixels.len().max(1) as f32
}

/// Remaps a mip's alpha so `share` of its texels pass the alpha test at `cutoff`,
/// as the full-size image's do (Castaño 2010, both ways). Averaging a thin grate's
/// alpha drops whole blocks below the cutoff with distance, and averaging a grate's
/// holes (a ceiling grating's, a fence's) closes them: it would stand solid where it
/// should show what lies behind. The remap is monotonic and piecewise linear about the
/// alpha that should land on the cutoff, and keeps 0 and 1 where they are: only
/// partial texels move, so solid texels never sit near the cutoff for compression
/// and filtering error to punch holes through. Partial alphas first shift by up to
/// 1e-4 in a 4x4 ordered-dither pattern over the level (`width` texels a row), so a
/// regular grate, whose averages all tie, splits evenly across its area.
pub(super) fn preserve_coverage(pixels: &mut [[f32; 4]], width: usize, cutoff: f32, share: f32) {
    const BAYER: [f32; 16] = [
        0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
    ];
    let n = pixels.len();
    let kept = ((share * n as f32).round() as usize).min(n);
    let passing = (coverage(pixels, cutoff) * n as f32).round() as usize;
    if kept == 0 || kept == n || passing == kept {
        return;
    }
    for (i, p) in pixels.iter_mut().enumerate() {
        if p[3] > 0. && p[3] < 1. {
            let (x, y) = (i % width.max(1), i / width.max(1));
            p[3] = (p[3] + (BAYER[(y % 4) * 4 + x % 4] / 16. - 0.5) * 1e-4).clamp(1e-6, 1. - 1e-6);
        }
    }
    let mut alphas: Vec<f32> = pixels.iter().map(|p| p[3]).collect();
    alphas.sort_unstable_by(|a, b| b.total_cmp(a));
    // The kept-th largest alpha lands on the cutoff; the next one just under it.
    let (last, next) = (alphas[kept - 1], alphas[kept]);
    if last <= 0. || next >= 1. || last <= next {
        // Ties at 0 or 1 (or at the boundary) cannot be split by a remap.
        return;
    }
    let pivot = (last + next) * 0.5;
    for p in pixels {
        let a = p[3];
        p[3] = if a <= pivot {
            a / pivot * cutoff
        } else {
            cutoff + (a - pivot) / (1. - pivot) * (1. - cutoff)
        };
    }
}

fn box_color(pixels: [[f32; 4]; 4]) -> [f32; 4] {
    let mut sum = [0.; 4];
    for p in pixels {
        for i in 0..3 {
            sum[i] += p[i] * p[3];
        }
        sum[3] += p[3];
    }
    for i in 0..3 {
        sum[i] = if sum[3] > 1e-20 { sum[i] / sum[3] } else { 0. };
    }
    sum[3] /= 4.;
    sum
}

fn area_weighted(
    pixels: &[[f32; 4]],
    w: u32,
    h: u32,
    nw: u32,
    nh: u32,
    usage: TextureUsage,
) -> Vec<[f32; 4]> {
    (0..nw * nh)
        .into_par_iter()
        .map(|index| {
            let y = index / nw;
            let x = index % nw;
            let x0 = x as f32 * w as f32 / nw as f32;
            let x1 = (x + 1) as f32 * w as f32 / nw as f32;
            let y0 = y as f32 * h as f32 / nh as f32;
            let y1 = (y + 1) as f32 * h as f32 / nh as f32;
            let mut sum = [0.0; 4];
            let mut hdr_sum = [0.0f64; 4];
            let mut weight = 0.0;
            for sy in y0.floor() as u32..(y1.ceil() as u32).min(h) {
                for sx in x0.floor() as u32..(x1.ceil() as u32).min(w) {
                    let area = (x1.min((sx + 1) as f32) - x0.max(sx as f32))
                        * (y1.min((sy + 1) as f32) - y0.max(sy as f32));
                    let p = pixels[(sy * w + sx) as usize];
                    for i in 0..4 {
                        if usage == TextureUsage::Hdr {
                            hdr_sum[i] += p[i] as f64 * area as f64;
                            continue;
                        }
                        sum[i] += p[i]
                            * area
                            * if usage == TextureUsage::Color && i < 3 {
                                p[3]
                            } else {
                                1.0
                            };
                    }
                    weight += area;
                }
            }
            if usage == TextureUsage::Hdr {
                sum = hdr_sum.map(|v| (v / weight as f64) as f32);
            } else if usage == TextureUsage::Color {
                for i in 0..3 {
                    sum[i] = if sum[3] > 1e-20 { sum[i] / sum[3] } else { 0.0 };
                }
                sum[3] /= weight;
            } else {
                for c in &mut sum {
                    *c /= weight;
                }
            }
            if usage == TextureUsage::Normal {
                let n = crate::mesh::normalize([sum[0], sum[1], sum[2]]);
                sum[..3].copy_from_slice(&n);
            }
            sum
        })
        .collect()
}

#[cfg(test)]
mod tests;
