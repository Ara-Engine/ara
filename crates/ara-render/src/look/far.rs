//! Distant sun shadows past the cascades ([`FarShadows`]): one cached D32 map
//! over a square region around the camera. The region recentres only when the
//! camera travels a quarter of the reach, and the map is redrawn only when what
//! it shows changes (sun, region, settings, or content inside it).
use super::FarShadows;
use crate::graph::State;
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, Image, Queue, Resource, vk, vulkan::VulkanDevice};
use glam::{Mat4, Vec3};

/// What a far map shows; it is redrawn whenever this changes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct FarView {
    sun: [u32; 3],
    /// Region centre in light space, in snap steps.
    centre: [i64; 3],
    settings: FarShadows,
}

/// A far shadow map and what it currently holds.
pub(crate) struct FarMap {
    pub image: Image,
    pub sampled: u32,
    pub state: State,
    pub settings: FarShadows,
    /// A frame record whose first cascade is the far matrix: shadow draws read
    /// their matrix through it (root word 0) at cascade 0.
    pub frame: Buffer,
    pub frame_index: u32,
    /// Light view-projection of the drawn map.
    pub matrix: Mat4,
    /// What the map holds; `None` until first drawn.
    pub drawn: Option<FarView>,
    /// Casters inside the region changed since it was drawn.
    pub stale: bool,
}

impl FarMap {
    pub(crate) fn new(
        gpu: &mut VulkanDevice,
        settings: FarShadows,
        frame_bytes: u64,
    ) -> Result<Self> {
        let image = gpu.create_image(
            settings.resolution,
            settings.resolution,
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        )?;
        let parts = (|| {
            let sampled = gpu.sampled_image(image, true)?.index;
            let frame =
                gpu.create_buffer(frame_bytes, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
            Ok::<_, anyhow::Error>((sampled, frame, gpu.storage_buffer(frame)?.index))
        })();
        let (sampled, frame, frame_index) = match parts {
            Ok(parts) => parts,
            Err(error) => {
                gpu.retire(Resource::Image(image))?;
                gpu.collect()?;
                return Err(error);
            }
        };
        Ok(Self {
            image,
            sampled,
            state: State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
            settings,
            frame,
            frame_index,
            matrix: Mat4::IDENTITY,
            drawn: None,
            stale: false,
        })
    }

    /// The light matrix and view for a camera at `camera` and a sun towards
    /// `sun`: the cascades' fixed light rotation, with the region centre snapped
    /// to a quarter-reach grid (itself whole texels) so small camera moves keep
    /// the drawn map.
    pub(crate) fn fit(settings: FarShadows, camera: Vec3, sun: Vec3) -> (Mat4, FarView) {
        let up = if sun.dot(Vec3::Y).abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let light = Mat4::look_at_rh(Vec3::ZERO, -sun, up);
        // The region reaches `distance` from any camera within its snap cell.
        let radius = settings.distance * 1.25;
        let texel = radius * 2. / settings.resolution as f32;
        let step = ((settings.distance * 0.25 / texel).round() * texel).max(texel);
        let c = light.transform_point3(camera);
        let centre = [c.x, c.y, c.z].map(|v| (v / step).round() as i64);
        let [x, y, z] = centre.map(|v| v as f32 * step);
        // Reversed-Z like the cascades; casters up to 1 km sunward still shadow.
        let matrix = Mat4::orthographic_rh(
            x - radius,
            x + radius,
            y - radius,
            y + radius,
            -(z - radius - 100.),
            -(z + radius + 1000.),
        ) * light;
        let sun = sun.to_array().map(f32::to_bits);
        (
            matrix,
            FarView {
                sun,
                centre,
                settings,
            },
        )
    }

    /// Whether `view` needs a redraw this frame.
    pub(crate) fn needs_draw(&self, view: &FarView) -> bool {
        self.stale || self.drawn != Some(*view)
    }

    pub(crate) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.image))?;
        gpu.retire(Resource::Buffer(self.frame))?;
        gpu.collect()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_camera_moves_keep_the_drawn_view() {
        let settings = FarShadows {
            resolution: 2048,
            distance: 1000.,
        };
        let sun = Vec3::new(0.3, 0.8, 0.2).normalize();
        let (_, a) = FarMap::fit(settings, Vec3::new(10., 2., 5.), sun);
        let (_, b) = FarMap::fit(settings, Vec3::new(40., 3., -20.), sun);
        assert_eq!(a, b, "tens of metres stay in the snap cell");
        let (_, far) = FarMap::fit(settings, Vec3::new(400., 2., 5.), sun);
        assert_ne!(a, far, "a quarter of the reach recentres");
        let (_, turned) = FarMap::fit(settings, Vec3::new(10., 2., 5.), Vec3::Y);
        assert_ne!(a, turned, "a turning sun redraws");
    }

    #[test]
    fn the_region_covers_the_reach_from_anywhere_in_its_cell() {
        let settings = FarShadows {
            resolution: 1024,
            distance: 600.,
        };
        let sun = Vec3::new(-0.4, 0.7, 0.5).normalize();
        for camera in [
            Vec3::ZERO,
            Vec3::new(70., 0., -70.),
            Vec3::new(-74., 10., 74.),
        ] {
            let (matrix, _) = FarMap::fit(settings, camera, sun);
            for direction in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
                let p = matrix.project_point3(camera + direction * 600.);
                assert!(
                    p.x.abs() <= 1. && p.y.abs() <= 1.,
                    "{camera} {direction}: {p}"
                );
                assert!(
                    (0.0..=1.0).contains(&p.z),
                    "{camera} {direction}: depth {p}"
                );
            }
        }
    }
}
