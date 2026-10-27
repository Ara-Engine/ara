//! Sun shadows of blended (alpha-film) casters (look-translucent.slangh). The
//! cascades, far map, local faces and virtual pages draw opaque casters only; a
//! High/Epic frame with blended casters also draws those from the sun into a map
//! fitted to their bounds: their nearest depth (D32), then the product of
//! (1 - opacity) over every film (R16F cleared to one, blended ONE,
//! ONE_MINUS_SRC_ALPHA from a zero colour), so a closed film's two sides pass
//! (1 - a)^2 as a path tracer's alpha shadows do, without dither or noise.
//! Receivers behind the nearest film (the virtual page projection and the High
//! tier's lighting and forward shading) take the filtered transmittance. Frames
//! without blended casters declare nothing; the map is allocated on first use and
//! redrawn only when its fit (the sun, the casters' bounds) changes or casters
//! changed inside its light volume (the cascades' caster-change signal).
use super::{frame::LookFrame, gpu::LookGpu};
use crate::graph::{Graph, ResourceId, State};
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, GpuDevice, Image, Queue, Resource, vk};
use glam::{Mat4, Vec3};

/// Texels per side of the translucent sun map.
pub(crate) const RESOLUTION: u32 = 1024;
/// Receivers this close behind the nearest film (m) still count as on it.
const BIAS_METRES: f32 = 0.02;

/// The map's images, its frame record (shadows[0] is the light matrix, as the far
/// map's) and their last states.
pub(crate) struct TranslucentSun {
    pub depth: Image,
    pub transmittance: Image,
    depth_sampled: u32,
    transmittance_sampled: u32,
    pub frame: Buffer,
    pub frame_index: u32,
    states: [State; 2],
    /// The light matrix the map holds; `None` until drawn.
    drawn: Option<Mat4>,
    /// Casters changed inside the drawn light volume since.
    stale: bool,
}

/// This frame's graph identities of the map: depth, then transmittance.
pub(crate) type TranslucentIds = [ResourceId; 2];

/// The light matrix (reversed-Z, as the cascades) of a map over world bounds
/// `bounds` for a sun towards `sun`, and its depth units per metre.
pub(crate) fn fit(bounds: [Vec3; 2], sun: Vec3) -> (Mat4, f32) {
    let up = if sun.dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let light = Mat4::look_at_rh(Vec3::ZERO, -sun, up);
    let [lo, hi] = super::far::projected(light, bounds[0], bounds[1]);
    // Two texels of margin around the casters; a metre of depth either side.
    let margin = (hi.x - lo.x).max(hi.y - lo.y).max(1e-3) * 2. / RESOLUTION as f32;
    let (near, far) = (-(lo.z - 1.), -(hi.z + 1.));
    let matrix = Mat4::orthographic_rh(
        lo.x - margin,
        hi.x + margin,
        lo.y - margin,
        hi.y + margin,
        near,
        far,
    ) * light;
    (matrix, 1. / (near - far).abs().max(1e-3))
}

impl LookGpu {
    /// Prepares this frame's translucent sun map over blended casters within world
    /// `bounds`: allocates it on first use (`frame_bytes`: the frame record's size)
    /// and writes the look record's rows (look-translucent.slangh). Returns the light
    /// matrix and, when the map must be redrawn, the buffer the caller uploads a copy
    /// of the frame record into with that matrix as cascade 0 (as the far map's)
    /// before culling the casters; `None` keeps the drawn map.
    pub(crate) fn translucent_sun(
        &mut self,
        gpu: &mut GpuDevice,
        look: &mut LookFrame,
        frame_bytes: u64,
        bounds: [Vec3; 2],
        sun: Vec3,
    ) -> Result<(Mat4, Option<Buffer>)> {
        if self.translucent.is_none() {
            self.translucent = Some(TranslucentSun::new(gpu, frame_bytes)?);
        }
        let map = self.translucent.as_ref().expect("allocated above");
        let (matrix, depth_per_metre) = fit(bounds, sun);
        look.refract_info[0][3] = map.frame_index;
        look.refract_params[1] = [
            f32::from_bits(map.transmittance_sampled | (map.depth_sampled << 16)),
            BIAS_METRES * depth_per_metre,
            RESOLUTION as f32,
            0.,
        ];
        let redraw = map.drawn != Some(matrix) || map.stale;
        Ok((matrix, redraw.then_some(map.frame)))
    }

