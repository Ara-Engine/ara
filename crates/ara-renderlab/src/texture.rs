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
    let mut mips = vec![Mip {
        width: size[0],
        height: size[1],
        rgba8: top,
    }];
    while let Some(last) = mips.last()
        && (last.width > 1 || last.height > 1)
    {
        let next = halve(last, role);
        mips.push(next);
    }
    Texture { role, mips }
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
fn halve(mip: &Mip, role: TextureRole) -> Mip {
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
            let out = match role {
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
