//! CPU startup mip pyramid packed in one portable single-level image.
use super::*;

/// The backend currently exposes single-level images. Pack decreasing rectangles
/// vertically; shader sampling wraps within each rectangle, including bilinear taps.
pub(super) fn texture(
    gpu: &mut VulkanDevice,
    texture: &ara_render::scene::Texture,
) -> Result<Image> {
    let [width, height] = texture.size;
    let mut atlas = vec![0_u8; (width * height * 2 * 4) as usize];
    let mut level = texture.rgba.to_vec();
    let (mut w, mut h, mut y) = (width, height, 0);
    loop {
        for row in 0..h {
            let start = ((y + row) * width * 4) as usize;
            atlas[start..start + (w * 4) as usize]
                .copy_from_slice(&level[(row * w * 4) as usize..((row + 1) * w * 4) as usize]);
        }
        y += h;
        if w == 1 || h == 1 {
            break;
        }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = Vec::with_capacity((nw * nh * 4) as usize);
        for j in 0..nh {
            for i in 0..nw {
                for c in 0..4 {
                    let mut sum = 0.;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let v = f32::from(
                                level[(((j * 2 + dy).min(h - 1) * w + (i * 2 + dx).min(w - 1)) * 4
                                    + c) as usize],
                            ) / 255.;
                            sum += if texture.srgb && c < 3 { decode(v) } else { v };
                        }
                    }
                    let v = sum * 0.25;
                    next.push(
                        ((if texture.srgb && c < 3 { encode(v) } else { v }) * 255.).round() as u8,
                    );
                }
            }
        }
        level = next;
        w = nw;
        h = nh;
    }
    upload_image(
        gpu,
        [width, height * 2],
        if texture.srgb {
            vk::Format::R8G8B8A8_SRGB
        } else {
            vk::Format::R8G8B8A8_UNORM
        },
        &atlas,
    )
}
/// Decode the same sRGB transfer used by uploaded material textures.
pub(super) fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}
