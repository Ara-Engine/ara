//! Cooked Ara textures (KTX2, BC7 or BC5) as Renderlab's prepared RGBA8 mip chains.
//! Renderlab takes uncompressed chains, so each texture is decoded once and its
//! levels are box-filtered here: colour in linear light, normals renormalized.
use anyhow::Result;
use renderlab::asset::{Mip, Texture, TextureRole};

/// Decodes `ktx2` and builds the complete chain for `role`.
pub(crate) fn prepare(ktx2: &[u8], role: TextureRole) -> Result<Texture> {
    let decoded = ara_runtime_render::decode_texture(ktx2)?;
    Ok(chain(decoded.size, decoded.rgba, role))
}

/// The full mip chain, down to 1x1, of tightly packed RGBA8 `top`.
pub(crate) fn chain(size: [u32; 2], top: Vec<u8>, role: TextureRole) -> Texture {
    chain_with(size, top, role, &[])
}

/// `chain`, where the channels in `roots` hold the square root of a linear quantity
/// (`with_glow`'s emissive map): each level averages their squares and takes the root,
/// so a pattern half on and half off stays half as bright down the chain.
fn chain_with(size: [u32; 2], top: Vec<u8>, role: TextureRole, roots: &[usize]) -> Texture {
    let mut mips = vec![Mip {
        width: size[0],
        height: size[1],
        rgba8: top,
    }];
    while let Some(last) = mips.last()
        && (last.width > 1 || last.height > 1)
    {
        let next = halve(last, role, roots);
        mips.push(next);
    }
    Texture { role, mips }
}

/// A material texture with an emissive map packed into channels its role leaves
/// unused. Renderlab materials have three texture slots and none for emission, but an
/// opaque material never reads its base colour's alpha, nor a metallic-roughness
/// texture's red and alpha. `channels` pairs a channel of this texture with the
/// emissive channel it carries, stored as the square root of the linear value so eight
/// bits keep the dark end (`scene`'s emissive-map program squares it back). The map is
/// resampled to the host's size, averaged over each host texel's footprint where the map
/// is the larger; a material without the host texture gets a white one of the map's
/// size. `glow` is the map's finest level: its size and tightly packed sRGB RGBA8
/// texels.
pub(crate) fn with_glow(
    host: Option<&[u8]>,
    glow: ([u32; 2], &[u8]),
    role: TextureRole,
    channels: &[(usize, usize)],
) -> Result<Texture> {
    let (glow_size, glow) = glow;
    let (size, mut rgba) = match host {
        Some(ktx2) => {
            let decoded = ara_runtime_render::decode_texture(ktx2)?;
            (decoded.size, decoded.rgba)
        }
        None => (glow_size, vec![255; glow.len()]),
    };
    let decode: Vec<f32> = (0..=255u8).map(srgb_to_linear).collect();
    for y in 0..size[1] {
        for x in 0..size[0] {
            let at = ((y * size[0] + x) * 4) as usize;
            for (to, from) in channels {
                let linear = footprint(&decode, glow, glow_size, size, [x, y], *from);
                rgba[at + to] = (linear.max(0.).sqrt().min(1.) * 255.).round() as u8;
            }
        }
    }
    let roots: Vec<usize> = channels.iter().map(|(to, _)| *to).collect();
    Ok(chain_with(size, rgba, role, &roots))
}

