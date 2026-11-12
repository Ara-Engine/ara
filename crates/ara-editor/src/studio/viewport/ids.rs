//! Object ids for a Renderlab Scene, made on the CPU until Renderlab delivers its own.
//!
//! The Scene's outlines, hover, grid occlusion and clicks read a packed id image
//! (`ara_editor_gpu::IdSource::Packed`): one RGBA8 texel per output pixel holding the
//! little-endian id of the object drawn there, 0 for nothing. The id of draw `n` of the
//! frame is `n + 1`, the same compact channel `ara_render::Renderer::entities()` used, so
//! the editor's channel map, outline groups and click path are unchanged. This module
//! rasterises the frame's own draws from the Scene camera into that image, depth tested,
//! without a GPU: it matches the picture to the accuracy of the draws' triangles (cut-outs,
//! alpha and displacement are not applied; terrain has no draws here, so it carries no id).

use crate::gpu::{IdHit, IdPick};
use anyhow::{Context, Result};
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk,
};
use ara_runtime::{
    components::MeshShape,
    extract::{CameraView, Draw, Geometry, RenderFrame},
};
use ara_runtime_render::geometry::{Source, shape};
use glam34::Vec3;
use std::{collections::HashMap, mem::Discriminant, time::Duration};

/// Nearest depth in metres a triangle is drawn at: Renderlab's near plane.
const NEAR: f32 = 0.05;

/// The frame's channels: draw `n` is channel `n + 1`, as `Renderer::entities()` listed them.
pub(super) fn entities(frame: &RenderFrame) -> Vec<(u64, String)> {
    frame
        .draws
        .iter()
        .map(|draw| (draw.entity, draw.label.clone()))
        .collect()
}

/// The Scene's ids on the presenter's device: the channels, the CPU texels (what a click
/// reads, bit for bit) and the GPU copy the overlays sample. Redraws only when the draws'
/// transforms, the camera or the size change.
#[derive(Default)]
pub(super) struct Ids {
    rasteriser: Rasteriser,
    entities: Vec<(u64, String)>,
    texels: Vec<u8>,
    size: [u32; 2],
    target: Option<Target>,
    key: Option<u64>,
}

/// The packed id image (RGBA8 UNORM, one texel per output pixel) and its staging buffer.
struct Target {
    image: Image,
    staging: Buffer,
    size: [u32; 2],
    /// Never written: the first upload starts from an undefined layout.
    fresh: bool,
}

impl Ids {
    /// Brings the ids up to `frame` seen at `size`. Uploads outside any deferred
    /// completion scope: it waits for the copy.
    pub(super) fn update(
        &mut self,
        gpu: &mut GpuDevice,
        frame: &RenderFrame,
        size: [u32; 2],
    ) -> Result<()> {
        let key = change_key(frame, size);
        if self.key == Some(key) && self.target.as_ref().is_some_and(|t| t.size == size) {
            return Ok(());
        }
        self.entities = entities(frame);
        self.texels = crate::studio::trace::time("scene ids rasterise", Duration::ZERO, || {
            self.rasteriser.ids(frame, size)
        });
        self.size = size;
        if self.target.as_ref().is_some_and(|t| t.size != size) {
            self.release(gpu)?;
        }
        if self.target.is_none() {
            self.target = Some(Target::new(gpu, size)?);
        }
        let target = self
            .target
            .as_mut()
            .context("the id image was created above")?;
        crate::studio::trace::time("scene ids upload", Duration::ZERO, || {
            target.upload(gpu, &self.texels)
        })?;
        self.key = Some(key);
        Ok(())
    }

    /// The id image the overlays sample, once drawn.
    pub(super) fn image(&self) -> Option<Image> {
        self.key.and(self.target.as_ref()).map(|t| t.image)
    }

    /// The ids as the click and the channel map read them.
    pub(super) fn host(&self) -> Option<IdHost<'_>> {
        self.key.map(|_| IdHost {
            entities: &self.entities,
            texels: &self.texels,
            size: self.size,
        })
    }

    /// Hands the image and its staging buffer back to `gpu`.
    pub(super) fn release(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        self.key = None;
        if let Some(target) = self.target.take() {
            gpu.retire(Resource::Image(target.image))?;
            gpu.retire(Resource::Buffer(target.staging))?;
            gpu.collect()?;
        }
        Ok(())
    }
}

