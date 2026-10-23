use crate::*;
use anyhow::{Result, ensure};
use ara_asset_format::{Texture, TextureFormat};

/// Complete CPU mip chain, prepared off the interactive frame thread.
pub struct Source {
    pub(crate) size: [u32; 2],
    pub(crate) levels: Vec<Vec<u8>>,
    pub(crate) srgb: bool,
    pub(crate) priority: u32,
}
impl Source {
    /// Builds box-filtered mips in linear light for color, linear for data.
    /// Repeat addressing matches the page gutters and the shared shader helper.
    pub fn rgba8(size: [u32; 2], rgba: Vec<u8>, srgb: bool, priority: u32) -> Result<Self> {
        ensure!(
            !size.contains(&0)
                && size.iter().all(|v| *v <= 32768)
                && rgba.len() as u64 == u64::from(size[0]) * u64::from(size[1]) * 4,
            "invalid texture source"
        );
        let mut levels = vec![rgba];
        let mut extent = size;
        while extent != [1, 1] {
            let next = extent.map(|v| (v / 2).max(1));
            let input = &levels[levels.len() - 1];
            let mut mip = vec![0; (next[0] * next[1] * 4) as usize];
            for y in 0..next[1] {
                for x in 0..next[0] {
                    let lo = [x * extent[0] / next[0], y * extent[1] / next[1]];
                    let hi = [(x + 1) * extent[0] / next[0], (y + 1) * extent[1] / next[1]];
                    for channel in 0..4 {
                        let mut sum = 0.;
                        let mut count = 0.;
                        for iy in lo[1]..hi[1] {
                            for ix in lo[0]..hi[0] {
                                let v = f32::from(
                                    input[((iy * extent[0] + ix) * 4 + channel) as usize],
                                ) / 255.;
                                sum += if srgb && channel < 3 { decode(v) } else { v };
                                count += 1.;
                            }
                        }
                        let v = sum / count;
                        mip[((y * next[0] + x) * 4 + channel) as usize] =
                            ((if srgb && channel < 3 { encode(v) } else { v }) * 255.).round()
                                as u8;
                    }
                }
            }
            levels.push(mip);
            extent = next;
        }
        Ok(Self {
            size,
            levels,
            srgb,
            priority,
        })
    }
    /// Decodes a validated cooked BC7 color or BC4/BC5 data KTX2 into virtual
    /// pages. Disk payloads remain compressed; only resident GPU tiles are RGBA8.
    pub fn from_cooked(bytes: &[u8], priority: u32) -> Result<Self> {
        let t = Texture::read(bytes)?;
        let size = t.dimensions();
        let srgb = matches!(t.format(), TextureFormat::Bc7Srgb);
        let mut levels = Vec::new();
        for (mip, input) in t.levels().enumerate() {
            let extent = size.map(|v| (v >> mip).max(1));

            let mut pixels = vec![0u32; (extent[0] * extent[1]) as usize];
            let result = match t.format() {
                TextureFormat::Bc4 => texture2ddecoder::decode_bc4(
                    input,
                    extent[0] as usize,
                    extent[1] as usize,
                    &mut pixels,
                ),
                TextureFormat::Bc5 => texture2ddecoder::decode_bc5(
                    input,
                    extent[0] as usize,
                    extent[1] as usize,
                    &mut pixels,
                ),
                TextureFormat::Bc7 | TextureFormat::Bc7Srgb => texture2ddecoder::decode_bc7(
                    input,
                    extent[0] as usize,
                    extent[1] as usize,
                    &mut pixels,
                ),
                _ => anyhow::bail!("virtual texture color source requires BC4, BC5 or BC7"),
            };
            result.map_err(anyhow::Error::msg)?;
            levels.push(
                pixels
                    .into_iter()
                    .flat_map(|p| {
                        [
                            ((p >> 16) & 255) as u8,
                            ((p >> 8) & 255) as u8,
                            (p & 255) as u8,
                            (p >> 24) as u8,
                        ]
                    })
                    .collect(),
            );
        }
        ensure!(
            levels.len() == size[0].max(size[1]).ilog2() as usize + 1,
            "streamed texture needs a complete mip chain"
        );
        Ok(Self {
            size,
            levels,
            srgb,
            priority,
        })
    }
}
pub(crate) fn decode(v: f32) -> f32 {
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

pub(crate) struct Page {
    pub texture: usize,
    pub mip: u32,
    pub xy: [u32; 2],
    pub parent: Option<u32>,
    pub pinned: bool,
}
/// Stable virtual addresses and immutable source mip chains for an extraction.
pub struct Catalog {
    pub(crate) sources: Vec<Source>,
    pub(crate) pages: Vec<Page>,
    pub(crate) descriptors: Vec<[u32; 8]>,
}
impl Catalog {
    /// Assigns deterministic mip-major virtual page IDs to all source textures.
    pub fn new(sources: Vec<Source>) -> Result<Self> {
        ensure!(!sources.is_empty(), "empty virtual texture catalog");
        let mut pages = Vec::new();
        let mut descriptors = Vec::new();
        for (texture, source) in sources.iter().enumerate() {
            let first = u32::try_from(pages.len())?;
            let mut bases = Vec::new();
            let mut extent = source.size;
            for mip in 0..source.levels.len() as u32 {
                bases.push(pages.len() as u32);
                let side = extent.map(|v| v.div_ceil(PAGE_SIZE));
                for y in 0..side[1] {
                    for x in 0..side[0] {
                        pages.push(Page {
                            texture,
                            mip,
                            xy: [x, y],
                            parent: None,
                            pinned: side == [1, 1],
                        });
                    }
                }
                extent = extent.map(|v| (v / 2).max(1));
            }
            for page in &mut pages[first as usize..] {
                if page.mip as usize + 1 < bases.len() {
                    let width = (source.size[0] >> (page.mip + 1))
                        .max(1)
                        .div_ceil(PAGE_SIZE);
                    let height = (source.size[1] >> (page.mip + 1))
                        .max(1)
                        .div_ceil(PAGE_SIZE);
                    page.parent = Some(
                        bases[page.mip as usize + 1]
                            + (page.xy[1] / 2).min(height - 1) * width
                            + (page.xy[0] / 2).min(width - 1),
                    );
                }
            }
            descriptors.push([
                source.size[0],
                source.size[1],
                first,
                source.levels.len() as u32,
                u32::from(source.srgb),
                source.priority,
                0,
                0,
            ]);
        }
        ensure!(
            pages.len() < 16_000_000,
            "virtual page table exceeds supported range"
        );
        Ok(Self {
            sources,
            pages,
            descriptors,
        })
    }
    /// Number of virtual pages, including pinned mip tails.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    /// Extracts a wrapped tile with a one-texel border, suitable for bilinear reads.
    pub fn page(&self, id: u32) -> Result<Vec<u8>> {
        let p = self
            .pages
            .get(id as usize)
            .ok_or_else(|| anyhow::anyhow!("unknown texture page"))?;
        let source = &self.sources[p.texture];
        let size = source.size.map(|v| (v >> p.mip).max(1));
        let input = &source.levels[p.mip as usize];
        let mut bytes = vec![0; PAGE_BYTES as usize];
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let ix =
                    (i64::from(p.xy[0] * PAGE_SIZE + x) - 1).rem_euclid(i64::from(size[0])) as u32;
                let iy =
                    (i64::from(p.xy[1] * PAGE_SIZE + y) - 1).rem_euclid(i64::from(size[1])) as u32;
                let from = ((iy * size[0] + ix) * 4) as usize;
                let to = ((y * TILE_SIZE + x) * 4) as usize;
                bytes[to..to + 4].copy_from_slice(&input[from..from + 4]);
            }
        }
        Ok(bytes)
    }
}