/// The linear emission of `channel` over target texel `at` of a `size` image: the
/// glow map (`glow_size`, sRGB RGBA8 `glow`, decoded through `decode`) sampled
/// bilinearly at the texel's centre when the map is no larger, and where it is larger
/// the mean of a grid of bilinear taps spread over the texel's footprint, so no texel
/// of the map is skipped. The map wraps, as the sampler repeats.
fn footprint(
    decode: &[f32],
    glow: &[u8],
    glow_size: [u32; 2],
    size: [u32; 2],
    at: [u32; 2],
    channel: usize,
) -> f32 {
    let [width, height] = glow_size.map(i64::from);
    let texel = |x: i64, y: i64| {
        let at = (y.rem_euclid(height) * width + x.rem_euclid(width)) as usize * 4;
        decode[glow[at + channel] as usize]
    };
    let taps =
        [0, 1].map(|axis| (glow_size[axis] as f32 / size[axis] as f32).ceil().max(1.) as u32);
    let mut sum = 0.;
    for j in 0..taps[1] {
        for i in 0..taps[0] {
            // This tap's place in the texel, in map texels (half a texel in from the
            // edge of its share of the footprint).
            let place = |axis: usize, k: u32| {
                (at[axis] as f32 + (k as f32 + 0.5) / taps[axis] as f32) / size[axis] as f32
                    * glow_size[axis] as f32
                    - 0.5
            };
            let (u, v) = (place(0, i), place(1, j));
            let (x0, y0) = (u.floor(), v.floor());
            let (fx, fy) = (u - x0, v - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let top = texel(x0, y0) * (1. - fx) + texel(x0 + 1, y0) * fx;
            let bottom = texel(x0, y0 + 1) * (1. - fx) + texel(x0 + 1, y0 + 1) * fx;
            sum += top * (1. - fy) + bottom * fy;
        }
    }
    sum / (taps[0] * taps[1]) as f32
}

/// Mean linear RGB of sRGB-encoded RGBA8 texels (zero for an empty image).
pub(crate) fn mean_linear(rgba8: &[u8]) -> [f32; 3] {
    let texels = rgba8.as_chunks::<4>().0;
    let mut sum = [0f64; 3];
    for p in texels {
        for (s, c) in sum.iter_mut().zip(p) {
            *s += f64::from(srgb_to_linear(*c));
        }
    }
    let n = texels.len().max(1) as f64;
    sum.map(|s| (s / n) as f32)
}

fn srgb_to_linear(value: u8) -> f32 {
    let v = f32::from(value) / 255.;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub(crate) fn linear_to_srgb(value: f32) -> u8 {
    let v = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1. / 2.4) - 0.055
    };
    (v.clamp(0., 1.) * 255.).round() as u8
}

