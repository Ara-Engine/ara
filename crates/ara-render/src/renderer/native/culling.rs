//! Stable per-group visible ranges. Idle frames neither traverse nor upload them.
use super::*;
use crate::instanced::FrameWork;
use batches::{Batch, Group};
use updates::Indices;
mod pages;
mod patches;
mod projection;
pub(crate) use pages::PageView;

/// Cascades, opaque/forward camera lists and the existing far-shadow list.
pub(super) const BASE_PASSES: usize = 7;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct View {
    matrix: Mat4,
    size: [u32; 2],
    error: f32,
    minimum: f32,
    row_lengths: Vec3,
    axes: [Vec4; 3],
}

pub(super) struct Culling {
    pub draws: Vec<Vec<Batch>>,
    pub(super) visible: Vec<[u32; 2]>,
    selected: Vec<Option<usize>>,
    projections: Vec<projection::LodBounds>,
    dirty: Vec<Indices>,
    views: Vec<Option<View>>,
    lod_view: Option<View>,
    pixels: [f32; 2],
    lod_dirty: Indices,
    transparent_dirty: bool,
    transparent: Vec<(f32, usize, usize)>,
    scratch: Vec<Vec<[u32; 2]>>,
    /// Per shadow pass, its culled instances by clip cell (`patches::Grid`).
    grids: Vec<patches::Grid>,
    /// Patch scratch: candidate instances, then (batch slot, instance) entries.
    candidates: Vec<u32>,
    entries: Vec<(usize, usize)>,
    /// The opaque instances in the sun's light space (virtual shadow pages).
    grid: pages::LightGrid,
    /// The last `cull_pages`' batches, per page.
    pub(super) page_batches: Vec<Vec<Batch>>,
}
impl Culling {
    pub fn new(groups: &Groups, scene: &NativeScene) -> Self {
        let instances = scene.instances.len();
        Self::with_projections(
            groups,
            instances,
            scene
                .instances
                .iter()
                .map(|i| projection::LodBounds::new(i.world, i.bounds(&scene.geometry[i.geometry])))
                .collect(),
        )
    }
    /// A camera's lists reference the shared groups and geometry; only its bounds
    /// cache and visible ranges are independent.
    pub(super) fn fork(&self, groups: &Groups) -> Self {
        Self::with_projections(groups, self.selected.len(), self.projections.to_vec())
    }
    fn with_projections(
        groups: &Groups,
        instances: usize,
        projections: Vec<projection::LodBounds>,
    ) -> Self {
        Self {
            draws: (0..BASE_PASSES)
                .map(|p| {
                    if p == 5 {
                        Vec::with_capacity(instances)
                    } else {
                        vec![Batch::default(); groups.batches]
                    }
                })
                .collect(),
            visible: vec![[0; 2]; instances * BASE_PASSES],
            selected: vec![Some(0); instances],
            projections,
            dirty: (0..BASE_PASSES)
                .map(|_| Indices::new(groups.groups.len()))
                .collect(),
            views: vec![None; BASE_PASSES],
            grids: vec![patches::Grid::default(); BASE_PASSES],
            candidates: Vec::new(),
            entries: Vec::new(),
            grid: pages::LightGrid::default(),
            page_batches: Vec::new(),
            lod_view: None,
            pixels: [1., 1.5],
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
    /// A publication/pose revision invalidates this camera's lists, without
    /// copying local vertices, materials, textures or any GPU geometry.
    pub(super) fn refresh(&mut self, source: &Self, groups: &Groups) {
        self.projections.copy_from_slice(&source.projections);
        self.views.fill(None);
        self.grid = pages::LightGrid::default();
        self.lod_view = None;
        for index in 0..self.selected.len() {
            self.lod_dirty.insert(index);
        }
        for dirty in &mut self.dirty {
            for group in 0..groups.groups.len() {
                dirty.insert(group);
            }
        }
        self.transparent_dirty = true;
    }
    /// Called only after the larger coherent GPU buffer was successfully staged.
    pub fn reserve(&mut self, groups: &Groups, passes: usize) {
        self.visible.resize(self.selected.len() * passes, [0; 2]);
        self.draws
            .resize_with(passes, || vec![Batch::default(); groups.batches]);
        self.dirty
            .resize_with(passes, || Indices::new(groups.groups.len()));
        self.views.resize(passes, None);
        self.grids.resize_with(passes, Default::default);
    }
    pub fn configure(&mut self, quality: &crate::look::Quality) -> bool {
        let pixels = [quality.lod_pixels, quality.min_pixels];
        let changed = self.pixels != pixels;
        self.pixels = pixels;
        changed
    }
    fn group(&mut self, index: usize) {
        for dirty in &mut self.dirty {
            dirty.insert(index);
        }
    }
    pub fn instance(&mut self, groups: &Groups, index: usize, world: Mat4, bounds: [Vec3; 2]) {
        self.projections[index] = projection::LodBounds::new(world, bounds);
        self.grid_moved(index, bounds);
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
        gpu: &mut VulkanDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        frame: &frame::Frame,
        camera: &crate::scene::Camera,
        shadow_resolution: u32,
        work: &mut FrameWork,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("RasterNative.cull_lod");
        let view = View::new(
            camera.projection * camera.view,
            [frame.extent[0], frame.extent[1]],
            self.pixels[0],
            self.pixels[1],
        );
        if self.lod_view != Some(view) {
            for i in 0..self.projections.len() {
                self.lod(groups, i, &view, work);
            }
            self.lod_view = Some(view);
        } else {
            for n in 0..self.lod_dirty.list.len() {
                let i = self.lod_dirty.list[n];
                self.lod(groups, i, &view, work);
            }
        }
        self.lod_dirty.clear();
        let result = (|| {
            for pass in 0..5 {
                if pass < 4 && pass >= frame.look[2] as usize {
                    if self.views[pass].take().is_some() {
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
                    if pass < 4 {
                        View::shadow(frame.shadows[pass], shadow_resolution, self.pixels[1])
                    } else {
                        view
                    },
                    work,
                )?;
            }
            if self.transparent_dirty || self.views[5] != Some(view) {
                self.transparency(gpu, buffer, groups, bounds, camera, work)?;
                self.views[5] = Some(view);
                self.transparent_dirty = false;
            }
            self.dirty[5].clear();
            Ok(())
        })();
        if result.is_err() {
            self.views.fill(None);
        }
        result
    }

    fn lod(&mut self, groups: &Groups, i: usize, view: &View, work: &mut FrameWork) {
        work.lod_tests += 1;
        let group = groups.instance_group[i];
        let next = view.lod(&groups.groups[group], self.projections[i]);
        if next != self.selected[i] {
            self.selected[i] = next;
            // Shadow views choose LOD from their own texel density. A camera
            // LOD switch changes only the camera lists, never shadow casters.
            self.dirty[4].insert(group);
            if groups.groups[group].transparent {
                self.transparent_dirty = true;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn opaque(
        &mut self,
        gpu: &mut VulkanDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        pass: usize,
        view: View,
        work: &mut FrameWork,
    ) -> Result<()> {
        let planes = planes(view.matrix);
        let all = self.views[pass] != Some(view);
        // Cascade and local-face lists index their instances for patches.
        let indexed = !(4..BASE_PASSES).contains(&pass);
        if all && indexed {
            self.grids[pass].clear();
        }
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
                    let lod = if pass == 4 {
                        self.selected[i]
                    } else {
                        view.lod(group, self.projections[i])
                    };
                    if let Some(lod) = lod {
                        self.scratch[lod].push([i as u32, group.first + lod as u32]);
                        if indexed {
                            self.grids[pass].insert(view.matrix, i as u32, bounds[i], !all);
                        }
                    }
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
                gpu.upload_queued_range(
                    buffer,
                    (first * 8) as u64,
                    bytemuck::cast_slice(&self.visible[first..cursor]),
                )?;
                work.visible_rows += cursor - first;
            }
        }
        self.views[pass] = Some(view);
        self.dirty[pass].clear();
        Ok(())
    }

    fn transparency(
        &mut self,
        gpu: &mut VulkanDevice,
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
                if self.selected[i].is_some() && visible(&planes, bounds[i]) {
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
            let lod = self.selected[i].expect("visible transparent instance has a selected LOD");
            self.draws[5].push(Batch {
                first: (first + n) as u32,
                count: 1,
                vertices: groups.groups[g].counts[lod],
            });
            self.visible[first + n] = [i as u32, groups.groups[g].first + lod as u32];
        }
        let count = self.transparent.len();
        if count > 0 {
            gpu.upload_queued_range(
                buffer,
                (first * 8) as u64,
                bytemuck::cast_slice(&self.visible[first..first + count]),
            )?;
            work.visible_rows += count;
        }
        Ok(())
    }
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
    fn local_shadow_perspective_culls_all_six_planes_including_near() {
        let pose = Mat4::from_rotation_translation(
            glam::Quat::from_rotation_y(0.7),
            Vec3::new(11., 3., -7.),
        );
        let planes = planes(
            Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1., 20., 0.05) * pose.inverse(),
        );
        let bounds = |p| {
            let center = pose.transform_point3(p);
            [center - Vec3::splat(0.001), center + Vec3::splat(0.001)]
        };
        assert!(visible(&planes, bounds(Vec3::new(0., 0., -1.))));
        for point in [
            Vec3::new(-2., 0., -1.),
            Vec3::new(2., 0., -1.),
            Vec3::new(0., -2., -1.),
            Vec3::new(0., 2., -1.),
            Vec3::new(0., 0., -21.),
            Vec3::new(0., 0., -0.02),
        ] {
            assert!(!visible(&planes, bounds(point)), "{point:?}");
        }
    }

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
