//! Deferred decals: static records written at publication (and on decal edits) and
//! a per-frame visible list the `native_decals` compute pass bins into 16x16 tiles.
//! Culling, fades and draw order are decided here, once per decal, not per pixel.
use super::*;
use crate::instanced::Decal;
use glam::Vec2;

/// Visible decals per frame: the tile pass keeps one bit each in group-shared memory.
pub(in crate::renderer) const MAX_VISIBLE: usize = 1024;
/// Bytes of one decal record (`shaders/decals.slang` `DecalRecord`).
const RECORD: usize = 160;
/// Bytes of the visible list header (count and the G-buffer storage views) and of
/// one visible entry.
const HEADER: usize = 32;
const ENTRY: usize = 32;

/// One decal's GPU record: world to its unit box, its axes, opacities and material.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Record {
    to_box: Mat4,
    /// x, y, z axes (unit, world) with the base colour, normal and roughness opacities.
    axes: [[f32; 4]; 3],
    /// Metallic, specular, emissive and occlusion opacities.
    opacity: [f32; 4],
    /// Unused, cos(fade angle start), cos(fade angle end), material row (bits).
    angle: [f32; 4],
    /// Opacity map channel (bits; 4 none), occlusion factor, occlusion channel (bits),
    /// specular level.
    maps: [f32; 4],
}

/// Native decal data: records, this frame's visible list and its tile storage views.
pub(super) struct Decals {
    pub decals: Vec<Decal>,
    pub records: Buffer,
    pub record_index: u32,
    pub visible: Buffer,
    pub visible_index: u32,
    /// Visible decals this frame (`prepare`).
    pub count: usize,
    /// Records were rewritten since the consuming frame.
    pub dirty: bool,
    entries: Vec<(i32, u64, f32, [u32; 8])>,
}

impl Decals {
    pub fn new(gpu: &mut GpuDevice, decals: &[Decal], owned: &mut Vec<Resource>) -> Result<Self> {
        let records = host_buffer(gpu, (decals.len() * RECORD).max(16), owned)?;
        let visible = host_buffer(
            gpu,
            HEADER + decals.len().clamp(1, MAX_VISIBLE) * ENTRY,
            owned,
        )?;
        let mut this = Self {
            decals: decals.to_vec(),
            record_index: gpu.storage_buffer(records)?.index,
            records,
            visible_index: gpu.storage_buffer(visible)?.index,
            visible,
            count: 0,
            dirty: true,
            entries: Vec::new(),
        };
        if !decals.is_empty() {
            let bytes: Vec<u8> = this
                .decals
                .iter()
                .flat_map(|d| bytemuck::bytes_of(&record(d)).to_vec())
                .collect();
            gpu.upload(this.records, &bytes)?;
        }
        this.dirty = true;
        Ok(this)
    }

    /// Streams changed decal parameters/poses (same count and order).
    pub fn update(&mut self, gpu: &mut GpuDevice, decals: &[Decal]) -> Result<()> {
        ensure!(
            decals.len() == self.decals.len(),
            "Native decal count changed; replace the scene"
        );
        self.decals.clone_from_slice(decals);
        let bytes: Vec<u8> = decals
            .iter()
            .flat_map(|d| bytemuck::bytes_of(&record(d)).to_vec())
            .collect();
        if !bytes.is_empty() {
            gpu.upload_queued_range(self.records, 0, &bytes)?;
        }
        self.dirty = true;
        Ok(())
    }