impl Target {
    fn new(gpu: &mut GpuDevice, size: [u32; 2]) -> Result<Self> {
        let image = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::SAMPLED,
        )?;
        let staging = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 4,
            vk::BufferUsageFlags::TRANSFER_SRC,
            true,
        )?;
        Ok(Self {
            image,
            staging,
            size,
            fresh: true,
        })
    }

    /// Copies `rgba8` into the image and waits; the image ends in `CopySource`, where the
    /// UI adapter expects an external image.
    fn upload(&mut self, gpu: &mut GpuDevice, rgba8: &[u8]) -> Result<()> {
        // Every earlier use of the staging buffer was resolved below.
        gpu.upload(self.staging, rgba8)?;
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("Scene.IdUpload".into()));
        commands.barriers([Barrier {
            resource: Resource::Image(self.image),
            before: if self.fresh {
                Access::Undefined
            } else {
                Access::CopySource
            },
            after: Access::CopyDestination,
            handoff: None,
        }]);
        commands.push(Command::UploadImage {
            source: self.staging,
            destination: self.image,
        });
        commands.barriers([Barrier {
            resource: Resource::Image(self.image),
            before: Access::CopyDestination,
            after: Access::CopySource,
            handoff: None,
        }]);
        commands.push(Command::EndPass);
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.resolve(point)?;
        // The UI samples this image on its own queue, like the colour copy
        // (`PresentationImage::copy`).
        gpu.publish_submission_to_ui(point)?;
        self.fresh = false;
        Ok(())
    }
}

/// The Scene's ids borrowed for a click: the channel list and the texels.
pub(in crate::studio) struct IdHost<'a> {
    /// Draw `n` is channel `n + 1`.
    pub entities: &'a [(u64, String)],
    texels: &'a [u8],
    size: [u32; 2],
}

impl IdPick for IdHost<'_> {
    fn id_at(&mut self, [x, y]: [u32; 2]) -> Result<IdHit> {
        anyhow::ensure!(
            x < self.size[0] && y < self.size[1],
            "pixel ({x}, {y}) is outside the {:?} id image",
            self.size
        );
        let at = (y as usize * self.size[0] as usize + x as usize) * 4;
        let texel = self
            .texels
            .get(at..at + 4)
            .context("the id image is smaller than its size")?;
        Ok(IdHit::from_packed([texel[0], texel[1], texel[2], texel[3]]))
    }
}

/// What the ids depend on: the size, the camera and each draw's identity and transform.
fn change_key(frame: &RenderFrame, size: [u32; 2]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut add = |bits: u64| hash = (hash ^ bits).wrapping_mul(0x0000_0100_0000_01b3);
    size.iter().for_each(|n| add(u64::from(*n)));
    if let Some(c) = frame.camera {
        c.position
            .iter()
            .chain(&c.forward)
            .chain([&c.fov])
            .for_each(|n| add(u64::from(n.to_bits())));
    }
    for draw in &frame.draws {
        add(draw.entity);
        draw.world
            .to_cols_array()
            .iter()
            .for_each(|n| add(u64::from(n.to_bits())));
        add(draw.skin.as_ref().map_or(0, |r| r.start as u64 + 1));
    }
    // Skinned draws move with the palettes, not their transforms.
    frame
        .palettes
        .iter()
        .flat_map(|m| m.to_cols_array())
        .for_each(|n| add(u64::from(n.to_bits())));
    hash
}

/// Rasterises a frame's draws into a packed id image. Keeps its depth buffer and the
/// procedural shapes between frames.
#[derive(Default)]
pub(super) struct Rasteriser {
    shapes: HashMap<(Discriminant<MeshShape>, [u32; 3]), Source>,
    depth: Vec<f32>,
}

