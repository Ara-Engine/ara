//! Heightfield horizon shadows and sky visibility ([`Quality::horizon`]): distant
//! terrain shadowing at a cost independent of distance, the raster counterpart of
//! Unreal's distance-field far shadows for heightfield worlds.
//!
//! A top-down depth capture of the content's large opaque casters (terrain and
//! buildings; radii under [`MIN_RADIUS`] are left out) becomes the heightfield.
//! From it, `horizonSunMain` marches each texel towards the sun's azimuth and
//! stores the sun's visible share (the sun disk sets the penumbra) beside the top
//! surface height, and
//! `horizonSkyMain` stores the cosine-weighted sky visibility over twelve
//! azimuths. The capture and sky visibility are redone when content is replaced;
//! the sun visibility when the sun moves. Shading takes the minimum of the near
//! shadows and the horizon at the top surface (look-horizon.slangh), and scales
//! the sky's light by its visibility outside the probe volume.
//!
//! [`Quality::horizon`]: super::Quality::horizon
use crate::graph::State;
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, Image, Queue, Resource, vk, vulkan::VulkanDevice};
use glam::{Mat4, Vec3};

/// Casters with bound radii under this many metres stay out of the heightfield
/// (foliage, props): the near shadows cover them, the horizon covers landforms.
pub(crate) const MIN_RADIUS: f32 = 4.;
/// Widest heightfield, in metres; larger content keeps its centre.
const MAX_EXTENT: f32 = 8192.;
/// A receiver this far below the heightfield's top surface is under an overhang
/// (a roof, a canopy) the horizon does not describe: it keeps the near shadows.
const TOP_TOLERANCE: f32 = 1.5;

/// What the heightfield holds: its world rectangle (x, z origin and size), the
/// capture's light matrix and the linear depth-to-height mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Capture {
    pub rect: [f32; 4],
    pub matrix: Mat4,
    pub height: [f32; 2],
}

impl Capture {
    /// A top-down capture of world bounds `bounds` (min, max), clamped to
    /// [`MAX_EXTENT`] around their centre. Texel rows run with +Z, columns with +X
    /// (clip +Y is the top row).
    pub(crate) fn new(bounds: [Vec3; 2]) -> Self {
        let [lo, hi] = bounds;
        let centre = (lo + hi) * 0.5;
        let half = ((hi - lo) * 0.5)
            .min(Vec3::splat(MAX_EXTENT * 0.5))
            .max(Vec3::ONE);
        let (bottom, top) = (lo.y - 1., hi.y + 1.);
        let eye = Vec3::new(centre.x, top, centre.z);
        let view = Mat4::look_at_rh(eye, eye - Vec3::Y, Vec3::NEG_Z);
        // Reversed-Z: the top maps to depth one, the bottom to zero.
        let matrix =
            Mat4::orthographic_rh(-half.x, half.x, -half.z, half.z, top - bottom, 0.) * view;
        let depth = |y: f32| matrix.project_point3(Vec3::new(centre.x, y, centre.z)).z;
        let (d0, d1) = (depth(bottom), depth(top));
        let scale = (top - bottom) / (d1 - d0);
        Self {
            rect: [
                centre.x - half.x,
                centre.z - half.z,
                2. * half.x,
                2. * half.z,
            ],
            matrix,
            height: [scale, bottom - d0 * scale],
        }
    }
}

/// The look record's horizon rows (look/frame.rs `horizon_*`).
pub(crate) struct Rows {
    pub rect: [f32; 4],
    pub height: [f32; 4],
    pub images: [u32; 4],
    pub storage: [u32; 4],
}

/// The horizon's GPU resources and what they hold.
pub(crate) struct Horizon {
    /// Capture texels per side (`Quality::horizon`).
    pub resolution: u32,
    /// The top-down D32 capture.
    pub depth: Image,
    /// The sun map (RG16F, capture texels): the sun's visible share and the top
    /// surface height, so shading reads one texel.
    pub sun: Image,
    /// Sky visibility (R16F, a quarter of the capture's texels per side).
    pub sky: Image,
    pub depth_sampled: u32,
    pub sun_indices: [u32; 2],
    pub sky_indices: [u32; 2],
    pub states: [State; 3],
    /// A frame record whose first cascade is the capture matrix: capture draws read
    /// their matrix through it (root word 0) at cascade 0.
    pub frame: Buffer,
    pub frame_index: u32,
    /// What the heightfield holds; `None` until captured, and again after content
    /// is replaced.
    pub capture: Option<Capture>,
    /// The sun direction (bits) the sun visibility holds.
    pub sun_key: Option<[u32; 3]>,
    /// The sky visibility holds the current capture.
    pub sky_done: bool,
}