    /// The map now holds `matrix`'s view (its pass was recorded this frame).
    pub(crate) fn translucent_drawn(&mut self, matrix: Mat4) {
        if let Some(map) = &mut self.translucent {
            map.drawn = Some(matrix);
            map.stale = false;
        }
    }

    /// Imports the map for this frame's draw and its readers.
    pub(crate) fn translucent_ids(&self, graph: &mut Graph) -> Option<TranslucentIds> {
        let map = self.translucent.as_ref()?;
        Some([
            graph.import(Resource::Image(map.depth), map.states[0]),
            graph.import(Resource::Image(map.transmittance), map.states[1]),
        ])
    }

    /// Records the map's final states of a compiled frame.
    pub(crate) fn translucent_finish(&mut self, ids: TranslucentIds, final_states: &[State]) {
        if let Some(map) = &mut self.translucent {
            map.states = ids.map(|id| final_states[id.0]);
        }
    }
}

impl TranslucentSun {
    fn new(gpu: &mut GpuDevice, frame_bytes: u64) -> Result<Self> {
        let mut owned = Vec::new();
        let result = (|| {
            let depth = gpu.create_image(
                RESOLUTION,
                RESOLUTION,
                vk::Format::D32_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            )?;
            owned.push(Resource::Image(depth));
            let transmittance = gpu.create_image(
                RESOLUTION,
                RESOLUTION,
                vk::Format::R16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT,
            )?;
            owned.push(Resource::Image(transmittance));
            let frame =
                gpu.create_buffer(frame_bytes, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
            owned.push(Resource::Buffer(frame));
            let undefined = State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            };
            Ok::<_, anyhow::Error>(Self {
                depth_sampled: gpu.sampled_image(depth, true)?.index,
                transmittance_sampled: gpu.sampled_image(transmittance, false)?.index,
                frame_index: gpu.storage_buffer(frame)?.index,
                depth,
                transmittance,
                frame,
                states: [undefined; 2],
                drawn: None,
                stale: false,
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

    /// Records this frame's caster changes (`all`: new content; `bounds`: old and new
    /// world bounds of changed casters) against the drawn map.
    pub(crate) fn observe(&mut self, all: bool, bounds: &[[Vec3; 2]]) {
        if let Some(matrix) = self.drawn {
            self.stale |= all
                || bounds
                    .iter()
                    .any(|&[lo, hi]| super::far::overlaps(matrix, lo, hi));
        }
    }

    /// Retires the map of a released view.
    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.depth))?;
        gpu.retire(Resource::Image(self.transmittance))?;
        gpu.retire(Resource::Buffer(self.frame))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fit_holds_the_casters_inside_reversed_depth() {
        let sun = Vec3::new(0.3, 0.8, -0.4).normalize();
        let bounds = [Vec3::new(-1., 0., 3.), Vec3::new(2., 0.6, 5.)];
        let (matrix, per_metre) = fit(bounds, sun);
        let [lo, hi] = super::super::far::projected(matrix, bounds[0], bounds[1]);
        assert!(lo.x > -1. && hi.x < 1. && lo.y > -1. && hi.y < 1.);
        assert!(lo.z > 0. && hi.z < 1.);
        // Reversed: a point towards the sun is nearer (larger depth).
        let centre = (bounds[0] + bounds[1]) * 0.5;
        assert!(
            matrix.project_point3(centre + sun).z > matrix.project_point3(centre).z,
            "nearer the sun is deeper in reversed Z"
        );
        let step = matrix.project_point3(centre + sun).z - matrix.project_point3(centre).z;
        assert!((step - per_metre).abs() < 1e-4, "{step} vs {per_metre}");
    }
}