impl Rasteriser {
    /// The packed id image of `frame` seen from its camera at `size` pixels (rows from the
    /// top, RGBA8 bytes as stored). Draws outside the view, behind it, or with a
    /// non-finite transform leave their pixels alone.
    pub(super) fn ids(&mut self, frame: &RenderFrame, size: [u32; 2]) -> Vec<u8> {
        let [width, height] = size.map(|n| n as usize);
        let mut texels = vec![0u8; width * height * 4];
        if width == 0 || height == 0 {
            return texels;
        }
        self.depth.clear();
        self.depth.resize(width * height, 0.);
        let view = View::new(frame.camera, size);
        let Self { shapes, depth } = self;
        for (n, draw) in frame.draws.iter().enumerate() {
            let id = (n as u32 + 1).to_le_bytes();
            let (vertices, indices) = match &draw.geometry {
                Geometry::Shape { shape: kind, size } => {
                    let key = (std::mem::discriminant(kind), size.map(f32::to_bits));
                    let source = shapes.entry(key).or_insert_with(|| shape(*kind, *size));
                    (source.vertices.as_slice(), source.indices.as_slice())
                }
                Geometry::Model { model, primitive } => {
                    let Some(p) = model.primitives.get(*primitive) else {
                        continue;
                    };
                    (p.vertices.as_slice(), p.indices.as_slice())
                }
            };
            let world: Vec<Vec3> = vertices
                .iter()
                .map(|v| world_position(draw, frame, v))
                .collect();
            for triangle in indices.as_chunks::<3>().0 {
                let corner = |k: usize| world.get(triangle[k] as usize).copied();
                let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
                    continue;
                };
                view.draw(a, b, c, |x, y, inverse_w| {
                    let at = y * width + x;
                    if inverse_w > depth[at] {
                        depth[at] = inverse_w;
                        texels[at * 4..at * 4 + 4].copy_from_slice(&id);
                    }
                });
            }
        }
        texels
    }
}

/// A vertex's world position: skinned by the frame's palette when the draw has one,
/// otherwise through the draw's transform alone.
fn world_position(draw: &Draw, frame: &RenderFrame, v: &ara_runtime::model::ModelVertex) -> Vec3 {
    let position = Vec3::from_array(v.position);
    let Some(palette) = draw
        .skin
        .as_ref()
        .and_then(|range| frame.palettes.get(range.clone()))
    else {
        return draw.world.transform_point3(position);
    };
    let mut skinned = Vec3::ZERO;
    let mut total = 0.;
    for (joint, weight) in v.joints.iter().zip(v.weights) {
        if let (Some(matrix), true) = (palette.get(usize::from(*joint)), weight > 0.) {
            skinned += matrix.transform_point3(position) * weight;
            total += weight;
        }
    }
    // Rigid vertices (all weights zero) and a palette that is short keep the rest pose.
    let local = if total > 0. {
        skinned / total
    } else {
        position
    };
    draw.world.transform_point3(local)
}

/// The Scene camera as a pinhole over `size` pixels.
struct View {
    eye: Vec3,
    /// Right, up and forward unit axes of the camera.
    axes: [Vec3; 3],
    /// Pixels per unit of x/y over depth: the focal lengths.
    focal: [f32; 2],
    size: [f32; 2],
}

impl View {
    fn new(camera: Option<CameraView>, size: [u32; 2]) -> Self {
        // As `ara-renderlab`'s `lighting::camera`: the overview of the origin without one.
        let (eye, forward, fov) = match camera {
            Some(c) => (
                Vec3::from_array(c.position),
                Vec3::from_array(c.forward),
                c.fov.to_radians(),
            ),
            None => {
                let eye = Vec3::new(12., 10., 12.);
                (eye, -eye, 55f32.to_radians())
            }
        };
        let up = if forward.normalize_or_zero().y.abs() > 0.999 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let forward = forward.normalize_or(Vec3::NEG_Z);
        let right = forward.cross(up).normalize_or(Vec3::X);
        let true_up = right.cross(forward);
        let size = size.map(|n| n as f32);
        // Vertical field of view: `size[1] / 2` pixels span `tan(fov / 2)` at depth 1.
        let f = size[1] * 0.5 / (fov * 0.5).tan();
        Self {
            eye,
            axes: [right, true_up, forward],
            focal: [f, f],
            size,
        }
    }

