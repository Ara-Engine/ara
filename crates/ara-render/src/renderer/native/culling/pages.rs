//! Virtual shadow page casters (look::vsm): a grid of the opaque instances in the
//! sun's light space, and per dirty page the instances its frustum holds, packed
//! into one caller-owned rows buffer (`Culling::cull_pages`, `record_page`).
use super::*;
use glam::{IVec2, Vec2};
use std::collections::HashMap;

/// Fine grid cell, in light-space metres.
const FINE: f32 = 8.;
/// Coarse grid cell, in light-space metres.
const COARSE: f32 = 128.;
/// Cells an instance may span per axis before moving to the next coarser bucket.
const SPAN: i32 = 8;

/// The opaque instances by light-space cell: fine cells for small casters, coarse
/// ones for large, and a list of those wider still. Moved instances are added again
/// (stale entries stay; candidates are tested and deduplicated) until moves outnumber
/// the instances, when it is rebuilt (amortised constant work per move).
#[derive(Default)]
pub(in crate::renderer::native) struct LightGrid {
    rotation: Mat4,
    fine: HashMap<IVec2, Vec<u32>>,
    coarse: HashMap<IVec2, Vec<u32>>,
    big: Vec<u32>,
    built: bool,
    moves: usize,
}

impl LightGrid {
    /// Light-space x/y rectangle of world bounds.
    fn rect(&self, [lo, hi]: [Vec3; 2]) -> (Vec2, Vec2) {
        let (mut min, mut max) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for corner in 0..8u32 {
            let pick = |bit: u32, axis: usize| {
                if corner >> bit & 1 == 0 {
                    lo[axis]
                } else {
                    hi[axis]
                }
            };
            let p = self
                .rotation
                .transform_point3(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)))
                .truncate();
            (min, max) = (min.min(p), max.max(p));
        }
        (min, max)
    }

    fn insert(&mut self, instance: u32, bounds: [Vec3; 2]) {
        let (lo, hi) = self.rect(bounds);
        for (size, coarse) in [(FINE, false), (COARSE, true)] {
            let (a, b) = (
                (lo / size).floor().as_ivec2(),
                (hi / size).floor().as_ivec2(),
            );
            if (b - a).max_element() < SPAN {
                let map = if coarse {
                    &mut self.coarse
                } else {
                    &mut self.fine
                };
                for y in a.y..=b.y {
                    for x in a.x..=b.x {
                        map.entry(IVec2::new(x, y)).or_default().push(instance);
                    }
                }
                return;
            }
        }
        self.big.push(instance);
    }

    /// Adds every candidate for light-space rectangle `lo..hi` to `out` (with
    /// duplicates and stale entries).
    fn gather(&self, lo: Vec2, hi: Vec2, out: &mut Vec<u32>) {
        for (size, map) in [(FINE, &self.fine), (COARSE, &self.coarse)] {
            // Instances are listed in every cell they cover.
            let (a, b) = (
                (lo / size).floor().as_ivec2(),
                (hi / size).floor().as_ivec2(),
            );
            for y in a.y..=b.y {
                for x in a.x..=b.x {
                    if let Some(cell) = map.get(&IVec2::new(x, y)) {
                        out.extend_from_slice(cell);
                    }
                }
            }
        }
        out.extend_from_slice(&self.big);
    }
}

/// One dirty page to cull: its light view and light-space rectangle.
pub(crate) struct PageView {
    pub matrix: Mat4,
    pub lo: Vec2,
    pub hi: Vec2,
}

impl Culling {
    /// Rebuilds the light grid for light rotation `rotation` unless it holds it with
    /// fewer stale entries than instances.
    fn ensure_grid(&mut self, groups: &Groups, bounds: &[[Vec3; 2]], rotation: Mat4) {
        if self.grid.built && self.grid.rotation == rotation && self.grid.moves <= bounds.len() {
            return;
        }
        self.grid = LightGrid {
            rotation,
            built: true,
            ..LightGrid::default()
        };
        for (i, &b) in bounds.iter().enumerate() {
            if !groups.groups[groups.instance_group[i]].transparent {
                self.grid.insert(i as u32, b);
            }
        }
    }

    /// A moved instance joins its new grid cells (the grid is rebuilt on content).
    pub(super) fn grid_moved(&mut self, index: usize, bounds: [Vec3; 2]) {
        if self.grid.built {
            self.grid.moves += 1;
            self.grid.insert(index as u32, bounds);
        }
    }

    /// Culls each page of `pages` (in order) into rows of `buffer` (`capacity` rows)
    /// and keeps its batches for `record_page`; returns how many pages fit (a first
    /// page with more casters than `capacity` keeps that many).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::renderer::native) fn cull_pages(
        &mut self,
        gpu: &mut GpuDevice,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        rotation: Mat4,
        pages: &[PageView],
        (buffer, capacity): (Buffer, usize),
        work: &mut FrameWork,
    ) -> Result<usize> {
        self.ensure_grid(groups, bounds, rotation);
        self.page_batches.clear();
        let mut rows: Vec<[u32; 2]> = Vec::new();
        let mut candidates = std::mem::take(&mut self.candidates);
        let mut entries = std::mem::take(&mut self.entries);
        for page in pages {
            candidates.clear();
            self.grid.gather(page.lo, page.hi, &mut candidates);
            candidates.sort_unstable();
            candidates.dedup();
            let planes = planes(page.matrix);
            let view = View::shadow(page.matrix, ara_vsm::PAGE_SIZE, self.pixels[1]);
            entries.clear();
            for &i in &candidates {
                let i = i as usize;
                work.cull_tests += 1;
                let group = &groups.groups[groups.instance_group[i]];
                if group.transparent || !visible(&planes, bounds[i]) {
                    continue;
                }
                if let Some(lod) = view.lod(group, self.projections[i]) {
                    entries.push((group.slot + lod, i));
                }
            }
            if rows.len() + entries.len() > capacity {
                if !rows.is_empty() {
                    break;
                }
                // A page with more casters than the whole buffer draws what fits
                // rather than holding every page behind it.
                entries.truncate(capacity);
            }
            entries.sort_unstable();
            let mut batches = Vec::new();
            for chunk in entries.chunk_by(|a, b| a.0 == b.0) {
                let group = &groups.groups[groups.instance_group[chunk[0].1]];
                let lod = chunk[0].0 - group.slot;
                batches.push(Batch {
                    first: rows.len() as u32,
                    count: chunk.len() as u32,
                    vertices: group.counts[lod],
                });
                rows.extend(
                    chunk
                        .iter()
                        .map(|&(_, i)| [i as u32, group.first + lod as u32]),
                );
            }
            self.page_batches.push(batches);
        }
        if !rows.is_empty() {
            gpu.upload_queued_range(buffer, 0, bytemuck::cast_slice(&rows))?;
            work.visible_rows += rows.len();
        }
        self.candidates = candidates;
        self.entries = entries;
        Ok(self.page_batches.len())
    }
}
