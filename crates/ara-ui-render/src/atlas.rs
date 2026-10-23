//! Persistent shelf atlas with cached Swash raster glyphs and scalable outline distance fields.
use crate::{
    gpu::{Pipeline, TextureUpload},
    msdf,
};
use ara_ui::Result;
use ara_ui_text::FontAsset;
use std::collections::BTreeMap;
/// Glyph cache policy, selected per UI surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum TextMode {
    /// Hinted raster coverage at the actual small-screen font size.
    #[default]
    Raster,
    /// 48 px outline MSDF scaled to the target glyph size.
    Msdf,
}
#[derive(Clone, Copy)]
pub(crate) struct Entry {
    pub texture: u32,
    pub uv: [f32; 4],
    pub offset: [f32; 2],
    pub size: [f32; 2],
    pub pipeline: Pipeline,
    pub range: f32,
}
struct Page {
    pixels: Vec<u8>,
    x: u32,
    y: u32,
    row: u32,
    dirty: bool,
}
/// Glyph cache and RGBA atlas pages. Pages never move while the renderer is alive.
pub struct GlyphAtlas {
    size: u32,
    pages: Vec<Page>,
    cache: BTreeMap<(u64, u32, u32, u32, TextMode), Option<Entry>>,
    scale: swash::scale::ScaleContext,
}
impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new(1024).expect("default atlas size is valid")
    }
}
impl GlyphAtlas {
    /// Create an atlas; dimensions must fit nonempty glyphs and bounded allocations.
    pub fn new(size: u32) -> Result<Self> {
        if !(64..=4096).contains(&size) {
            return Err("ARA7050: atlas size must be 64–4096".into());
        }
        Ok(Self {
            size,
            pages: Vec::new(),
            cache: BTreeMap::new(),
            scale: swash::scale::ScaleContext::new(),
        })
    }
    /// Discard device resources and glyph handles after device loss; the next frame repopulates.
    pub fn reset(&mut self) {
        self.pages.clear();
        self.cache.clear();
    }
    pub(crate) fn glyph(
        &mut self,
        font: &FontAsset,
        id: u32,
        size: f32,
        mode: TextMode,
    ) -> Result<Option<Entry>> {
        if !size.is_finite() || size <= 0.0 || size > 1024.0 || id > u16::MAX as u32 {
            return Err("ARA7050: invalid glyph request".into());
        }
        let key = (font.source_id, font.index, id, size.to_bits(), mode);
        if let Some(entry) = self.cache.get(&key) {
            return Ok(*entry);
        }
        let font = swash::FontRef::from_index(&font.bytes, font.index as usize)
            .ok_or("ARA7050: invalid font")?;
        let mut scaler = self
            .scale
            .builder(font)
            .size(if mode == TextMode::Msdf { 48.0 } else { size })
            .hint(mode == TextMode::Raster)
            .build();
        let raster = swash::scale::Render::new(&[
            swash::scale::Source::ColorOutline(0),
            swash::scale::Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
            swash::scale::Source::Outline,
        ])
        .format(swash::zeno::Format::Alpha)
        .render(&mut scaler, id as u16);
        let Some(image) = raster else {
            self.cache.insert(key, None);
            return Ok(None);
        };
        let color = image.content != swash::scale::image::Content::Mask;
        let (pixels, width, height, left, top, pipeline, range, ratio) =
            if mode == TextMode::Msdf && !color {
                let outline = scaler
                    .scale_outline(id as u16)
                    .ok_or("ARA7050: scalable glyph has no outline")?;
                let image = msdf::generate(&outline, 4.0)?;
                (
                    image.pixels,
                    image.width,
                    image.height,
                    image.left,
                    image.top,
                    Pipeline::Msdf,
                    4.0,
                    size / 48.0,
                )
            } else {
                let p = image.placement;
                let mut pixels = Vec::with_capacity((p.width * p.height * 4) as usize);
                if color {
                    pixels.extend_from_slice(&image.data);
                } else {
                    for a in image.data {
                        pixels.extend_from_slice(&[255, 255, 255, a]);
                    }
                }
                (
                    pixels,
                    p.width,
                    p.height,
                    p.left,
                    p.top,
                    if color {
                        Pipeline::ColorGlyph
                    } else {
                        Pipeline::Mask
                    },
                    0.0,
                    if mode == TextMode::Msdf {
                        size / 48.0
                    } else {
                        1.0
                    },
                )
            };
        if width == 0 || height == 0 {
            self.cache.insert(key, None);
            return Ok(None);
        }
        let (page, x, y) = self.insert(width, height, &pixels)?;
        let entry = Entry {
            texture: 0x8000_0000 | page as u32,
            uv: [
                x as f32 / self.size as f32,
                y as f32 / self.size as f32,
                width as f32 / self.size as f32,
                height as f32 / self.size as f32,
            ],
            offset: [left as f32 * ratio, -(top as f32) * ratio],
            size: [width as f32 * ratio, height as f32 * ratio],
            pipeline,
            range,
        };
        self.cache.insert(key, Some(entry));
        Ok(Some(entry))
    }
    fn insert(&mut self, w: u32, h: u32, pixels: &[u8]) -> Result<(usize, u32, u32)> {
        if w + 2 > self.size || h + 2 > self.size {
            return Err("ARA7050: glyph exceeds atlas page; select a larger atlas".into());
        }
        let size = self.size;
        for page in &mut self.pages {
            if page.x + w + 1 > size {
                page.x = 1;
                page.y += page.row + 1;
                page.row = 0;
            }
        }
        let page = self
            .pages
            .iter()
            .position(|p| p.y + h < size)
            .unwrap_or_else(|| {
                self.pages.push(Page {
                    pixels: vec![0; (size * size * 4) as usize],
                    x: 1,
                    y: 1,
                    row: 0,
                    dirty: true,
                });
                self.pages.len() - 1
            });
        let p = &mut self.pages[page];
        let (x, y) = (p.x, p.y);
        for row in 0..h {
            let dest = (((y + row) * size + x) * 4) as usize;
            let start = (row * w * 4) as usize;
            p.pixels[dest..dest + (w * 4) as usize]
                .copy_from_slice(&pixels[start..start + (w * 4) as usize]);
        }
        p.x += w + 1;
        p.row = p.row.max(h);
        p.dirty = true;
        Ok((page, x, y))
    }
    pub(crate) fn uploads(&mut self, gpu: &mut impl crate::gpu::UiGpu) -> Result<()> {
        for (i, p) in self.pages.iter_mut().enumerate() {
            if p.dirty {
                gpu.upload(TextureUpload {
                    id: 0x8000_0000 | i as u32,
                    width: self.size,
                    height: self.size,
                    rgba: &p.pixels,
                })?;
                p.dirty = false;
            }
        }
        Ok(())
    }
    /// Resident atlas memory, including transparent gutters.
    pub fn bytes(&self) -> usize {
        self.pages.len() * (self.size * self.size * 4) as usize
    }
}