    /// `p` in camera space: x right, y up, looking down -z.
    fn to_view(&self, p: Vec3) -> Vec3 {
        let d = p - self.eye;
        Vec3::new(
            d.dot(self.axes[0]),
            d.dot(self.axes[1]),
            -d.dot(self.axes[2]),
        )
    }

    /// Draws one world-space triangle: calls `put(x, y, 1 / depth)` for each pixel whose
    /// centre it covers. Nearer pixels have the larger third argument.
    fn draw(&self, a: Vec3, b: Vec3, c: Vec3, mut put: impl FnMut(usize, usize, f32)) {
        // Camera space looks down -z; clip the triangle to the near plane first.
        let (polygon, count) = clip_near([a, b, c].map(|p| self.to_view(p)));
        if count < 3 {
            return;
        }
        let mut screen = [[0f32; 3]; 4];
        for (out, p) in screen.iter_mut().zip(&polygon[..count]) {
            let depth = -p.z;
            *out = [
                self.size[0] * 0.5 + self.focal[0] * p.x / depth,
                self.size[1] * 0.5 - self.focal[1] * p.y / depth,
                1. / depth,
            ];
        }
        let screen = &screen[..count];
        if screen.iter().flatten().any(|n| !n.is_finite()) {
            return;
        }
        // Wholly off one side of the view: nothing to fill.
        let off = |axis: usize, limit: f32| {
            screen.iter().all(|p| p[axis] < 0.) || screen.iter().all(|p| p[axis] >= limit)
        };
        if off(0, self.size[0]) || off(1, self.size[1]) {
            return;
        }
        for k in 1..count - 1 {
            self.fill(screen[0], screen[k], screen[k + 1], &mut put);
        }
    }

    /// Fills a screen-space triangle (x, y, 1 / depth) with edge functions, both windings.
    fn fill(&self, a: [f32; 3], b: [f32; 3], c: [f32; 3], put: &mut impl FnMut(usize, usize, f32)) {
        let edge = |p: [f32; 3], q: [f32; 3], x: f32, y: f32| {
            (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])
        };
        let area = edge(a, b, c[0], c[1]);
        if area.abs() < 1e-9 {
            return;
        }
        let low =
            |f: fn(f32, f32) -> f32, i: usize, limit: f32| f(f(a[i], b[i]), c[i]).clamp(0., limit);
        let (x0, x1) = (
            low(f32::min, 0, self.size[0] - 1.),
            low(f32::max, 0, self.size[0] - 1.),
        );
        let (y0, y1) = (
            low(f32::min, 1, self.size[1] - 1.),
            low(f32::max, 1, self.size[1] - 1.),
        );
        for y in y0.floor() as usize..=y1.ceil() as usize {
            for x in x0.floor() as usize..=x1.ceil() as usize {
                if x as f32 >= self.size[0] || y as f32 >= self.size[1] {
                    continue;
                }
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w = [edge(b, c, px, py), edge(c, a, px, py), edge(a, b, px, py)];
                let inside = if area > 0. {
                    w.iter().all(|e| *e >= 0.)
                } else {
                    w.iter().all(|e| *e <= 0.)
                };
                if inside {
                    put(x, y, (w[0] * a[2] + w[1] * b[2] + w[2] * c[2]) / area);
                }
            }
        }
    }
}

