//! Stable per-group visible ranges. Idle frames neither traverse nor upload them.
use super::*;
use crate::instanced::FrameWork;
use batches::{Batch, Group};
use updates::Indices;
mod pages;
mod patches;
mod projection;
pub(crate) use pages::PageView;

/// Cascades, opaque/forward camera lists, the existing far-shadow list and the
/// translucent sun casters.
pub(super) const BASE_PASSES: usize = 8;
/// Blended casters seen from the sun (`look::translucent`); every other pass skips them.
pub(super) const TRANSLUCENT_PASS: usize = 7;

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
    /// Per forward draw (`draws[5]`), whether it refracts (`batches::is_refractive`).
    pub(super) refract: Vec<bool>,
    /// Per forward draw, whether it takes back- and front-face passes
    /// (`batches::is_two_pass`).
    pub(super) two_pass: Vec<bool>,
    /// Normalised screen rect (x0, y0, x1, y1; +y down) the visible refractive
    /// draws sample the opaque scene in: their bounds, widened by their own size
    /// on every side for rays bent sideways; `None` without refractive draws.
    pub(super) refract_rect: Option<[f32; 4]>,
    /// Forward draws (indices into `draws[5]`, ascending) before which the lit scene
    /// is copied again (`look::refract` layers): a refractive draw whose sampled rect
    /// overlaps transparent draws made since the last copy, so glass, films and
    /// particles behind glass show through it. At most `REFRACT_LAYERS` - 1.
    pub(super) refract_layers: Vec<usize>,
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
            refract: Vec::new(),
            two_pass: Vec::new(),
            refract_rect: None,
            refract_layers: Vec::new(),
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
        let refract = batches::is_refractive(value);
        let bends = batches::bends_rays(value);
        let translucent = batches::is_translucent(value);
        for &g in &groups.material_groups[material] {
            if groups.groups[g].transparent != transparent
                || groups.groups[g].translucent != translucent
            {
                groups.groups[g].transparent = transparent;
                groups.groups[g].translucent = translucent;
                self.group(g);
                self.transparent_dirty = true;
            }
            if groups.groups[g].refract != refract || groups.groups[g].bends != bends {
                groups.groups[g].refract = refract;
                groups.groups[g].bends = bends;
                self.transparent_dirty = true;
            }
            let two_pass = batches::is_two_pass(value, groups.groups[g].planar);
            if groups.groups[g].two_pass != two_pass {
                groups.groups[g].two_pass = two_pass;
                self.transparent_dirty = true;
            }
            // Batches carry the PSO routing: re-list a group whose routing changes.
            let paint = batches::reads_vertex_colors(value, groups.groups[g].colored);
            let paint_shadow = paint && value.pbr.flags[0] == 1;
            if (groups.groups[g].paint, groups.groups[g].paint_shadow) != (paint, paint_shadow) {
                groups.groups[g].paint = paint;
                groups.groups[g].paint_shadow = paint_shadow;
                self.group(g);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        gpu: &mut GpuDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        frame: &frame::Frame,
        camera: &ara_frame::scene::Camera,
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
        gpu: &mut GpuDevice,
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
            // Pass 4 is the camera; every other opaque-path pass is a shadow view.
            let drawn = if pass == TRANSLUCENT_PASS {
                group.translucent && group.casts
            } else {
                !group.transparent && (pass == 4 || group.casts)
            };
            if !drawn {
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
                    first_index: group.first_indices[lod],
                    paint: if pass == 4 {
                        group.paint
                    } else {
                        group.paint_shadow
                    },
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
        gpu: &mut GpuDevice,
        buffer: Buffer,
        groups: &Groups,
        bounds: &[[Vec3; 2]],
        camera: &ara_frame::scene::Camera,
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
        self.refract.clear();
        self.two_pass.clear();
        let mut layering = Layering::default();
        let matrix = camera.projection * camera.view;
        for (n, &(_, i, g)) in self.transparent.iter().enumerate() {
            let lod = self.selected[i].expect("visible transparent instance has a selected LOD");
            self.draws[5].push(Batch {
                first: (first + n) as u32,
                count: 1,
                vertices: groups.groups[g].counts[lod],
                first_index: groups.groups[g].first_indices[lod],
                paint: false,
            });
            self.visible[first + n] = [i as u32, groups.groups[g].first + lod as u32];
            let refract = groups.groups[g].refract;
            self.refract.push(refract);
            self.two_pass.push(groups.groups[g].two_pass);
            let kind = refract.then_some(groups.groups[g].bends);
            layering.draw(n, kind, projected_rect(matrix, bounds[i]));
        }
        self.refract_rect = layering.rect;
        self.refract_layers = layering.layers;
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

/// Copies of the lit scene a frame's refractive draws may read (`refract_layers`
/// plus the first): later overlapping glass reads the last copy.
pub(super) const REFRACT_LAYERS: usize = 4;

/// Splits the sorted transparent draws into `look::refract` layers as they are
/// listed (`draw`): a refractive draw starts a new layer where what it shows of
/// the scene behind it (its footprint, and half its size around it where it bends
/// rays) overlaps transparent draws made since the last copy.
#[derive(Default)]
struct Layering {
    /// Draws before which the scene is copied again (`Culling::refract_layers`).
    layers: Vec<usize>,
    /// The union of the refractive draws' sampled rects (`Culling::refract_rect`).
    rect: Option<[f32; 4]>,
    /// Screen rect of the transparent draws since the last copy.
    drawn: Option<[f32; 4]>,
}

impl Layering {
    /// Draw `n` with normalised screen rect `screen`; `refract`: `None` for a
    /// blended draw, else whether it bends rays (`batches::bends_rays`).
    fn draw(&mut self, n: usize, refract: Option<bool>, screen: [f32; 4]) {
        if let Some(bends) = refract {
            let shows = widened(screen, if bends { 0.5 } else { 0. });
            if self.drawn.is_some_and(|d| overlaps(d, shows))
                && self.layers.len() + 1 < REFRACT_LAYERS
            {
                self.layers.push(n);
                self.drawn = None;
            }
            let rect = widened(screen, 1.);
            self.rect = Some(self.rect.map_or(rect, |r| union(r, rect)));
        }
        self.drawn = Some(self.drawn.map_or(screen, |d| union(d, screen)));
    }
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Screen rect `screen` (a refractive instance's projected bounds) widened by
/// `share` of its own size on every side, clamped to the screen: by one, the rect
/// its rays may sample.
fn widened(screen: [f32; 4], share: f32) -> [f32; 4] {
    let (lo, hi) = (
        glam::Vec2::new(screen[0], screen[1]),
        glam::Vec2::new(screen[2], screen[3]),
    );
    let size = (hi - lo) * share;
    let (lo, hi) = (
        (lo - size).max(glam::Vec2::ZERO),
        (hi + size).min(glam::Vec2::ONE),
    );
    [lo.x, lo.y, hi.x, hi.y]
}

/// The normalised screen rect (x0, y0, x1, y1; +y down) of world bounds `bounds`;
/// the whole screen when they reach behind the camera.
fn projected_rect(matrix: Mat4, bounds: [Vec3; 2]) -> [f32; 4] {
    let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
    for corner in 0..8 {
        let p = Vec3::new(
            if corner & 1 == 0 {
                bounds[0].x
            } else {
                bounds[1].x
            },
            if corner & 2 == 0 {
                bounds[0].y
            } else {
                bounds[1].y
            },
            if corner & 4 == 0 {
                bounds[0].z
            } else {
                bounds[1].z
            },
        );
        let clip = matrix * p.extend(1.);
        if clip.w <= 1e-4 {
            return [0., 0., 1., 1.];
        }
        let uv = glam::Vec2::new(clip.x / clip.w * 0.5 + 0.5, 0.5 - clip.y / clip.w * 0.5);
        lo = lo.min(uv);
        hi = hi.max(uv);
    }
    let (lo, hi) = (lo.max(glam::Vec2::ZERO), hi.min(glam::Vec2::ONE));
    [lo.x, lo.y, hi.x.max(lo.x), hi.y.max(lo.y)]
}

pub(super) fn planes(matrix: Mat4) -> [Vec4; 6] {
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
    fn glass_over_transparent_draws_starts_a_layer() {
        // Back to front: a film, a pane beside it, a pane over both, then a sphere
        // (bends rays) whose surroundings reach the last pane.
        let mut l = Layering::default();
        l.draw(0, None, [0.1, 0.1, 0.3, 0.3]);
        l.draw(1, Some(false), [0.5, 0.1, 0.7, 0.3]);
        l.draw(2, Some(false), [0.2, 0.2, 0.6, 0.4]);
        l.draw(3, Some(true), [0.65, 0.45, 0.8, 0.6]);
        assert_eq!(l.layers, [2, 3]);
        let rect = l.rect.expect("refractive draws");
        for (a, b) in rect.iter().zip([0., 0., 1., 0.75]) {
            assert!((a - b).abs() < 1e-5, "{rect:?}");
        }
        // Thin glass beside earlier draws needs no copy; at most four copies.
        let mut l = Layering::default();
        l.draw(0, Some(false), [0.1, 0.1, 0.2, 0.2]);
        l.draw(1, Some(false), [0.3, 0.1, 0.4, 0.2]);
        assert!(l.layers.is_empty());
        for n in 2..10 {
            l.draw(n, Some(false), [0.1, 0.1, 0.4, 0.2]);
        }
        assert_eq!(l.layers.len(), REFRACT_LAYERS - 1);
    }

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
        let camera = ara_frame::scene::CameraSpec::LookAt {
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
