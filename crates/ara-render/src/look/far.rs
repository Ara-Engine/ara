//! Distant sun shadows past the cascades ([`FarShadows`]): one cached D32 map
//! over a square region around the camera. The region recentres only when the
//! camera travels a quarter of the reach, and the map is redrawn only when what
//! it shows changes (sun, region, settings, content, or casters inside it).
use super::FarShadows;
use crate::graph::State;
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, Image, Queue, Resource, vk, vulkan::VulkanDevice};
use glam::{Mat4, Vec3};

/// Frames a far map waits before redrawing for moved casters (distant moving
/// shadows may lag this much); a new view or new content redraws at once.
const STALE_FRAMES: u32 = 30;

/// What a far map shows; it is redrawn whenever this changes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct FarView {
    sun: [u32; 3],
    /// Region centre in light space, in snap steps.
    centre: [i64; 3],
    settings: FarShadows,
}

/// What a far map holds and whether it must be redrawn.
#[derive(Default)]
pub(crate) struct FarCache {
    /// Light view-projection of the drawn map.
    pub matrix: Mat4,
    /// The drawn view; `None` until first drawn.
    pub view: Option<FarView>,
    /// Casters inside the region changed since it was drawn.
    stale: bool,
    /// The content was replaced: redraw on the next frame.
    urgent: bool,
    /// Frames since the map was drawn.
    since: u32,
}

impl FarCache {
    /// Whether `view` needs a redraw this frame.
    pub(crate) fn needs_draw(&self, view: &FarView) -> bool {
        self.view != Some(*view) || self.urgent || (self.stale && self.since >= STALE_FRAMES)
    }

    /// Records this frame's caster changes (`all`: new content; `bounds`: old
    /// and new world bounds of changed casters) against the drawn region.
    pub(crate) fn observe(&mut self, all: bool, bounds: &[[Vec3; 2]]) {
        self.since = self.since.saturating_add(1);
        if self.view.is_none() {
            return;
        }
        self.urgent |= all;
        self.stale |= all || bounds.iter().any(|&[lo, hi]| overlaps(self.matrix, lo, hi));
    }

    /// The map now holds `view`, drawn with `matrix`.
    pub(crate) fn drawn(&mut self, matrix: Mat4, view: FarView) {
        *self = Self {
            matrix,
            view: Some(view),
            ..Self::default()
        };
    }
}

/// A far shadow map's GPU resources and what it currently holds.
pub(crate) struct FarMap {
    pub image: Image,
    pub sampled: u32,
    pub state: State,
    pub settings: FarShadows,
    /// A frame record whose first cascade is the far matrix: shadow draws read
    /// their matrix through it (root word 0) at cascade 0.
    pub frame: Buffer,
    pub frame_index: u32,
    pub cache: FarCache,
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
            cache: FarCache::default(),
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

    pub(crate) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.image))?;
        gpu.retire(Resource::Buffer(self.frame))?;
        gpu.collect()?;
        Ok(())
    }
}

/// Whether world bounds `lo..hi` reach the light volume of `matrix`.
pub(crate) fn overlaps(matrix: Mat4, lo: Vec3, hi: Vec3) -> bool {
    let [min, max] = projected(matrix, lo, hi);
    min.x <= 1. && max.x >= -1. && min.y <= 1. && max.y >= -1. && min.z <= 1. && max.z >= 0.
}

/// Clip-space bounds of world bounds `lo..hi` through the orthographic `matrix`.
pub(crate) fn projected(matrix: Mat4, lo: Vec3, hi: Vec3) -> [Vec3; 2] {
    let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for corner in 0..8u32 {
        let pick = |bit: u32, axis: usize| {
            if corner >> bit & 1 == 0 {
                lo[axis]
            } else {
                hi[axis]
            }
        };
        let p = matrix.project_point3(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)));
        (min, max) = (min.min(p), max.max(p));
    }
    [min, max]
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
    fn casters_inside_the_region_stale_the_map_and_redraw_after_a_delay() {
        let settings = FarShadows {
            resolution: 1024,
            distance: 600.,
        };
        let sun = Vec3::new(0.2, 0.9, 0.1).normalize();
        let (matrix, view) = FarMap::fit(settings, Vec3::ZERO, sun);
        let mut cache = FarCache::default();
        assert!(cache.needs_draw(&view), "never drawn");
        cache.drawn(matrix, view);
        cache.observe(
            false,
            &[[Vec3::new(5000., 0., 0.), Vec3::new(5001., 1., 1.)]],
        );
        assert!(!cache.needs_draw(&view), "a caster outside the region");
        cache.observe(false, &[[Vec3::new(50., 0., 0.), Vec3::new(51., 2., 1.)]]);
        assert!(!cache.needs_draw(&view), "moved casters wait a few frames");
        for _ in 0..STALE_FRAMES {
            cache.observe(false, &[]);
        }
        assert!(cache.needs_draw(&view));
        cache.drawn(matrix, view);
        cache.observe(true, &[]);
        assert!(cache.needs_draw(&view), "new content redraws at once");
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
