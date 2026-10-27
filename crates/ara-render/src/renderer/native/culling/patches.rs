//! Shadow patch culling (look::cascades, look::local): a patch redraws a few texels
//! of a cached shadow view, so it tests only the instances that view's culled list
//! holds there. Each cascade or local-face list keeps a coarse grid over its clip
//! square ([`Grid`]), maintained by its own culls; a patch gathers the grid cells
//! it covers plus the groups moved since that list was culled.
use super::*;
use glam::Vec2;

/// Grid cells per clip-square side.
const CELLS: usize = 16;

/// The instances a shadow view's culled list holds, by coarse cell of its clip
/// square. A full cull rebuilds it and incremental ones add their dirty groups;
/// an instance that moved stays listed in its old cells as well, since patches
/// test every candidate again.
#[derive(Clone, Default)]
pub(super) struct Grid {
    cells: Vec<Vec<u32>>,
}

impl Grid {
    pub(super) fn clear(&mut self) {
        for cell in &mut self.cells {
            cell.clear();
        }
    }

    /// Lists `instance` (world `bounds`) in the cells of `matrix` it reaches;
    /// `again` skips cells that already hold it (incremental culls).
    pub(super) fn insert(&mut self, matrix: Mat4, instance: u32, bounds: [Vec3; 2], again: bool) {
        if self.cells.is_empty() {
            self.cells = vec![Vec::new(); CELLS * CELLS];
        }
        let [x0, y0, x1, y1] = reach(matrix, bounds);
        for y in y0..=y1 {
            for cell in &mut self.cells[y * CELLS + x0..=y * CELLS + x1] {
                if !again || !cell.contains(&instance) {
                    cell.push(instance);
                }
            }
        }
    }

    /// Appends the instances listed in the cells of clip rectangle `clip`.
    fn gather(&self, clip: [Vec2; 2], out: &mut Vec<u32>) {
        if self.cells.is_empty() {
            return;
        }
        let [x0, y0] = cell(clip[0]);
        let [x1, y1] = cell(clip[1]);
        for y in y0..=y1 {
            for cell in &self.cells[y * CELLS + x0..=y * CELLS + x1] {
                out.extend_from_slice(cell);
            }
        }
    }
}

/// The cell holding clip-space `p` (clamped to the square).
fn cell(p: Vec2) -> [usize; 2] {
    let at = |v: f32| (((v + 1.) * 0.5 * CELLS as f32) as usize).min(CELLS - 1);
    [at(p.x.clamp(-1., 1.)), at(p.y.clamp(-1., 1.))]
}

/// The cells `[x0, y0, x1, y1]` world bounds reach through `matrix`: all of them
/// when a corner is not in front of a perspective view.
fn reach(matrix: Mat4, [lo, hi]: [Vec3; 2]) -> [usize; 4] {
    let (mut min, mut max) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for corner in 0..8u32 {
        let pick = |bit: u32, axis: usize| {
            if corner >> bit & 1 == 0 {
                lo[axis]
            } else {
                hi[axis]
            }
        };
        let clip = matrix * Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)).extend(1.);
        if clip.w <= 1e-6 {
            return [0, 0, CELLS - 1, CELLS - 1];
        }
        let p = clip.truncate().truncate() / clip.w;
        (min, max) = (min.min(p), max.max(p));
    }
    let ([x0, y0], [x1, y1]) = (cell(min), cell(max));
    [x0, y0, x1, y1]
}

impl Culling {
    /// Culls a shadow patch into `pass`: the instances `source`'s list (drawn with
    /// `parent`) holds in clip rectangle `clip` of that view, plus its groups moved
    /// since, tested against the cropped `view`. A source list culled for another
    /// view falls back to every instance.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::renderer::native) fn patch(
        &mut self,
        gpu: &mut GpuDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        (source, parent, clip): (usize, Mat4, [Vec2; 2]),
        pass: usize,
        view: View,
        work: &mut FrameWork,
    ) -> Result<()> {
        let mut candidates = std::mem::take(&mut self.candidates);
        candidates.clear();
        if self.views[source].is_some_and(|v| v.matrix == parent) {
            self.grids[source].gather(clip, &mut candidates);
            for &g in &self.dirty[source].list {
                candidates.extend(groups.groups[g].instances.iter().map(|&i| i as u32));
            }
            candidates.sort_unstable();
            candidates.dedup();
        } else {
            candidates.extend(0..self.selected.len() as u32);
        }
        let planes = planes(view.matrix);
        let mut entries = std::mem::take(&mut self.entries);
        entries.clear();
        for &i in &candidates {
            let i = i as usize;
            work.cull_tests += 1;
            let g = groups.instance_group[i];
            let group = &groups.groups[g];
            if group.transparent || !visible(&planes, bounds[i]) {
                continue;
            }
            if let Some(lod) = view.lod(group, self.projections[i]) {
                entries.push((group.slot + lod, i));
            }
        }
        entries.sort_unstable();
        for batch in &mut self.draws[pass] {
            batch.count = 0;
        }
        let first = pass * self.selected.len();
        let mut cursor = first;
        for chunk in entries.chunk_by(|a, b| a.0 == b.0) {
            let i = chunk[0].1;
            let group = &groups.groups[groups.instance_group[i]];
            let lod = chunk[0].0 - group.slot;
            for (row, &(_, i)) in self.visible[cursor..cursor + chunk.len()]
                .iter_mut()
                .zip(chunk)
            {
                *row = [i as u32, group.first + lod as u32];
            }
            self.draws[pass][chunk[0].0] = Batch {
                first: cursor as u32,
                count: chunk.len() as u32,
                vertices: group.counts[lod],
                first_index: group.first_indices[lod],
            };
            cursor += chunk.len();
        }
        if cursor > first {
            gpu.upload_queued_range(
                buffer,
                (first * 8) as u64,
                bytemuck::cast_slice(&self.visible[first..cursor]),
            )?;
            work.visible_rows += cursor - first;
        }
        // The slot holds a patch now: a later full cull of it starts over.
        self.views[pass] = None;
        self.dirty[pass].clear();
        self.candidates = candidates;
        self.entries = entries;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_lists_instances_in_the_cells_they_reach() {
        // A 20 m orthographic view: 1.25 m cells.
        let matrix = Mat4::orthographic_rh(-10., 10., -10., 10., 10., -10.);
        let mut grid = Grid::default();
        grid.insert(
            matrix,
            7,
            [Vec3::new(0.1, 0.1, 0.), Vec3::new(0.2, 0.2, 0.)],
            false,
        );
        grid.insert(
            matrix,
            9,
            [Vec3::new(-10., -10., 0.), Vec3::new(10., 10., 0.)],
            false,
        );
        grid.insert(
            matrix,
            7,
            [Vec3::new(0.1, 0.1, 0.), Vec3::new(0.2, 0.2, 0.)],
            true,
        );
        let gather = |clip| {
            let mut out = Vec::new();
            grid.gather(clip, &mut out);
            out
        };
        assert_eq!(
            gather([Vec2::splat(0.005), Vec2::splat(0.03)]),
            [7, 9],
            "one cell: each once, with the instance covering every cell"
        );
        // Four cells list the covering instance in each; patches deduplicate.
        assert_eq!(gather([Vec2::splat(-0.9), Vec2::splat(-0.8)]), [9; 4]);
    }
}