/// The part of a camera-space triangle at least [`NEAR`] metres in front of the eye: up to
/// four corners, and how many.
fn clip_near(triangle: [Vec3; 3]) -> ([Vec3; 4], usize) {
    let inside = |p: &Vec3| -p.z >= NEAR;
    let mut out = [Vec3::ZERO; 4];
    if triangle.iter().all(inside) {
        out[..3].copy_from_slice(&triangle);
        return (out, 3);
    }
    let mut count = 0;
    for (i, p) in triangle.iter().enumerate() {
        let q = &triangle[(i + 1) % 3];
        if inside(p) {
            out[count] = *p;
            count += 1;
        }
        if inside(p) != inside(q) {
            let t = (-NEAR - p.z) / (q.z - p.z);
            out[count] = p.lerp(*q, t);
            count += 1;
        }
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam34::Mat4;

    fn draw(entity: u64, at: Vec3, size: f32) -> Draw {
        Draw {
            entity,
            address: format!("e{entity}"),
            label: format!("E{entity}"),
            geometry: Geometry::Shape {
                shape: MeshShape::Box,
                size: [size; 3],
            },
            world: Mat4::from_translation(at),
            color: [1.; 4],
            roughness: 0.5,
            metallic: 0.,
            emissive: [0.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
            casts_shadows: true,
            receives_decals: true,
            paint: None,
        }
    }

    /// A camera at the origin looking down -z, 90 degrees.
    fn frame(draws: Vec<Draw>) -> RenderFrame {
        let mut frame = RenderFrame::default();
        frame.camera = Some(CameraView {
            position: [0.; 3],
            forward: [0., 0., -1.],
            fov: 90.,
        });
        frame.draws = draws;
        frame
    }

    fn texel(image: &[u8], width: usize, x: usize, y: usize) -> u32 {
        let at = (y * width + x) * 4;
        u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]])
    }

    #[test]
    fn a_draw_holds_its_index_plus_one_and_the_rest_is_nothing() {
        let frame = frame(vec![draw(7, Vec3::new(0., 0., -4.), 2.)]);
        let image = Rasteriser::default().ids(&frame, [64, 64]);
        assert_eq!(texel(&image, 64, 32, 32), 1);
        assert_eq!(texel(&image, 64, 1, 1), 0);
        assert_eq!(entities(&frame), vec![(7, "E7".to_owned())]);
    }

    /// The box's near face (z = -3) spans x, y in [-1, 1]: at 90 degrees and depth 3 that
    /// is a third of the half width each side of the centre.
    #[test]
    fn a_draws_footprint_is_where_its_triangles_project() {
        let frame = frame(vec![draw(1, Vec3::new(0., 0., -4.), 2.)]);
        let image = Rasteriser::default().ids(&frame, [60, 60]);
        let covered = (0..60 * 60)
            .filter(|n| texel(&image, 60, n % 60, n / 60) == 1)
            .count();
        // The front face is 20 x 20 pixels; the sides only add a thin rim around it.
        assert!((400..=600).contains(&covered), "{covered} pixels");
        assert_eq!(texel(&image, 60, 29, 29), 1);
        assert_eq!(texel(&image, 60, 15, 30), 0);
    }

    #[test]
    fn the_nearer_draw_wins_whatever_the_draw_order() {
        let near = draw(1, Vec3::new(0., 0., -3.), 1.);
        let far = draw(2, Vec3::new(0., 0., -6.), 4.);
        for draws in [vec![near.clone(), far.clone()], vec![far, near]] {
            let near_index = draws
                .iter()
                .position(|d| d.entity == 1)
                .expect("the near draw") as u32
                + 1;
            let far_index = 3 - near_index;
            let image = Rasteriser::default().ids(&frame(draws), [64, 64]);
            assert_eq!(texel(&image, 64, 32, 32), near_index);
            // Outside the near box the far one shows.
            assert_eq!(texel(&image, 64, 16, 32), far_index);
        }
    }

    #[test]
    fn what_is_behind_the_camera_or_across_its_near_plane_is_safe() {
        let behind = draw(1, Vec3::new(0., 0., 5.), 2.);
        let straddling = draw(2, Vec3::new(0., 0., 0.), 4.);
        let image = Rasteriser::default().ids(&frame(vec![behind]), [32, 32]);
        assert!(image.iter().all(|b| *b == 0));
        // The eye is inside the box: every pixel sees its inside faces.
        let image = Rasteriser::default().ids(&frame(vec![straddling]), [32, 32]);
        assert_eq!(texel(&image, 32, 16, 16), 1);
    }

    #[test]
    fn a_zero_sized_target_has_no_texels() {
        assert!(Rasteriser::default().ids(&frame(vec![]), [0, 8]).is_empty());
    }
}
