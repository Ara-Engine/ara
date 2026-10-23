//! Bounded GPU tile pages with an always-resident coarse fallback.
use crate::sample::LandscapeData;
use anyhow::{Result, ensure};
use ara_terrain::{Tile, TileCoord};

/// Maximum detailed pages; memory does not grow with world extent.
pub const PAGE_CAPACITY: u32 = 256;
/// Fine sample stride of the always-resident coarse representation.
pub const COARSE_STEP: u32 = 4;
/// Maximum tile payloads published by a frame (13 KiB with 32-cell tiles).
pub const UPLOAD_BUDGET: usize = 1;

/// CPU ownership of immutable GPU pages. Replacements are visible only after a
/// matching upload pass; stale worker completions are rejected by the host.
pub(crate) struct Pages {
    pub words: Vec<u32>,
    pub owners: Vec<Option<TileCoord>>,
    pub tiles: u32,
    pub cells: u32,
    pub next: usize,
}
impl Pages {
    pub fn new(data: &LandscapeData, eye: [f32; 2]) -> Result<Self> {
        let tiles = data.header.max.x as u32;
        let cells = data.header.cells;
        let coarse_side = (data.side - 1) / COARSE_STEP + 1;
        let table = 8;
        let coarse = table + tiles * tiles;
        let pages = coarse + coarse_side * coarse_side * 3;
        let page_words = (cells + 1).pow(2) * 3;
        let mut words = vec![0; (pages + PAGE_CAPACITY * page_words) as usize];
        words[..8].copy_from_slice(&[
            tiles,
            cells,
            coarse_side,
            COARSE_STEP,
            table,
            coarse,
            pages,
            PAGE_CAPACITY,
        ]);
        for z in 0..coarse_side {
            for x in 0..coarse_side {
                let src = ((z * COARSE_STEP) * data.side + x * COARSE_STEP) as usize;
                let dst = (coarse + (z * coarse_side + x) * 3) as usize;
                words[dst..dst + 3].copy_from_slice(&data.samples[src]);
            }
        }
        let mut coords: Vec<_> = (0..tiles)
            .flat_map(|z| {
                (0..tiles).map(move |x| TileCoord {
                    x: x as i32,
                    z: z as i32,
                })
            })
            .collect();
        let width = data.header.tile_width();
        coords.sort_by(|a, b| {
            let distance = |c: &TileCoord| {
                ((c.x as f32 + 0.5) * width - eye[0]).powi(2)
                    + ((c.z as f32 + 0.5) * width - eye[1]).powi(2)
            };
            distance(a).total_cmp(&distance(b)).then(a.cmp(b))
        });
        let mut owners = vec![None; PAGE_CAPACITY as usize];
        for (slot, coord) in coords.into_iter().take(PAGE_CAPACITY as usize).enumerate() {
            owners[slot] = Some(coord);
            words[(table + coord.z as u32 * tiles + coord.x as u32) as usize] = slot as u32 + 1;
            let base = (pages + slot as u32 * page_words) as usize;
            for z in 0..=cells {
                for x in 0..=cells {
                    let src = ((coord.z as u32 * cells + z) * data.side
                        + coord.x as u32 * cells
                        + x) as usize;
                    let dst = base + ((z * (cells + 1) + x) * 3) as usize;
                    words[dst..dst + 3].copy_from_slice(&data.samples[src]);
                }
            }
        }
        Ok(Self {
            words,
            owners,
            tiles,
            cells,
            next: PAGE_CAPACITY as usize - 1,
        })
    }
    pub fn upload(&mut self, coord: TileCoord, tile: &Tile, out: &mut Vec<u32>) -> Result<()> {
        ensure!(
            coord.x >= 0
                && coord.z >= 0
                && (coord.x as u32) < self.tiles
                && (coord.z as u32) < self.tiles
                && tile.cells == self.cells,
            "invalid streamed GPU tile"
        );
        tile.validate().map_err(|d| anyhow::anyhow!(d.message))?;
        let slot = if let Some(slot) = self.owners.iter().position(|c| *c == Some(coord)) {
            slot
        } else {
            let slot = self.next;
            self.next = (slot + self.owners.len() - 1) % self.owners.len();
            slot
        };
        let old = self.owners[slot]
            .map(|c| c.z as u32 * self.tiles + c.x as u32)
            .unwrap_or(u32::MAX);
        self.owners[slot] = Some(coord);
        out.extend_from_slice(&[
            coord.z as u32 * self.tiles + coord.x as u32,
            slot as u32,
            old,
            0,
        ]);
        for ((h, w), hole) in tile.heights.iter().zip(&tile.weights).zip(&tile.holes) {
            out.extend_from_slice(&[h.to_bits(), u32::from_le_bytes(*w), u32::from(*hole)]);
        }
        Ok(())
    }
}
