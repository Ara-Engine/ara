//! Stable per-group visible ranges. Idle frames neither traverse nor upload them.
use super::*;
use crate::instanced::FrameWork;
use batches::{Batch, Group};
use updates::Indices;

#[derive(Clone, Copy, PartialEq)]
struct LodView {
    position: Vec3,
    projection_y: f32,
    height: u32,
}

pub(super) struct Culling {
    pub draws: [Vec<Batch>; 7],
    visible: Vec<[u32; 2]>,
    selected: Vec<usize>,
    dirty: [Indices; 7],
    matrices: [Option<Mat4>; 7],
    lod_view: Option<LodView>,
    lod_dirty: Indices,
    transparent_dirty: bool,
    transparent: Vec<(f32, usize, usize)>,
    scratch: Vec<Vec<[u32; 2]>>,
}
impl Culling {
    pub fn new(groups: &Groups, instances: usize) -> Self {
        Self {
            draws: std::array::from_fn(|p| {
                if p == 5 {
                    Vec::with_capacity(instances)
                } else {
                    vec![Batch::default(); groups.batches]
                }
            }),
            visible: vec![[0; 2]; instances * 7],
            selected: vec![0; instances],
            dirty: std::array::from_fn(|_| Indices::new(groups.groups.len())),
            matrices: [None; 7],
            lod_view: None,
            lod_dirty: Indices::new(instances),
            transparent_dirty: true,
            transparent: Vec::with_capacity(instances),
            scratch: (0..groups
                .groups
                .iter()
                .map(|g| g.counts.len())
                .max()
                .unwrap_or(0))
                .map(|_| Vec::new())
                .collect(),
        }
    }
    fn group(&mut self, index: usize) {
        for dirty in &mut self.dirty {
            dirty.insert(index);
        }
    }
    pub fn instance(&mut self, groups: &Groups, index: usize) {
        self.lod_dirty.insert(index);
        self.group(groups.instance_group[index]);
        if groups.groups[groups.instance_group[index]].transparent {
            self.transparent_dirty = true;
        }
    }
    pub fn material(
        &mut self,
        groups: &mut Groups,
        material: usize,
        value: &crate::instanced::Material,
    ) {
        let transparent = batches::is_transparent(value);
        for &g in &groups.material_groups[material] {
            if groups.groups[g].transparent != transparent {
                groups.groups[g].transparent = transparent;
                self.group(g);
                self.transparent_dirty = true;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        gpu: &VulkanDevice,
        buffer: Buffer,
        groups: &Groups,
        rows: &[InstanceRow],
        bounds: &[[Vec3; 2]],
        frame: &frame::Frame,
        camera: &crate::scene::Camera,
        casters: &mut Vec<[Vec3; 2]>,
        work: &mut FrameWork,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.cull_lod");
        let view = LodView {
            position: camera.position,
            projection_y: camera.projection.y_axis.y.abs(),
            height: frame.extent[1],
        };
        if self.lod_view != Some(view) {
            for i in 0..rows.len() {
                self.lod(groups, rows, bounds, i, view, casters, work);
            }
            self.lod_view = Some(view);
        } else {
            for n in 0..self.lod_dirty.list.len() {
                let i = self.lod_dirty.list[n];
                self.lod(groups, rows, bounds, i, view, casters, work);
            }
        }
        self.lod_dirty.clear();
        let vp = camera.projection * camera.view;
        let result = (|| {
            for pass in 0..5 {
                if pass < 4 && pass >= frame.look[2] as usize {
                    if self.matrices[pass].take().is_some() {
                        for batch in &mut self.draws[pass] {
                            batch.count = 0;
                        }
                    }
                    continue;
                }
                self.opaque(
                    gpu,
                    buffer,
                    groups,
                    bounds,
                    pass,
                    if pass < 4 { frame.shadows[pass] } else { vp },
                    work,
                )?;
            }
            if self.transparent_dirty || self.matrices[5] != Some(vp) {
                self.transparency(gpu, buffer, groups, bounds, camera, work)?;
                self.matrices[5] = Some(vp);
                self.transparent_dirty = false;
            }
            self.dirty[5].clear();
            Ok(())
        })();
        if result.is_err() {
            self.matrices = [None; 7];
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn lod(
        &mut self,
        groups: &Groups,
        rows: &[InstanceRow],
        bounds: &[[Vec3; 2]],
        i: usize,
        view: LodView,
        casters: &mut Vec<[Vec3; 2]>,
        work: &mut FrameWork,
    ) {
        work.lod_tests += 1;
        let group = groups.instance_group[i];
        let next = select_lod(&groups.groups[group], &rows[i], bounds[i], view);
        if next != self.selected[i] {
            self.selected[i] = next;
            self.group(group);
            if groups.groups[group].transparent {
                self.transparent_dirty = true;
            }
            casters.push(bounds[i]);
        }
    }

    pub fn shadow(
        &mut self,
        gpu: &VulkanDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        matrix: Mat4,
        work: &mut FrameWork,
    ) -> Result<()> {
        self.opaque(gpu, buffer, groups, bounds, 6, matrix, work)
    }

    #[allow(clippy::too_many_arguments)]
    fn opaque(
        &mut self,
        gpu: &VulkanDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        pass: usize,
        matrix: Mat4,
        work: &mut FrameWork,
    ) -> Result<()> {
        let planes = planes(matrix);
        let all = self.matrices[pass] != Some(matrix);
        let count = if all {
            groups.groups.len()
        } else {
            self.dirty[pass].list.len()
        };
        for n in 0..count {
            let g = if all { n } else { self.dirty[pass].list[n] };
            let group = &groups.groups[g];
            for lod in 0..group.counts.len() {
                self.draws[pass][group.slot + lod].count = 0;
                self.scratch[lod].clear();
            }
            if group.transparent {
                continue;
            }
            for &i in &group.instances {
                work.cull_tests += 1;
                if visible(&planes, bounds[i]) {
                    let lod = self.selected[i];
                    self.scratch[lod].push([i as u32, group.first + lod as u32]);
                }
            }
            let first = pass * self.selected.len() + group.offset;
            let mut cursor = first;
            for (lod, entries) in self.scratch[..group.counts.len()].iter().enumerate() {
                self.visible[cursor..cursor + entries.len()].copy_from_slice(entries);
                self.draws[pass][group.slot + lod] = Batch {
                    first: cursor as u32,
                    count: entries.len() as u32,
                    vertices: group.counts[lod],
                };
                cursor += entries.len();
            }
            if cursor > first {
                gpu.upload_range(
                    buffer,
                    (first * 8) as u64,
                    bytemuck::cast_slice(&self.visible[first..cursor]),
                )?;
                work.visible_rows += cursor - first;
            }
        }
        self.matrices[pass] = Some(matrix);
        self.dirty[pass].clear();
        Ok(())
    }

    fn transparency(
        &mut self,
        gpu: &VulkanDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        camera: &crate::scene::Camera,
        work: &mut FrameWork,
    ) -> Result<()> {
        self.transparent.clear();
        self.draws[5].clear();
        let planes = planes(camera.projection * camera.view);
        for (g, group) in groups.groups.iter().enumerate() {
            if !group.transparent {
                continue;
            }
            for &i in &group.instances {
                work.cull_tests += 1;
                if visible(&planes, bounds[i]) {
                    let distance =
                        ((bounds[i][0] + bounds[i][1]) * 0.5).distance_squared(camera.position);
                    self.transparent.push((distance, i, g));
                }
            }
        }
        self.transparent
            .sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let first = 5 * self.selected.len();
        for (n, &(_, i, g)) in self.transparent.iter().enumerate() {
            let lod = self.selected[i];
            self.draws[5].push(Batch {
                first: (first + n) as u32,
                count: 1,
                vertices: groups.groups[g].counts[lod],
            });
            self.visible[first + n] = [i as u32, groups.groups[g].first + lod as u32];
        }
        let count = self.transparent.len();
        if count > 0 {
            gpu.upload_range(
                buffer,
                (first * 8) as u64,
                bytemuck::cast_slice(&self.visible[first..first + count]),
            )?;
            work.visible_rows += count;
        }
        Ok(())
    }
}

fn select_lod(group: &Group, row: &InstanceRow, bounds: [Vec3; 2], view: LodView) -> usize {
    let scale = row
        .world
        .x_axis
        .length()
        .max(row.world.y_axis.length())
        .max(row.world.z_axis.length());
    let radius = (bounds[1] - bounds[0]).length() * 0.5;
    let distance = (((bounds[0] + bounds[1]) * 0.5).distance(view.position) - radius).max(0.1);
    let pixels = view.projection_y * view.height as f32 * 0.5 / distance;
    group
        .errors
        .iter()
        .enumerate()
        .rev()
        .find_map(|(lod, error)| (*error * scale * pixels <= 1.).then_some(lod))
        .unwrap_or(0)
}

fn planes(matrix: Mat4) -> [Vec4; 6] {
    let x = matrix.row(0);
    let y = matrix.row(1);
    let z = matrix.row(2);
    let w = matrix.row(3);
    [w + x, w - x, w + y, w - y, z, w - z]
}
fn visible(planes: &[Vec4; 6], bounds: [Vec3; 2]) -> bool {
    let centre = (bounds[0] + bounds[1]) * 0.5;
    let extent = (bounds[1] - bounds[0]) * 0.5;
    planes.iter().all(|plane| {
        plane.truncate().dot(centre) + plane.w + plane.truncate().abs().dot(extent) >= 0.
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reversed_z_culling_keeps_intersections_and_rejects_offscreen_bounds() {
        let camera = crate::scene::CameraSpec::LookAt {
            position: Vec3::ZERO,
            target: -Vec3::Z,
            vertical_fov: 1.,
        }
        .resolve([256, 144])
        .unwrap();
        let planes = planes(camera.projection * camera.view);
        assert!(visible(
            &planes,
            [Vec3::new(-1., -1., -5.), Vec3::new(1., 1., -3.)]
        ));
        assert!(!visible(
            &planes,
            [Vec3::new(100., -1., -5.), Vec3::new(102., 1., -3.)]
        ));
        assert!(!visible(
            &planes,
            [Vec3::new(-1., -1., 1.), Vec3::new(1., 1., 3.)]
        ));
    }
}