    /// Culls, fades and orders this frame's decals for `camera` at render size
    /// `size`, and writes the visible list with the G-buffer `storage` views
    /// (albedo, normal, material, emissive). Returns how many are visible.
    pub fn prepare(
        &mut self,
        gpu: &mut GpuDevice,
        camera: &ara_frame::scene::Camera,
        size: [u32; 2],
        storage: [u32; 4],
    ) -> Result<usize> {
        self.entries.clear();
        let vp = camera.projection * camera.view;
        let planes = culling::planes(vp);
        let height = (camera.projection.y_axis.y).abs().max(1e-6);
        for (index, decal) in self.decals.iter().enumerate() {
            let corners = corners(decal.world);
            if planes
                .iter()
                .any(|p| corners.iter().all(|c| p.dot(c.extend(1.0)) < 0.0))
            {
                continue;
            }
            let centre = decal.world.transform_point3(Vec3::ZERO);
            let radius = corners
                .iter()
                .map(|c| c.distance(centre))
                .fold(0.0, f32::max);
            let distance = centre.distance(camera.position);
            let mut fade = 1.0;
            let [start, end] = decal.fade_distance;
            if end > start {
                fade *= 1.0 - ((distance - start) / (end - start)).clamp(0.0, 1.0);
            }
            // Screen share of the view height (perspective; orthographic views keep it).
            let screen = if camera.projection.w_axis.w == 0.0 {
                radius * height / distance.max(1e-3)
            } else {
                radius * height
            };
            if decal.fade_screen_size > 0.0 {
                // Fade over the size's first doubling, as Unreal's FadeScreenSize does over time.
                fade *=
                    ((screen - decal.fade_screen_size) / decal.fade_screen_size).clamp(0.0, 1.0);
            }
            if fade <= 0.0 || decal.opacity.iter().all(|o| *o <= 0.0) {
                continue;
            }
            let (rect, depth) = footprint(vp, &corners, size);
            let mut entry = [0u32; 8];
            entry[0] = index as u32;
            entry[1] = rect[0] | rect[1] << 16;
            entry[2] = rect[2] | rect[3] << 16;
            entry[3] = fade.to_bits();
            entry[4] = depth[0].to_bits();
            entry[5] = depth[1].to_bits();
            self.entries
                .push((decal.sort_order, decal.entity, screen, entry));
        }
        if self.entries.len() > MAX_VISIBLE {
            // Keep the largest on screen; the rest are the least visible.
            self.entries.sort_by(|a, b| b.2.total_cmp(&a.2));
            self.entries.truncate(MAX_VISIBLE);
        }
        self.entries.sort_by_key(|e| (e.0, e.1));
        self.count = self.entries.len();
        let mut bytes = Vec::with_capacity(HEADER + self.count * ENTRY);
        let header = [
            self.count as u32,
            storage[0],
            storage[1],
            storage[2],
            storage[3],
            0,
            0,
            0,
        ];
        bytes.extend_from_slice(bytemuck::cast_slice(&header));
        for (.., entry) in &self.entries {
            bytes.extend_from_slice(bytemuck::cast_slice(entry));
        }
        gpu.upload_queued_range(self.visible, 0, &bytes)?;
        Ok(self.count)
    }
}

fn record(decal: &Decal) -> Record {
    let world = decal.world;
    let axis = |v: Vec3| v.normalize_or_zero();
    let [x, y, z] = [world.x_axis, world.y_axis, world.z_axis].map(|a| axis(a.truncate()));
    let o = decal.opacity;
    let cos = |degrees: f32| degrees.to_radians().cos();
    Record {
        to_box: world.inverse(),
        axes: [
            [x.x, x.y, x.z, o[0]],
            [y.x, y.y, y.z, o[1]],
            [z.x, z.y, z.z, o[2]],
        ],
        opacity: [o[3], o[4], o[5], o[6]],
        angle: [
            0.0,
            cos(decal.fade_angle[0]),
            cos(decal.fade_angle[1].max(decal.fade_angle[0] + 0.01)),
            f32::from_bits(decal.material as u32),
        ],
        maps: [
            f32::from_bits(decal.opacity_channel),
            decal.occlusion[0],
            f32::from_bits(decal.occlusion[1] as u32),
            decal.specular,
        ],
    }
}

