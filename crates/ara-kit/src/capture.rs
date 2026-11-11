//! Headless captures: a stillkit [`Image`] (premultiplied BGRA) as PNG bytes, for the
//! editor's and Hub's offscreen verification routes and tests.
use stillkit::Image;

/// A premultiplied BGRA snapshot as straight-alpha RGBA bytes.
pub fn rgba(image: &Image) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(image.pixels.len());
    for &[b, g, r, a] in image.pixels.as_chunks::<4>().0 {
        let un = |c: u8| {
            if a == 0 {
                0
            } else {
                ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[un(r), un(g), un(b), a]);
    }
    rgba
}

/// Encodes a premultiplied BGRA snapshot as a straight-alpha RGBA PNG.
pub fn png(image: &Image) -> anyhow::Result<Vec<u8>> {
    let rgba = rgba(image);
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(
        &rgba,
        image.width,
        image.height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

use image::ImageEncoder as _;