impl Horizon {
    pub(crate) fn new(gpu: &mut VulkanDevice, resolution: u32, frame_bytes: u64) -> Result<Self> {
        let sky_resolution = sky_resolution(resolution);
        let mut owned: Vec<Resource> = Vec::new();
        let result = (|| {
            let depth = gpu.create_image(
                resolution,
                resolution,
                vk::Format::D32_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            )?;
            owned.push(Resource::Image(depth));
            let storage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE;
            let sun =
                gpu.create_image(resolution, resolution, vk::Format::R16G16_SFLOAT, storage)?;
            owned.push(Resource::Image(sun));
            let sky = gpu.create_image(
                sky_resolution,
                sky_resolution,
                vk::Format::R16_SFLOAT,
                storage,
            )?;
            owned.push(Resource::Image(sky));
            let frame =
                gpu.create_buffer(frame_bytes, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
            owned.push(Resource::Buffer(frame));
            let undefined = State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            };
            Ok::<_, anyhow::Error>(Self {
                resolution,
                depth,
                sun,
                sky,
                depth_sampled: gpu.sampled_image(depth, true)?.index,
                sun_indices: [
                    gpu.sampled_image(sun, true)?.index,
                    gpu.storage_image(sun)?.index,
                ],
                sky_indices: [
                    gpu.sampled_image(sky, true)?.index,
                    gpu.storage_image(sky)?.index,
                ],
                states: [undefined; 3],
                frame,
                frame_index: gpu.storage_buffer(frame)?.index,
                capture: None,
                sun_key: None,
                sky_done: false,
            })
        })();
        if result.is_err() {
            for resource in owned {
                gpu.retire(resource)?;
            }
            gpu.collect()?;
        }
        result
    }

    /// Content was replaced: capture it again, then both visibilities.
    pub(crate) fn invalidate(&mut self) {
        self.capture = None;
        self.sun_key = None;
        self.sky_done = false;
    }

    /// Sky visibility texels per side.
    pub(crate) fn sky_resolution(&self) -> u32 {
        sky_resolution(self.resolution)
    }

    /// The look record's horizon rows for a frame that captures `pending` (else
    /// the held capture) and computes the sun (`sun`) and sky (`sky`) visibility:
    /// the rectangle, the depth-to-height mapping with the top-surface tolerance,
    /// and the sampled images (zero until computed) with the capture texels.
    pub(crate) fn rows(&self, pending: Option<Capture>, sun: bool, sky: bool) -> Option<Rows> {
        let capture = pending.or(self.capture)?;
        let sun = sun || self.sun_key.is_some();
        let sky = sky || self.sky_done;
        Some(Rows {
            rect: capture.rect,
            height: [capture.height[0], capture.height[1], TOP_TOLERANCE, 0.],
            images: [
                self.depth_sampled,
                if sun { self.sun_indices[0] } else { 0 },
                if sky { self.sky_indices[0] } else { 0 },
                self.resolution,
            ],
            storage: [
                self.sun_indices[1],
                self.sky_indices[1],
                self.sky_resolution(),
                0,
            ],
        })
    }

    pub(crate) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        for image in [self.depth, self.sun, self.sky] {
            gpu.retire(Resource::Image(image))?;
        }
        gpu.retire(Resource::Buffer(self.frame))?;
        gpu.collect()?;
        Ok(())
    }
}

fn sky_resolution(resolution: u32) -> u32 {
    (resolution / 4).max(128)
}

/// The sun direction's identity for the sun visibility (exact bits: a computed
/// map holds only the direction it was computed for).
pub(crate) fn sun_key(sun: Vec3) -> [u32; 3] {
    sun.to_array().map(f32::to_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_maps_heights_and_texels_linearly() {
        let capture = Capture::new([Vec3::new(-10., -2., 20.), Vec3::new(30., 8., 60.)]);
        assert_eq!(capture.rect, [-10., 20., 40., 40.]);
        for y in [-2., 0., 5., 8.] {
            let p = capture.matrix.project_point3(Vec3::new(5., y, 31.));
            assert!((capture.height[0] * p.z + capture.height[1] - y).abs() < 1e-3);
            assert!((0.0..=1.0).contains(&p.z), "inside the depth range: {p}");
        }
        // Columns run with +X, rows with +Z (texel row 0 at clip +Y).
        let uv = |x: f32, z: f32| {
            let p = capture.matrix.project_point3(Vec3::new(x, 0., z));
            (p.x * 0.5 + 0.5, 0.5 - p.y * 0.5)
        };
        let (u, v) = uv(-10., 20.);
        assert!(u.abs() < 1e-5 && v.abs() < 1e-5, "{u} {v}");
        let (u, v) = uv(30., 60.);
        assert!((u - 1.).abs() < 1e-5 && (v - 1.).abs() < 1e-5, "{u} {v}");
    }
}