/// The next level: each texel averages the (up to) 2x2 texels above it.
fn halve(mip: &Mip, role: TextureRole, roots: &[usize]) -> Mip {
    let (width, height) = ((mip.width / 2).max(1), (mip.height / 2).max(1));
    let decode: Vec<f32> = (0..=255u8).map(srgb_to_linear).collect();
    let texel = |x: u32, y: u32| {
        let at = ((y.min(mip.height - 1) * mip.width + x.min(mip.width - 1)) * 4) as usize;
        [
            mip.rgba8[at],
            mip.rgba8[at + 1],
            mip.rgba8[at + 2],
            mip.rgba8[at + 3],
        ]
    };
    let mut rgba8 = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let taps = [
                texel(2 * x, 2 * y),
                texel(2 * x + 1, 2 * y),
                texel(2 * x, 2 * y + 1),
                texel(2 * x + 1, 2 * y + 1),
            ];
            let mean = |channel: usize, value: &dyn Fn(u8) -> f32| {
                taps.iter().map(|t| value(t[channel])).sum::<f32>() / 4.
            };
            let unorm = |v: u8| f32::from(v) / 255.;
            let pack = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
            let mut out = match role {
                TextureRole::BaseColorSrgb => {
                    let linear = |v: u8| decode[v as usize];
                    [
                        linear_to_srgb(mean(0, &linear)),
                        linear_to_srgb(mean(1, &linear)),
                        linear_to_srgb(mean(2, &linear)),
                        pack(mean(3, &unorm)),
                    ]
                }
                role if role.is_normal() => {
                    let signed = |v: u8| f32::from(v) / 127.5 - 1.;
                    let n = [mean(0, &signed), mean(1, &signed), mean(2, &signed)];
                    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    let n = if length > 1e-6 {
                        n.map(|c| c / length)
                    } else {
                        [0., 0., 1.]
                    };
                    [
                        pack(n[0] * 0.5 + 0.5),
                        pack(n[1] * 0.5 + 0.5),
                        pack(n[2] * 0.5 + 0.5),
                        255,
                    ]
                }
                _ => [
                    pack(mean(0, &unorm)),
                    pack(mean(1, &unorm)),
                    pack(mean(2, &unorm)),
                    pack(mean(3, &unorm)),
                ],
            };
            // Square roots average as the root of the mean of their squares.
            for channel in roots {
                let square = |v: u8| unorm(v) * unorm(v);
                out[*channel] = pack(mean(*channel, &square).sqrt());
            }
            rgba8.extend(out);
        }
    }
    Mip {
        width,
        height,
        rgba8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mean_of_an_emissive_map_is_linear() {
        // Half the texels white, half black: half the linear light, not sRGB 128.
        let m = mean_linear(&[255, 255, 255, 255, 0, 0, 0, 255]);
        assert!(m.iter().all(|c| (c - 0.5).abs() < 1e-3), "{m:?}");
        assert_eq!(mean_linear(&[]), [0.; 3]);
    }

    #[test]
    fn an_emissive_map_rides_in_spare_channels_as_square_roots() {
        // Two texels: black, and full red with sRGB 137 (a quarter, linear) green.
        let glow = [0, 0, 0, 255, 255, 137, 0, 255];
        let (role, channels) = (TextureRole::MetallicRoughnessLinear, [(0, 0), (3, 1)]);
        let packed =
            with_glow(None, ([2, 1], &glow), role, &channels).expect("no host texture to decode");
        let top = &packed.mips[0];
        assert_eq!((top.width, top.height), (2, 1));
        // A white host: roughness and metalness scales stay one; red carries the
        // map's red and alpha its green, as square roots (a quarter is a half).
        assert_eq!(top.rgba8[..4], [0, 255, 255, 0]);
        assert_eq!(top.rgba8[4..7], [255, 255, 255]);
        assert!(top.rgba8[7].abs_diff(128) <= 1, "{}", top.rgba8[7]);
    }

    #[test]
    fn a_checkerboard_glow_keeps_its_mean_through_the_chain() {
        // An 8x8 checkerboard of full and no emission: half the linear light at every
        // level. Stored as square roots, the top reads 255 and 0; the last level must
        // read the root of a half (181), not half of the roots (128, a quarter shown).
        let glow: Vec<u8> = (0..64)
            .flat_map(|i| {
                let on = (i % 8 + i / 8) % 2 == 0;
                if on {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 255]
                }
            })
            .collect();
        let role = TextureRole::MetallicRoughnessLinear;
        let texture = with_glow(None, ([8, 8], &glow), role, &[(0, 0), (3, 1)])
            .expect("no host texture to decode");
        for level in texture.mips.iter().skip(1) {
            for texel in level.rgba8.as_chunks::<4>().0 {
                for channel in [0, 3] {
                    let shown = (f32::from(texel[channel]) / 255.).powi(2);
                    assert!(
                        (shown - 0.5).abs() < 0.02,
                        "{}x{}: {shown}",
                        level.width,
                        level.height
                    );
                }
            }
        }
    }

    #[test]
    fn a_glow_larger_than_its_texel_is_averaged_over_the_footprint() {
        // A 16x16 checkerboard read into a 4x4 image: every texel covers an equal share
        // of on and off, so each is half the light. Without the footprint, centre taps
        // would land on one colour.
        let decode: Vec<f32> = (0..=255u8).map(srgb_to_linear).collect();
        let glow: Vec<u8> = (0..256)
            .flat_map(|i| {
                let on = (i % 16 + i / 16) % 2 == 0;
                if on { [255; 4] } else { [0, 0, 0, 255] }
            })
            .collect();
        for y in 0..4 {
            for x in 0..4 {
                let linear = footprint(&decode, &glow, [16, 16], [4, 4], [x, y], 0);
                assert!((linear - 0.5).abs() < 1e-3, "({x}, {y}): {linear}");
            }
        }
    }

    #[test]
    fn chains_reach_one_texel_and_filter_by_role() {
        // 4x2: left half black, right half white.
        let top: Vec<u8> = (0..8)
            .flat_map(|i| if i % 4 < 2 { [0, 0, 0, 255] } else { [255; 4] })
            .collect();
        let colour = chain([4, 2], top.clone(), TextureRole::BaseColorSrgb);
        let sizes: Vec<_> = colour.mips.iter().map(|m| (m.width, m.height)).collect();
        assert_eq!(sizes, [(4, 2), (2, 1), (1, 1)]);
        // Half black, half white averages in linear light: sRGB 188, not 128.
        assert_eq!(colour.mips[2].rgba8[0], 188);
        let data = chain([4, 2], top, TextureRole::MetallicRoughnessLinear);
        assert_eq!(data.mips[2].rgba8[0], 128);
        // Opposed tangent-space tilts average back to a unit +Z normal.
        let normals = chain(
            [2, 1],
            vec![218, 128, 218, 255, 37, 128, 218, 255],
            TextureRole::NormalLinear,
        );
        assert_eq!(normals.mips[1].rgba8[..3], [128, 128, 255]);
    }
}
