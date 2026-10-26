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