fn corners(world: Mat4) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        let c = Vec3::new(
            if i & 1 == 0 { -0.5 } else { 0.5 },
            if i & 2 == 0 { -0.5 } else { 0.5 },
            if i & 4 == 0 { -0.5 } else { 0.5 },
        );
        world.transform_point3(c)
    })
}

/// The render-pixel rect (x0, y0, x1, y1) and reversed-Z depth range (near, far) the
/// box covers; a box crossing the camera plane covers the whole view and every depth.
fn footprint(vp: Mat4, corners: &[Vec3; 8], size: [u32; 2]) -> ([u32; 4], [f32; 2]) {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    let mut depth = [f32::NEG_INFINITY, f32::INFINITY];
    for c in corners {
        let clip = vp * c.extend(1.0);
        if clip.w <= 1e-5 {
            return ([0, 0, size[0], size[1]], [1.0, 0.0]);
        }
        let ndc = clip.truncate() / clip.w;
        let pixel = Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
            * Vec2::new(size[0] as f32, size[1] as f32);
        lo = lo.min(pixel);
        hi = hi.max(pixel);
        depth[0] = depth[0].max(ndc.z);
        depth[1] = depth[1].min(ndc.z);
    }
    let clamp = |v: f32, max: u32| v.clamp(0.0, max as f32) as u32;
    (
        [
            clamp(lo.x.floor(), size[0]),
            clamp(lo.y.floor(), size[1]),
            clamp(hi.x.ceil() + 1.0, size[0]),
            clamp(hi.y.ceil() + 1.0, size[1]),
        ],
        [depth[0].min(1.0), depth[1].max(0.0)],
    )
}

impl Native {
    /// Whether the content has decals to draw.
    pub(in crate::renderer) fn has_decals(&self) -> bool {
        !self.decals.decals.is_empty()
    }
    /// Culls and orders this frame's decals into the visible list (see
    /// `Decals::prepare`); `storage` holds the G-buffer storage views.
    pub(in crate::renderer) fn prepare_decals(
        &mut self,
        gpu: &mut GpuDevice,
        camera: &ara_frame::scene::Camera,
        size: [u32; 2],
        storage: [u32; 4],
    ) -> Result<usize> {
        self.decals.prepare(gpu, camera, size, storage)
    }
    /// Records the decal pass over render size `size`; `coverage` draws the decal
    /// coverage view (`ViewMode::DecalCoverage`) instead of blending.
    pub(in crate::renderer) fn record_decals(
        &self,
        recorder: &mut Recorder,
        root: RootConstants,
        size: [u32; 2],
        coverage: bool,
    ) {
        recorder.push(Command::Pipeline(self.pipelines.decals));
        let mut constants = root;
        constants.words[1] = self.material_index;
        constants.words[2] = self.instance_index;
        constants.words[3] = self.decals.record_index;
        constants.words[4] = self.decals.visible_index;
        constants.words[6] = self.textures.binding().map_or(0, |b| b.buffer_index);
        constants.words[7] = u32::from(coverage);
        recorder.push(Command::Constants(constants));
        recorder.push(Command::Dispatch([
            size[0].div_ceil(16),
            size[1].div_ceil(16),
            1,
        ]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_in_front_covers_its_projection_and_one_crossing_the_camera_everything() {
        let camera = ara_frame::scene::CameraSpec::LookAt {
            position: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            vertical_fov: 0.8,
        }
        .resolve([200, 100])
        .expect("camera");
        let vp = camera.projection * camera.view;
        let (rect, depth) = footprint(vp, &corners(Mat4::IDENTITY), [200, 100]);
        assert!(
            rect[0] > 60 && rect[2] < 140 && rect[1] > 20 && rect[3] < 80,
            "{rect:?}"
        );
        assert!(
            depth[0] > depth[1] && depth[1] > 0.0,
            "reversed-Z near over far: {depth:?}"
        );
        let crossing = Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(
            footprint(vp, &corners(crossing), [200, 100]),
            ([0, 0, 200, 100], [1.0, 0.0])
        );
    }
}
